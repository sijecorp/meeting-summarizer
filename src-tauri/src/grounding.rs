//! The grounding check (M6.5 plan 5.6, E1, ENG11): after every summary, on every route, the action
//! items' owners and deadlines the transcript does not back become "미정". The LLM's text stays as
//! `summary.raw.md`; the checked one is `summary.generated.md`, which review edits and the edit count
//! (SND-03) compares against.
//!
//! It only asks whether a name or a date is somewhere in the transcript, so an action item pairing
//! another task's name and date passes: `grounding_replaced` counts ungrounded names and dates, not
//! hallucinations.
//!
//! Lines are read as the meeting document reads them (`parseDoc`, MeetingParts.tsx); the line
//! parser knows no sections, so the check follows the headings. `tests/fixtures/summary-lines.json`
//! holds example lines that this module's tests and Vitest both read, so the two cannot drift.

const UNDECIDED: &str = "미정";
/// The action items' heading, spaces aside (architecture 6.2).
const ACTIONS: &str = "액션아이템";
/// What splits an owner cell into people: "민수, 지현", "민수 및 지현", "민수 & 지현".
pub const SEPARATORS: [char; 5] = [',', '·', '/', '&', '및'];
/// Titles and honorifics after a name, dropped before it is looked for: "김민수 팀장님" is 김민수.
const TITLES: [&str; 9] = [
    "팀장",
    "과장",
    "대리",
    "부장",
    "차장",
    "사원",
    "매니저",
    "님",
    "씨",
];
/// Owners that name a group, not a person (spaces aside, lowercase): not judged.
const GROUPS: [&str; 7] = ["전원", "모두", "다들", "팀전체", "전체", "all", "everyone"];
/// After a number of days, a span ("3일 안에") rather than a date: not judged.
const SPANS: [&str; 7] = ["안", "이내", "내", "동안", "간", "후", "뒤"];

/// One summary line, as `parseDoc` reads it.
#[derive(Debug, PartialEq)]
#[cfg_attr(test, derive(serde::Deserialize))]
#[cfg_attr(test, serde(tag = "kind", rename_all = "snake_case"))]
pub enum Block {
    Heading {
        text: String,
    },
    Item {
        owner: Option<String>,
        text: String,
        due: Option<String>,
    },
    /// `- 없음`.
    #[cfg_attr(test, serde(rename = "none"))]
    Nothing,
    Text {
        text: String,
    },
    Gap,
}

/// `parseDoc` for one line: `#` to `###` headings, `- [담당자] 할 일 (기한)` items (`-`, `*` or `•`),
/// `- 없음`, and everything else as typed.
pub fn parse(line: &str) -> Block {
    let hashes = line.len() - line.trim_start_matches('#').len();
    if (1..=3).contains(&hashes)
        && let Some(text) = line[hashes..].strip_prefix(' ')
    {
        return Block::Heading { text: text.into() };
    }
    let Some(rest) = ["- ", "* ", "• "].iter().find_map(|m| line.strip_prefix(m)) else {
        return match line.trim() {
            "" => Block::Gap,
            _ => Block::Text { text: line.into() },
        };
    };
    if rest.trim() == "없음" {
        return Block::Nothing;
    }
    // A leading [owner], but not a link's [text](url).
    let (owner, rest) = match rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
        Some((owner, after)) if !owner.is_empty() && !after.starts_with('(') => {
            (Some(owner.into()), after.trim_start())
        }
        _ => (None, rest),
    };
    // A trailing (due) with no parentheses inside.
    let body = rest.trim_end();
    let due = body
        .strip_suffix(')')
        .and_then(|b| b.rsplit_once('('))
        .filter(|(_, due)| !due.is_empty() && !due.contains(')'));
    let (text, due) = match due {
        Some((text, due)) => (text.trim_end(), Some(due.into())),
        None => (body, None),
    };
    Block::Item {
        owner,
        text: text.into(),
        due,
    }
}

/// What the check made of a summary.
#[derive(Debug, PartialEq)]
pub struct Grounded {
    /// `summary.generated.md`.
    pub text: String,
    /// Owners and deadlines made "미정" (`grounding_replaced`).
    pub replaced: u32,
    /// False when the action items could not be read: no such heading, or a line under it that is
    /// not `- [담당자] 할 일 (기한)`, `- 없음` or empty. Nothing is replaced then.
    pub checked: bool,
}

/// Checks the action items of `summary` against `transcript`, with the glossary's names.
pub fn check(summary: &str, transcript: &str, glossary: &[String]) -> Grounded {
    let heard = Heard::new(transcript);
    let unchecked = || Grounded {
        text: summary.into(),
        replaced: 0,
        checked: false,
    };
    let (mut lines, mut replaced, mut found, mut actions) = (Vec::new(), 0, false, false);
    for line in summary.split('\n') {
        let mut line = line.to_string();
        match parse(&line) {
            Block::Heading { text } => {
                actions = squash(&text) == ACTIONS;
                found |= actions;
            }
            Block::Item {
                owner: Some(owner),
                due,
                ..
            } if actions => {
                // The due date first: it comes after the owner, so the owner's place stays put.
                if let Some(due) = due.filter(|d| d != UNDECIDED && !dated(d, &heard)) {
                    let at = line.rfind('(').expect("a parsed due date") + 1;
                    line.replace_range(at..at + due.len(), UNDECIDED);
                    replaced += 1;
                }
                let mut cell = String::new();
                let mut start = 0;
                for (at, sep) in owner.match_indices(SEPARATORS).chain([(owner.len(), "")]) {
                    let piece = &owner[start..at];
                    let name = piece.trim();
                    if name.is_empty() || name == UNDECIDED || named(name, &heard, glossary) {
                        cell += piece;
                    } else {
                        cell += &piece.replacen(name, UNDECIDED, 1);
                        replaced += 1;
                    }
                    cell += sep;
                    start = at + sep.len();
                }
                let at = line.find('[').expect("a parsed owner") + 1;
                line.replace_range(at..at + owner.len(), &cell);
            }
            Block::Item { .. } | Block::Text { .. } if actions => return unchecked(),
            _ => {}
        }
        lines.push(line);
    }
    if !found {
        return unchecked();
    }
    Grounded {
        text: lines.join("\n"),
        replaced,
        checked: true,
    }
}

/// The text without its whitespace: "김 과장님" and "김과장님" are the same.
fn squash(text: &str) -> String {
    text.split_whitespace().collect()
}

/// The transcript as the check reads it: its words (lowercase, without punctuation at their ends)
/// and the dates it names.
pub struct Heard {
    words: Vec<String>,
    dates: Vec<Date>,
}

impl Heard {
    pub fn new(transcript: &str) -> Self {
        let words = transcript
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|w| !w.is_empty())
            .collect();
        Self {
            words,
            dates: dates(&squash(transcript)),
        }
    }

    /// `text` (squashed, lowercase) is said: inside one word, or across words from a word's start,
    /// so "김 과장님" says 김과장 but "끝나서 연락" does not say 서연. Each start looks at no more
    /// words than `text` has characters.
    fn says(&self, text: &str) -> bool {
        (0..self.words.len()).any(|i| {
            let mut run = String::new();
            for word in &self.words[i..] {
                if run.is_empty() && word.contains(text) {
                    return true;
                }
                run += word;
                if run.len() >= text.len() {
                    return run.starts_with(text);
                }
                if !text.starts_with(&run) {
                    return false;
                }
            }
            false
        })
    }
}

/// An owner the transcript backs (`Heard::says`): the name, spaces, case and a title after it
/// (`TITLES`) aside. A group (`GROUPS`) is not judged. A glossary name of three Hangul syllables or
/// more also counts when any two of them in a row are inside a word: the transcription may
/// misspell it, and 김민수 is called "민수 씨". Any other needs all of it: the glossary is in the
/// prompt, so the model can name someone nobody mentioned.
pub fn named(name: &str, heard: &Heard, glossary: &[String]) -> bool {
    let name = person(name);
    let syllables: Vec<char> = name.chars().collect();
    GROUPS.contains(&name.as_str())
        || heard.says(&name)
        || (syllables.len() >= 3
            && syllables.iter().all(|c| ('가'..='힣').contains(c))
            && glossary.iter().any(|g| person(g) == name)
            && syllables.windows(2).any(|pair| {
                let pair = String::from_iter(pair);
                heard.words.iter().any(|w| w.contains(&pair))
            }))
}

/// A name as it is looked for: no spaces, lowercase, without titles after it while two characters
/// are left ("김 과장님" is 김과장, not 김).
fn person(name: &str) -> String {
    let mut name = squash(name).to_lowercase();
    while let Some(rest) = TITLES
        .iter()
        .find_map(|t| name.strip_suffix(t))
        .filter(|rest| rest.chars().count() >= 2)
    {
        name = rest.to_string();
    }
    name
}

/// Month (when named) and day.
type Date = (Option<u32>, u32);

/// A deadline the transcript backs, a "기한:" before it and 까지, 에 or 까지는 after it aside. One
/// that starts with a date ("10월 18일", "10/18", "18일", a year before it or not, "2026-10-18")
/// needs the same day there, in the same month when both name one: "10월 18일" is backed by
/// "10월 18일" or "18일", "18일" by "18일". A relative one (내일, 모레, a weekday, 이번 주 or 다음 주
/// with or without one) needs the same words. Any other shape, a span of days ("3일 안에") too, is
/// not judged.
fn dated(due: &str, heard: &Heard) -> bool {
    let due = squash(due);
    // Qwen3.5-4B writes the prompt's word too: "(기한: 금요일)".
    let due = due.strip_prefix("기한:").unwrap_or(&due);
    let due = ["까지는", "까지", "에"]
        .iter()
        .find_map(|p| due.strip_suffix(p))
        .unwrap_or(due);
    let chars: Vec<char> = due.chars().collect();
    // A year is not judged, the month and day after it are.
    let date = match chars.get(4) {
        Some('년' | '-') if chars[..4].iter().all(char::is_ascii_digit) => &chars[5..],
        _ => &chars[..],
    };
    if let Some(((month, day), len)) = date_at(date) {
        let rest = String::from_iter(&date[len..]);
        return SPANS.iter().any(|s| rest.starts_with(s))
            || heard
                .dates
                .iter()
                .any(|&(m, d)| d == day && (month.is_none() || m.is_none() || m == month));
    }
    !relative(due) || heard.says(due)
}

fn relative(due: &str) -> bool {
    let weekday = due
        .strip_prefix("이번주")
        .or_else(|| due.strip_prefix("다음주"));
    if weekday == Some("") || matches!(due, "내일" | "모레") {
        return true;
    }
    let mut chars = weekday.unwrap_or(due).chars();
    chars.next().is_some_and(|c| "월화수목금토일".contains(c)) && chars.as_str() == "요일"
}

/// Every date in `text` (squashed), in the shapes `date_at` reads, each read once: "11월18일" is
/// November 18, never also a bare 18th.
fn dates(text: &str) -> Vec<Date> {
    let chars: Vec<char> = text.chars().collect();
    let (mut found, mut i) = (Vec::new(), 0);
    while i < chars.len() {
        let date = date_at(&chars[i..]).filter(|_| i == 0 || !chars[i - 1].is_ascii_digit());
        match date {
            Some((date, len)) => {
                found.push(date);
                i += len;
            }
            None => i += 1,
        }
    }
    found
}

/// A date at the start of `s` (squashed), with its length: "10월18일", "10/18", "10-18" or "18일".
/// Numbers have one or two digits, so "2026/10/18" is read from its "10".
fn date_at(s: &[char]) -> Option<(Date, usize)> {
    let number = |at: usize| {
        let digits: String = s
            .get(at..)?
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let n = digits.parse::<u32>().ok().filter(|_| digits.len() <= 2)?;
        Some((n, at + digits.len()))
    };
    let (first, at) = number(0)?;
    let (date, end) = match s.get(at) {
        Some('월') => {
            let (day, at) = number(at + 1)?;
            (s.get(at) == Some(&'일')).then_some(((Some(first), day), at + 1))?
        }
        Some('/' | '-') => {
            let (day, at) = number(at + 1)?;
            ((Some(first), day), at)
        }
        Some('일') => ((None, first), at + 1),
        _ => return None,
    };
    let (month, day) = date;
    (month.is_none_or(|m| (1..=12).contains(&m)) && (1..=31).contains(&day)).then_some((date, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// The parser reads every shared example line as `parseDoc` does (Meeting.test.tsx reads the
    /// same file).
    #[test]
    fn lines_read_as_the_meeting_document_reads_them() {
        #[derive(Deserialize)]
        struct Case {
            line: String,
            block: Block,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../tests/fixtures/summary-lines.json")).unwrap();
        assert!(cases.len() > 20);
        for case in cases {
            assert_eq!(parse(&case.line), case.block, "{:?}", case.line);
        }
    }

    const HEARD: &str = "민수 씨는 금요일까지 릴리스 노트 써 주세요\n김과장님이 18일에 발표합니다\n지현 님이 QA 팀이랑 다음 주 수요일 일정 맞춰 주세요\n결제는 10/20 전에 끝내요";

    fn actions(items: &str) -> String {
        format!(
            "# 주간 회의\n\n## 결정사항\n- [철수] 결정은 그대로 (내일)\n## 액션 아이템\n{items}\n## 미해결 질문\n- 없음"
        )
    }

    fn ground(items: &str, glossary: &[&str]) -> (String, u32, bool) {
        heard_as(HEARD, items, glossary)
    }

    /// `items` checked against `transcript`.
    fn heard_as(transcript: &str, items: &str, glossary: &[&str]) -> (String, u32, bool) {
        let glossary: Vec<String> = glossary.iter().map(|g| g.to_string()).collect();
        let g = check(&actions(items), transcript, &glossary);
        let items = g.text.split("## 액션 아이템\n").nth(1).unwrap();
        let items = items.split("\n## 미해결 질문").next().unwrap();
        (items.to_string(), g.replaced, g.checked)
    }

    fn same(items: &str) {
        assert_eq!(ground(items, &[]), (items.to_string(), 0, true), "{items}");
    }

    #[test]
    fn owners_in_the_transcript_stay_and_others_become_undecided_one_by_one() {
        same("- [민수] 릴리스 노트 (금요일)");
        // Honorifics and spaces aside, either way round.
        same(
            "- [민수 씨] 릴리스 노트\n- [김 과장님] 발표\n- [QA팀] 회귀 테스트\n- [미정] 대시보드 (미정)",
        );
        assert_eq!(
            ground("- [철수] 릴리스 노트 (금요일)", &[]),
            ("- [미정] 릴리스 노트 (금요일)".into(), 1, true)
        );
        // Only the one nobody named, the separators as written.
        assert_eq!(
            ground("* [민수, 철수 · 지현 님/영희] 일정 맞추기", &[]),
            ("* [민수, 미정 · 지현 님/미정] 일정 맞추기".into(), 2, true)
        );
    }

    /// ENG11: a glossary name of three syllables is backed by any two of them in a row, a
    /// two-syllable one only by all of it, and a name not in the glossary only by all of it.
    #[test]
    fn glossary_names_are_backed_by_two_syllables_in_a_row() {
        assert_eq!(
            ground("- [김민수] 노트", &["김민수"]),
            ("- [김민수] 노트".into(), 0, true)
        );
        assert_eq!(
            ground("- [박지현] 일정", &["박지현"]),
            ("- [박지현] 일정".into(), 0, true)
        );
        assert_eq!(ground("- [김민수] 노트", &[]).1, 1);
        assert_eq!(ground("- [이영희] 노트", &["이영희"]).1, 1);
        assert_eq!(ground("- [철수] 노트", &["철수"]).1, 1);
    }

    #[test]
    fn dates_are_read_as_month_and_day() {
        // "18일에" backs the 18th of any month; "10/20" backs October 20 written either way.
        same(
            "- [민수] 발표 (18일)\n- [민수] 발표 (10월 18일까지)\n- [민수] 결제 (10월 20일)\n- [민수] 결제 (10/20)\n- [민수] 결제 (20일까지는)\n- [민수] 결제 (2024년 10월 20일)",
        );
        // A year the model made up is not judged, but its month and day are.
        assert_eq!(
            ground(
                "- [민수] 발표 (11월 8일)\n- [민수] 발표 (10/19)\n- [민수] 결제 (11월 20일에)\n- [민수] 결제 (2024년 11월 20일)",
                &[]
            ),
            (
                "- [민수] 발표 (미정)\n- [민수] 발표 (미정)\n- [민수] 결제 (미정)\n- [민수] 결제 (미정)".into(),
                4,
                true
            )
        );
        // An owner and a date, each on its own.
        assert_eq!(
            ground("- [철수, 민수] 발표 (19일)", &[]),
            ("- [미정, 민수] 발표 (미정)".into(), 2, true)
        );
    }

    #[test]
    fn relative_days_need_the_same_words_and_other_shapes_are_not_judged() {
        same(
            "- [민수] 노트 (금요일까지)\n- [지현] 일정 (다음 주 수요일)\n- [민수] 정리 (이번 주 안)\n- [민수] 정리 (분기 말)\n- [민수] 노트 (기한: 금요일)",
        );
        assert_eq!(
            ground(
                "- [민수] 노트 (내일)\n- [지현] 일정 (이번 주 수요일)\n- [민수] 노트 (목요일에)\n- [민수] 노트 (기한: 모레)",
                &[]
            ),
            (
                "- [민수] 노트 (미정)\n- [지현] 일정 (미정)\n- [민수] 노트 (미정)\n- [민수] 노트 (미정)".into(),
                4,
                true
            )
        );
    }

    /// Review: a name is never put together from the end of one word and the start of the next,
    /// while spaces inside a name still do not matter.
    #[test]
    fn names_are_not_assembled_across_word_boundaries() {
        for (transcript, owner, glossary) in [
            ("끝나서 연락 주세요", "서연", &[][..]),
            ("그건 하지 현재로는", "지현", &[]),
            ("이 주 안에 끝내요", "이주연", &["이주연"]),
        ] {
            let items = format!("- [{owner}] 노트");
            assert_eq!(
                heard_as(transcript, &items, glossary),
                ("- [미정] 노트".into(), 1, true),
                "{transcript}"
            );
        }
        let kept = |transcript, items: &str| {
            assert_eq!(
                heard_as(transcript, items, &[]),
                (items.to_string(), 0, true),
                "{transcript}"
            )
        };
        kept("김 과장님이 맡아요", "- [김과장] 노트");
        kept("(김 과장님)이 맡아요", "- [김 과장님] 노트");
        kept("박민수 씨가 써요", "- [민수] 노트");
    }

    /// Review: the two-syllable rule is for Hangul names; others need all of it, in any case.
    #[test]
    fn other_scripts_need_the_whole_name_in_any_case() {
        assert_eq!(
            heard_as("deploy plan 정리", "- [Daniel] 배포", &["Daniel"]),
            ("- [미정] 배포".into(), 1, true)
        );
        let items = "- [Daniel] 배포\n- [daniel] 배포";
        assert_eq!(
            heard_as("DANIEL 씨가 배포해요", items, &["Daniel"]),
            (items.into(), 0, true)
        );
    }

    #[test]
    fn groups_titles_and_more_separators() {
        // A group is not a name to back.
        same("- [전원] 회고\n- [팀 전체] 회고\n- [Everyone] 회고\n- [다들] 회고");
        // A title after a name goes before it is looked for.
        assert_eq!(
            ground("- [김민수 팀장] 노트\n- [지현 매니저님] 일정", &["김민수"]),
            (
                "- [김민수 팀장] 노트\n- [지현 매니저님] 일정".into(),
                0,
                true
            )
        );
        assert_eq!(
            ground("- [민수 및 철수] 노트\n- [지현 & 영희] 일정", &[]),
            (
                "- [민수 및 미정] 노트\n- [지현 & 미정] 일정".into(),
                2,
                true
            )
        );
    }

    /// Review: "11월 18일" is November 18 only, never also a bare 18th; ISO dates read as month and
    /// day; spans of days are not dates.
    #[test]
    fn a_dated_transcript_backs_only_its_own_month() {
        let transcript = "민수 씨가 11월 18일에 발표합니다";
        assert_eq!(
            heard_as(transcript, "- [민수] 발표 (10월 18일)", &[]),
            ("- [민수] 발표 (미정)".into(), 1, true)
        );
        let items = "- [민수] 발표 (11월 18일)\n- [민수] 발표 (18일)\n- [민수] 발표 (2026-11-18)";
        assert_eq!(heard_as(transcript, items, &[]), (items.into(), 0, true));
        same(
            "- [민수] 결제 (2026-10-20)\n- [민수] 정리 (3일 안에)\n- [민수] 정리 (5일 이내)\n- [민수] 정리 (2일 동안)",
        );
        assert_eq!(ground("- [민수] 결제 (2026-11-20)", &[]).1, 1);
    }

    /// Only the action items: 결정사항's "[철수] … (내일)" stays.
    #[test]
    fn other_sections_are_left_alone() {
        let summary = actions("- [철수] 노트 (내일)");
        let g = check(&summary, HEARD, &[]);
        assert!(g.text.contains("- [철수] 결정은 그대로 (내일)"));
        assert_eq!(
            g.text,
            summary.replace("- [철수] 노트 (내일)", "- [미정] 노트 (미정)")
        );
    }

    /// ENG11: what cannot be read is not passed as checked, and is left as it is.
    #[test]
    fn unreadable_action_items_or_no_heading_leave_it_unchecked() {
        for items in [
            "- [민수] 노트\n- 철수가 대시보드를 맡는다",
            "철수: 대시보드",
            "- [철수] 노트\n  - 세부 항목",
        ] {
            let summary = actions(items);
            let g = check(&summary, HEARD, &[]);
            assert_eq!(
                g,
                Grounded {
                    text: summary,
                    replaced: 0,
                    checked: false
                },
                "{items}"
            );
        }
        let none = "# 회의\n## 결정사항\n- [철수] 노트 (내일)\n## 할 일\n- [철수] 노트";
        assert!(!check(none, HEARD, &[]).checked);
        // Empty and 없음 lines are read; a heading spelled with other spaces is still the heading.
        let g = check(
            "# 회의\n###  액션아이템 \n- 없음\n\n- [철수] 노트",
            HEARD,
            &[],
        );
        assert_eq!((g.replaced, g.checked), (1, true));
    }
}
