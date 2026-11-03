//! Hallucination filter (TRN-08, architecture 5.5) and the abnormal-result check (TRN-07).

use super::engine::Segment;
use super::lang::LANGS;
use std::collections::HashSet;
use std::sync::LazyLock;

/// Built in: a new list ships with a new build, like every other change during the pilot. One per
/// language of `LANGS`, lowercase so Latin script matches whatever its case (plan 5.6-13).
static PHRASES: LazyLock<Vec<HashSet<String>>> = LazyLock::new(|| {
    LANGS
        .iter()
        .map(|l| {
            l.hallucinations
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .map(|l| normalize(l).to_lowercase())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .collect()
});
/// TRN-07: this much speech by VAD with no text means the chunk failed.
const SPEECH_WITHOUT_TEXT_MS: u64 = 30_000;
/// TRN-07: the same word more than this many times in a row.
const MAX_REPEAT: usize = 8;

/// Punctuation (anything that is neither a letter, a digit nor whitespace) removed, whitespace
/// trimmed and collapsed to single spaces.
pub fn normalize(text: &str) -> String {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Drops a segment only when its whole normalized text is a listed phrase, never on a partial
/// or similar match. A segment with no letters left (only "♪" or "...") goes too: it has no words.
/// Only the lists of the chunk's `languages` apply.
pub fn filter(segments: Vec<Segment>, languages: &[String]) -> Vec<Segment> {
    let lists: Vec<&HashSet<String>> = LANGS
        .iter()
        .zip(PHRASES.iter())
        .filter(|(l, _)| languages.iter().any(|c| c == l.code))
        .map(|(_, phrases)| phrases)
        .collect();
    segments
        .into_iter()
        .filter(|s| {
            let text = normalize(&s.text).to_lowercase();
            !text.is_empty() && !lists.iter().any(|p| p.contains(&text))
        })
        .collect()
}

/// TRN-07: why a chunk's (filtered) result looks broken, if it does.
/// ponytail: words, not Whisper tokens; a phrase repeated without spaces is not caught.
pub fn abnormal(segments: &[Segment], speech_ms: u64) -> Option<&'static str> {
    let words: Vec<String> = segments
        .iter()
        .flat_map(|s| s.text.split_whitespace())
        .map(normalize)
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() && speech_ms > SPEECH_WITHOUT_TEXT_MS {
        return Some("음성 구간인데 전사 결과가 비어 있습니다");
    }
    let mut run = 1;
    for pair in words.windows(2) {
        run = if pair[0] == pair[1] { run + 1 } else { 1 };
        if run > MAX_REPEAT {
            return Some("같은 말이 계속 반복됩니다");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str) -> Segment {
        Segment {
            start_ms: 0,
            end_ms: 1_000,
            text: text.into(),
            lang: None,
        }
    }

    fn langs(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|c| c.to_string()).collect()
    }

    fn texts(segments: Vec<Segment>) -> Vec<String> {
        segments.into_iter().map(|s| s.text).collect()
    }

    #[test]
    fn normalizes_spacing_and_punctuation() {
        assert_eq!(
            normalize("  시청해주셔서,   감사합니다!! "),
            "시청해주셔서 감사합니다"
        );
        assert_eq!(
            normalize("“구독과 좋아요” 부탁드립니다…"),
            "구독과 좋아요 부탁드립니다"
        );
        assert_eq!(normalize("3분기 출시는 10월."), "3분기 출시는 10월");
        assert_eq!(normalize("♪ ... ♪"), "");
    }

    #[test]
    fn removes_only_exact_matches_after_normalizing() {
        let kept = filter(
            vec![
                seg(" 시청해주셔서 감사합니다."),
                seg("시청해 주셔서 감사합니다!"),
                seg("다음 영상에서 만나요~"),
                seg("♪♪"),
                // Similar or containing a listed phrase: real speech, kept.
                seg("오늘 회의에 참석해 주셔서 감사합니다."),
                seg("시청해주셔서 감사합니다. 그럼 시작하겠습니다."),
                seg("다음 영상에서 만나요 라고 하면 안 됩니다"),
            ],
            &langs(&["ko"]),
        );
        assert_eq!(
            texts(kept),
            [
                "오늘 회의에 참석해 주셔서 감사합니다.",
                "시청해주셔서 감사합니다. 그럼 시작하겠습니다.",
                "다음 영상에서 만나요 라고 하면 안 됩니다",
            ]
        );
    }

    #[test]
    fn list_comments_and_blank_lines_are_not_phrases() {
        assert_eq!(PHRASES.len(), LANGS.len());
        for list in PHRASES.iter() {
            assert!(!list.is_empty());
            assert!(list.iter().all(|p| !p.is_empty() && !p.contains("TRN")));
        }
    }

    /// Plan 5.6-13: Latin script in any case, and only the lists of the meeting's languages.
    #[test]
    fn latin_phrases_match_in_any_case_and_only_for_their_language() {
        let segments = || {
            vec![
                seg("THANK YOU FOR WATCHING!"),
                seg("Cảm ơn các bạn đã theo dõi."),
                seg("시청해주셔서 감사합니다"),
                seg("Thank you for watching the demo."),
            ]
        };
        assert_eq!(
            texts(filter(segments(), &langs(&["en", "vi"]))),
            [
                "시청해주셔서 감사합니다",
                "Thank you for watching the demo."
            ]
        );
        assert_eq!(
            texts(filter(segments(), &langs(&["ko"]))),
            [
                "THANK YOU FOR WATCHING!",
                "Cảm ơn các bạn đã theo dõi.",
                "Thank you for watching the demo."
            ]
        );
    }

    #[test]
    fn empty_result_is_abnormal_only_with_enough_speech() {
        assert!(abnormal(&[], 30_001).is_some());
        assert!(abnormal(&[], 30_000).is_none());
        assert!(abnormal(&[seg("...")], 60_000).is_some());
        assert!(abnormal(&[seg("네.")], 60_000).is_none());
    }

    #[test]
    fn a_word_repeated_more_than_eight_times_is_abnormal() {
        let eight = "네 ".repeat(8);
        assert!(abnormal(&[seg(&eight)], 0).is_none());
        let nine = "감사합니다. ".repeat(9);
        assert!(abnormal(&[seg(&nine)], 0).is_some());
        // Across segments too, punctuation ignored.
        let split: Vec<Segment> = (0..9).map(|_| seg("그래서,")).collect();
        assert!(abnormal(&split, 0).is_some());
        // Broken runs do not add up.
        let broken = format!("{}아니 {}", "네 ".repeat(5), "네 ".repeat(5));
        assert!(abnormal(&[seg(&broken)], 0).is_none());
    }
}
