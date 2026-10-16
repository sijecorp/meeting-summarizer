//! Slack (architecture 7, SND-01..06): the message, its length (REV-02), the edit count (SND-03)
//! and the webhook request. Nothing here runs but `send_to_slack`, the review screen's button
//! (SND-05).
//!
//! Pure and tested without a server: `markdown` (header and length), `payload`, `sections` (the
//! mrkdwn fallback) and `edit_count`. The requests are tested against local stub servers.

use crate::settings::{self, SecretKind, SettingsStore};
use crate::store::{self, Meta, Sent, Status, Store, can_transition, one_line};
use crate::summarize::{cut, loopback, pack, split_title};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::ErrorKind;
use std::sync::Mutex;
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

/// REV-02: Slack's limit for a `markdown` block.
pub const MAX_CHARS: usize = 12_000;
/// A `section` block's text limit, for the mrkdwn fallback.
const SECTION_CHARS: usize = 3_000;
/// Architecture 7.1 (eng review D12): the whole request.
const TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Same as the meeting list's.
const UNTITLED: &str = "제목 없음";

fn heading(title: &str) -> &str {
    if title.is_empty() { UNTITLED } else { title }
}

/// `2026-10-13 (화) 14:00`: the meeting's start in the recorder's time zone. The webview gives the
/// offset; Rust cannot read it safely (store.rs).
fn local_start(created_at: &str, offset_minutes: i32) -> String {
    let offset =
        UtcOffset::from_whole_seconds(offset_minutes.saturating_mul(60)).unwrap_or(UtcOffset::UTC);
    let Ok(at) = OffsetDateTime::parse(created_at, &Rfc3339) else {
        return created_at.to_string();
    };
    let at = at.to_offset(offset);
    let weekday = ["월", "화", "수", "목", "금", "토", "일"]
        [usize::from(at.weekday().number_days_from_monday())];
    format!(
        "{:04}-{:02}-{:02} ({weekday}) {:02}:{:02}",
        at.year(),
        at.month() as u8,
        at.day(),
        at.hour(),
        at.minute()
    )
}

/// `1시간 5분`, `42분`, `1분 미만`.
fn length_text(ms: u64) -> String {
    let minutes = ms / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, 0) => "1분 미만".into(),
        (0, m) => format!("{m}분"),
        (h, 0) => format!("{h}시간"),
        (h, m) => format!("{h}시간 {m}분"),
    }
}

/// `<!here>`, `<@U…>` and `<#C…>` as text (7.1): a zero-width space after the `<`. Slack does not
/// say whether the `markdown` block reads them as mentions.
fn defuse(text: &str) -> String {
    text.replace("<!", "<\u{200B}!")
        .replace("<@", "<\u{200B}@")
        .replace("<#", "<\u{200B}#")
}

/// The `markdown` block (7.1): the title in bold, the meeting line, then the body, mentions
/// defused. Its characters are what REV-02 counts; the footer is a block of its own and is not
/// counted. `title` is one line (`store::one_line`).
pub fn markdown(
    title: &str,
    meta: &Meta,
    offset_minutes: i32,
    recorder: &str,
    body: &str,
) -> String {
    let mut line = format!(
        "{} · {}",
        local_start(&meta.created_at, offset_minutes),
        length_text(meta.duration_ms)
    );
    if !recorder.trim().is_empty() {
        line += &format!(" · 녹음: {}", recorder.trim());
    }
    defuse(&format!("**{}**\n{line}\n\n{body}", heading(title)))
}

/// mrkdwn's three control characters, so text cannot become a link or a mention.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `[text](url)` → `<url|text>` for web and mail links only: `(!here)` or `(@U…)` would become a
/// mention. Anything else in brackets, like `[담당자]`, stays as it is.
fn links(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let (before, after) = rest.split_at(open);
        let link = after[1..].split_once("](").and_then(|(text, tail)| {
            let (url, tail) = tail.split_once(')')?;
            let plain = !text.contains(['[', ']'])
                && ["https://", "http://", "mailto:"]
                    .iter()
                    .any(|scheme| url.starts_with(scheme))
                && !url.contains(char::is_whitespace);
            plain.then_some((text, url, tail))
        });
        out += before;
        match link {
            Some((text, url, tail)) => {
                out += &format!("<{url}|{text}>");
                rest = tail;
            }
            None => {
                out += "[";
                rest = &after[1..];
            }
        }
    }
    out + rest
}

/// One Markdown line in mrkdwn (7.1): escaped, `#` headings as bold lines, `**bold**` as `*bold*`,
/// links as `<url|text>`.
fn mrkdwn(line: &str) -> String {
    let line = escape(line);
    let level = line.len() - line.trim_start_matches('#').len();
    let line = match line[level..].strip_prefix(' ') {
        Some(text) if (1..=6).contains(&level) => format!("*{}*", text.trim().replace("**", "")),
        _ => line.replace("**", "*"),
    };
    links(&line)
}

/// The mrkdwn fallback's `section` texts: whole lines, at most 3,000 characters each. A longer
/// line is cut.
fn sections(markdown: &str) -> Vec<String> {
    let lines = markdown.lines().map(mrkdwn).flat_map(|l| {
        if l.chars().count() > SECTION_CHARS {
            cut(&l, SECTION_CHARS)
        } else {
            vec![l]
        }
    });
    pack(lines.collect(), "\n", SECTION_CHARS)
        .into_iter()
        .map(|p| p.join("\n"))
        .filter(|t| !t.trim().is_empty())
        .collect()
}

/// The webhook's JSON (7.1). `fallback`: `section` blocks in mrkdwn instead of the `markdown`
/// block [S5].
fn payload(title: &str, text: &str, edits: u32, fallback: bool) -> Value {
    let mut blocks: Vec<Value> = if fallback {
        sections(text)
            .into_iter()
            .map(|t| json!({ "type": "section", "text": { "type": "mrkdwn", "text": t } }))
            .collect()
    } else {
        vec![json!({ "type": "markdown", "text": text })]
    };
    blocks.push(json!({
        "type": "context",
        "elements": [{ "type": "mrkdwn", "text": format!("AI 요약 · 녹음자 검토 완료 (수정 {edits}곳)") }],
    }));
    // The notification preview, in mrkdwn: an LLM-written `<!channel>` must stay text.
    json!({ "text": format!("{} 회의 요약", escape(heading(title))), "blocks": blocks })
}

/// ONB-06's test message, in mrkdwn: the recorder's name cannot become a mention.
fn test_payload(recorder: &str) -> Value {
    let mut text = "회의 요약 테스트 메시지입니다. 이 채널로 회의록이 전송됩니다.".to_string();
    if !recorder.trim().is_empty() {
        text += &format!("\n녹음: {}", recorder.trim());
    }
    json!({ "text": escape(&text) })
}

/// SND-03 (7.2): lines added or removed between the generated body and the sent one, by a line
/// LCS. A removed line next to an added one is one change, not two. A new title adds 1. Without a
/// generated summary, everything counts against an empty one.
/// ponytail: an n×m table; summaries are a few hundred lines. Trim the common head and tail first
/// if huge pastes ever make it slow.
pub fn edit_count(generated: &str, title: &str, body: &str) -> u32 {
    let (generated_title, generated_body) = split_title(generated);
    let a: Vec<&str> = generated_body.lines().collect();
    let b: Vec<&str> = body.lines().collect();
    // lcs[i][j]: the most lines a[i..] and b[j..] have in common, in order.
    let mut lcs = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    // Between two common lines, `removed` and `added` lines pair up into max(removed, added).
    let (mut i, mut j, mut removed, mut added, mut changes) = (0, 0, 0, 0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            changes += removed.max(added);
            (removed, added) = (0, 0);
            (i, j) = (i + 1, j + 1);
        } else if j < b.len() && (i == a.len() || lcs[i][j + 1] >= lcs[i + 1][j]) {
            added += 1;
            j += 1;
        } else {
            removed += 1;
            i += 1;
        }
    }
    let retitled = one_line(&generated_title.unwrap_or_default()) != title;
    changes + removed.max(added) + u32::from(retitled)
}

/// Why a send failed (SND-06). `message` is what the recorder sees; none holds the webhook URL.
#[derive(Debug, PartialEq)]
enum Failure {
    NoUrl,
    /// Not https (plain http only to this machine), or not a URL.
    BadUrl,
    /// No connection: the only failure sure to have sent nothing.
    Network(String),
    Timeout,
    /// The request may have gone out, but its answer broke off.
    NoAnswer(String),
    /// A redirect, or a 2xx that is not 200 `ok`: a proxy's or a login page's answer, maybe.
    Unconfirmed(u16),
    /// 404, 410, `no_service`, `channel_not_found`, `channel_is_archived`.
    Gone(String),
    /// 403, `action_prohibited`.
    Forbidden(String),
    /// 429.
    RateLimited,
    /// A 400 about the blocks: the webhook does not take the `markdown` block [S5].
    Blocks(String),
    Other(String),
}

impl Failure {
    fn message(&self) -> String {
        match self {
            Failure::NoUrl => "웹훅 주소가 없습니다. 설정 → Slack에서 웹훅 주소를 입력해 주세요".into(),
            Failure::BadUrl => "웹훅 주소가 올바르지 않습니다. 설정 → Slack에서 https://로 시작하는 주소를 입력해 주세요".into(),
            Failure::Network(e) => {
                format!("Slack에 연결하지 못했습니다. 인터넷 연결을 확인해 주세요: {e}")
            }
            // These three may have posted all the same.
            Failure::Timeout => "Slack 응답 시간 초과입니다. 채널에 메시지가 올라갔는지 확인한 뒤 다시 보내 주세요".into(),
            Failure::NoAnswer(e) => format!(
                "Slack 응답을 받지 못했습니다({e}). 채널에 메시지가 올라갔는지 확인한 뒤 다시 보내 주세요"
            ),
            Failure::Unconfirmed(status) => format!(
                "Slack 응답을 확인하지 못했습니다(HTTP {status}). 채널에 메시지가 올라갔는지 확인한 뒤 다시 보내 주세요"
            ),
            Failure::Gone(detail) => format!(
                "웹훅이 없거나 삭제됐습니다({detail}). 설정 → Slack에서 웹훅 주소를 확인해 주세요"
            ),
            Failure::Forbidden(detail) => format!(
                "Slack이 이 채널로 보내는 것을 막았습니다({detail}). Slack 관리자에게 확인해 주세요"
            ),
            Failure::RateLimited => {
                "Slack 전송 한도를 넘었습니다(HTTP 429). 잠시 후 다시 보내 주세요".into()
            }
            Failure::Blocks(detail) | Failure::Other(detail) => format!("Slack 오류: {detail}"),
        }
    }
}

/// The keychain's URL for a webhook: https, or plain http only to `localhost` or a loopback IP (not
/// `*.localhost`, which a resolver may send elsewhere). It is the secret, so it is never returned,
/// logged or shown.
fn webhook_url(secret: Option<String>) -> Result<String, Failure> {
    let url = secret
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or(Failure::NoUrl)?;
    let parsed = reqwest::Url::parse(&url).map_err(|_| Failure::BadUrl)?;
    let subdomain = parsed
        .host_str()
        .is_some_and(|host| host.ends_with(".localhost"));
    if parsed.scheme() == "https" || (parsed.scheme() == "http" && loopback(&url) && !subdomain) {
        Ok(url)
    } else {
        Err(Failure::BadUrl)
    }
}

fn network(e: reqwest::Error) -> Failure {
    // The innermost cause says the most (DNS, TLS, refused), and none carries the URL.
    let e = e.without_url();
    let mut cause: &dyn std::error::Error = &e;
    while let Some(inner) = cause.source() {
        cause = inner;
    }
    if e.is_connect() {
        Failure::Network(cause.to_string())
    } else if e.is_timeout() {
        Failure::Timeout
    } else {
        Failure::NoAnswer(cause.to_string())
    }
}

/// Slack's answer: sent only with 200 and the body `ok`; an error code in the body otherwise.
fn classify(status: u16, body: &str) -> Result<(), Failure> {
    // Slack's error bodies are bare codes. Anything else, like a proxy's page, may quote the URL
    // in some form (its path, cut, escaped), so only the status shows.
    let code = body.trim();
    let detail = || {
        if (1..=64).contains(&code.len())
            && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            format!("HTTP {status}: {code}")
        } else {
            format!("HTTP {status}")
        }
    };
    let says = |codes: &[&str]| codes.iter().any(|c| body.contains(c));
    match status {
        200 if code == "ok" => Ok(()),
        200..=399 => Err(Failure::Unconfirmed(status)),
        429 => Err(Failure::RateLimited),
        404 | 410 => Err(Failure::Gone(detail())),
        _ if says(&["no_service", "channel_not_found", "channel_is_archived"]) => {
            Err(Failure::Gone(detail()))
        }
        _ if status == 403 || says(&["action_prohibited"]) => Err(Failure::Forbidden(detail())),
        400 if says(&["invalid_blocks", "markdown"]) => Err(Failure::Blocks(detail())),
        _ => Err(Failure::Other(detail())),
    }
}

/// One request, never retried: Slack may have posted a message whose answer got lost (7.1).
async fn post(url: &str, payload: &Value, timeout: Duration) -> Result<(), Failure> {
    let mut http = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout)
        // A redirect is an answer of its own (`classify`): followed, it could end on a login
        // page's 200, and the Referer would take the webhook URL to another host.
        .redirect(reqwest::redirect::Policy::none());
    // Like the LLM requests (6.1): this machine is never reached through a proxy.
    if loopback(url) {
        http = http.no_proxy();
    }
    let response = http
        .build()
        .map_err(|e| Failure::Other(e.to_string()))?
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload.to_string())
        .send()
        .await
        .map_err(network)?;
    let status = response.status().as_u16();
    // The status decides first: a body lost after a 200 leaves it unconfirmed, and a failure stays one.
    let text = response.text().await.unwrap_or_default();
    classify(status, &text)
}

/// `summary.generated.md`, or nothing when the meeting has none.
fn generated(store: &Store, id: &str) -> Result<String, String> {
    match std::fs::read_to_string(store.dir_for(id)?.join("summary.generated.md")) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("summary.generated.md을 읽을 수 없습니다: {e}")),
    }
}

/// A webhook's URL by its name.
type Lookup = Box<dyn Fn(&str) -> Result<Option<String>, String> + Send + Sync>;

/// Sending from the review screen.
pub struct Slack {
    /// Meetings being sent right now: a second click while one is out must not post twice.
    sending: Mutex<HashSet<String>>,
    /// The keychain; tests put a stand-in here.
    url: Lookup,
    timeout: Duration,
}

impl Default for Slack {
    fn default() -> Self {
        Self {
            sending: Mutex::default(),
            url: Box::new(|name| settings::get_secret(SecretKind::Slack, name)),
            timeout: TIMEOUT,
        }
    }
}

/// A meeting's place in `sending`, given up when dropped.
struct Claim<'a>(&'a Mutex<HashSet<String>>, &'a str);

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().remove(self.1);
    }
}

impl Slack {
    /// `get_meeting`: a send of this meeting is out, maybe from a review screen since closed.
    pub fn is_sending(&self, id: &str) -> bool {
        self.sending.lock().unwrap().contains(id)
    }

    /// `send_to_slack`: saves the edit the way autosave does, so what is sent is what is stored,
    /// then posts it to `webhook`. Success: `sent` with the channel, time and edit count, and
    /// `webhook` becomes `last_channel` (SND-04). Failure: still `review`, with the cause in
    /// `error` for 다시 보내기 (SND-06).
    #[allow(clippy::too_many_arguments)]
    pub async fn send(
        &self,
        store: &Store,
        settings: &SettingsStore,
        id: &str,
        webhook: &str,
        title: &str,
        body: &str,
        offset_minutes: i32,
    ) -> Result<Meta, String> {
        if !self.sending.lock().unwrap().insert(id.to_string()) {
            return Err("이미 보내는 중입니다".into());
        }
        let _claim = Claim(&self.sending, id);
        if store.get(id)?.no_speech {
            return Err("음성이 거의 감지되지 않아 보낼 요약이 없습니다".into());
        }
        // `review` only. The last attempt's cause goes: it is no longer what happens.
        store.save_summary(id, title, body)?;
        let meta = store.update(id, |m| {
            m.error = None;
            Ok(())
        })?;
        match self
            .deliver(store, settings, &meta, webhook, body, offset_minutes)
            .await
        {
            Ok(sent) => {
                let meta = store
                    .update(id, |m| {
                        if !can_transition(m.status, Status::Sent) {
                            return Err(format!("{:?}에서 보낸 것으로 바꿀 수 없습니다", m.status));
                        }
                        m.status = Status::Sent;
                        m.sent = Some(sent);
                        m.error = None;
                        Ok(())
                    })
                    .map_err(|e| {
                        format!("Slack에는 보냈지만 전송 기록을 저장하지 못했습니다: {e}")
                    })?;
                if let Err(e) = settings.set_last_channel(webhook) {
                    log::warn!("마지막 채널 저장 실패: {e}");
                }
                Ok(meta)
            }
            Err(e) => {
                let failed = store.update(id, |m| {
                    m.error = Some(e.clone());
                    Ok(())
                });
                if let Err(write) = failed {
                    log::error!("전송 실패 기록 실패 ({id}): {write}");
                }
                Err(e)
            }
        }
    }

    /// `send_test_message` (ONB-06): a fixed message through the same checks and request as a
    /// meeting's. `typed`: a URL typed in the Slack tab and not saved yet, sent instead of the
    /// keychain's. Only the outcome comes back, never the URL.
    pub async fn send_test(
        &self,
        settings: &SettingsStore,
        webhook: &str,
        typed: Option<String>,
    ) -> Result<(), String> {
        let secret = match typed.filter(|url| !url.trim().is_empty()) {
            Some(url) => Some(url),
            None => (self.url)(webhook)?,
        };
        let url = webhook_url(secret).map_err(|f| f.message())?;
        let recorder = settings
            .load()
            .map(|l| l.settings.recorder_name)
            .unwrap_or_default();
        post(&url, &test_payload(&recorder), self.timeout)
            .await
            .map_err(|f| f.message())
    }

    /// Between the save and the result: the webhook, the message, the request.
    async fn deliver(
        &self,
        store: &Store,
        settings: &SettingsStore,
        meta: &Meta,
        webhook: &str,
        body: &str,
        offset_minutes: i32,
    ) -> Result<Sent, String> {
        let settings = settings.load()?.settings;
        if !settings.webhooks.iter().any(|w| w.name == webhook) {
            return Err(format!("웹훅 '{webhook}'을(를) 찾을 수 없습니다"));
        }
        let text = markdown(
            &meta.title,
            meta,
            offset_minutes,
            &settings.recorder_name,
            body,
        );
        let over = text.chars().count().saturating_sub(MAX_CHARS);
        if over > 0 {
            return Err(format!(
                "Slack 한도 12,000자를 넘었습니다. {over}자를 줄여 주세요"
            ));
        }
        let edits = edit_count(&generated(store, &meta.id)?, &meta.title, body);
        let url = webhook_url((self.url)(webhook)?).map_err(|f| f.message())?;
        let mut sent = post(
            &url,
            &payload(&meta.title, &text, edits, false),
            self.timeout,
        )
        .await;
        // [S5] A webhook that refuses the `markdown` block gets mrkdwn sections, once.
        if matches!(sent, Err(Failure::Blocks(_))) {
            sent = post(
                &url,
                &payload(&meta.title, &text, edits, true),
                self.timeout,
            )
            .await;
        }
        sent.map_err(|f| f.message())?;
        Ok(Sent {
            channel: webhook.to_string(),
            at: store::utc(OffsetDateTime::now_utc()),
            edit_count: edits,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Settings, Webhook};
    use crate::summarize::tests::{Request, Stub, reply, serve};
    use std::fs;
    use std::net::TcpListener;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Instant;
    use tauri::async_runtime::block_on;

    const SECRET: &str = "/services/T000/B000/secret-token";

    fn meta(created_at: &str, duration_ms: u64) -> Meta {
        Meta {
            schema_version: 1,
            id: "m".into(),
            title: String::new(),
            status: Status::Review,
            created_at: created_at.into(),
            duration_ms,
            gaps: vec![],
            backend: None,
            chunks_done: 0,
            llm_connection: None,
            summarize_local: false,
            no_speech: false,
            error: None,
            sent: None,
        }
    }

    #[test]
    fn the_header_is_in_local_time_with_a_korean_weekday() {
        let at = "2026-10-13T05:00:03Z";
        assert_eq!(local_start(at, 540), "2026-10-13 (화) 14:00");
        // Across midnight either way.
        assert_eq!(
            local_start("2026-10-13T20:30:00Z", 540),
            "2026-10-14 (수) 05:30"
        );
        assert_eq!(local_start(at, -420), "2026-10-12 (월) 22:00");
        assert_eq!(local_start(at, 0), "2026-10-13 (화) 05:00");
        assert_eq!(
            markdown(
                "주간 회의",
                &meta(at, 3_900_000),
                540,
                " 홍길동 ",
                "## 결정사항\n- 없음"
            ),
            "**주간 회의**\n2026-10-13 (화) 14:00 · 1시간 5분 · 녹음: 홍길동\n\n## 결정사항\n- 없음"
        );
        // No recorder name: no "녹음:" at all. No title: the list's placeholder.
        assert_eq!(
            markdown("", &meta(at, 42 * 60_000), 540, " ", "본문"),
            "**제목 없음**\n2026-10-13 (화) 14:00 · 42분\n\n본문"
        );
    }

    #[test]
    fn durations_read_in_hours_and_minutes() {
        assert_eq!(length_text(0), "1분 미만");
        assert_eq!(length_text(59_999), "1분 미만");
        assert_eq!(length_text(60_000), "1분");
        assert_eq!(length_text(42 * 60_000 + 59_000), "42분");
        assert_eq!(length_text(2 * 3_600_000), "2시간");
        assert_eq!(length_text(3_900_000), "1시간 5분");
    }

    #[test]
    fn the_payload_has_the_markdown_block_and_the_footer() {
        let text = "**<주간> 회의**\n2026-10-13 (화) 14:00 · 42분\n\n- 없음";
        assert_eq!(
            payload("<주간> 회의", text, 3, false),
            json!({
                "text": "&lt;주간&gt; 회의 회의 요약",
                "blocks": [
                    { "type": "markdown", "text": text },
                    { "type": "context", "elements": [
                        { "type": "mrkdwn", "text": "AI 요약 · 녹음자 검토 완료 (수정 3곳)" }
                    ]}
                ]
            })
        );
        let fallback = payload("<주간> 회의", text, 3, true);
        assert_eq!(
            fallback["blocks"][0],
            json!({ "type": "section", "text": { "type": "mrkdwn",
                "text": "*&lt;주간&gt; 회의*\n2026-10-13 (화) 14:00 · 42분\n\n- 없음" } })
        );
        assert_eq!(fallback["blocks"][1]["type"], "context");
    }

    #[test]
    fn markdown_becomes_mrkdwn() {
        assert_eq!(mrkdwn("## 결정사항"), "*결정사항*");
        assert_eq!(mrkdwn("# **굵은** 제목 "), "*굵은 제목*");
        assert_eq!(mrkdwn("####### 일곱"), "####### 일곱");
        assert_eq!(mrkdwn("#해시태그"), "#해시태그");
        assert_eq!(mrkdwn("- **배포**는 목요일"), "- *배포*는 목요일");
        assert_eq!(
            mrkdwn("A&B <!channel> 1 > 0"),
            "A&amp;B &lt;!channel&gt; 1 &gt; 0"
        );
        assert_eq!(
            mrkdwn("- [미정] [자료](https://x.example/a?b=1&c=2) 확인 [민수](금요일"),
            "- [미정] <https://x.example/a?b=1&amp;c=2|자료> 확인 [민수](금요일"
        );
        assert_eq!(
            mrkdwn("[빈 주소]() [띄운 주소](https://a b)"),
            "[빈 주소]() [띄운 주소](https://a b)"
        );
        // Web and mail links only: any other target would be a mention or a channel.
        assert_eq!(
            mrkdwn(
                "[담당](!here) [민수](@U123) [모두](!channel) [팀](!subteam^S1) [방](#C1) [f](ftp://x)"
            ),
            "[담당](!here) [민수](@U123) [모두](!channel) [팀](!subteam^S1) [방](#C1) [f](ftp://x)"
        );
        assert_eq!(
            mrkdwn("[메일](mailto:a@x.example) [평문](http://x.example)"),
            "<mailto:a@x.example|메일> <http://x.example|평문>"
        );
    }

    #[test]
    fn mentions_stay_text_in_both_paths_and_count_as_sent() {
        let body = "<!here> 확인 <@U123> <#C456> <!subteam^S1|팀> [x](!here)";
        let text = markdown(
            "<!channel> 회의",
            &meta("2026-10-13T05:00:03Z", 0),
            540,
            "<@U9>",
            body,
        );
        assert_eq!(
            text,
            "**<\u{200B}!channel> 회의**\n2026-10-13 (화) 14:00 · 1분 미만 · 녹음: <\u{200B}@U9>\n\n\
             <\u{200B}!here> 확인 <\u{200B}@U123> <\u{200B}#C456> <\u{200B}!subteam^S1|팀> [x](!here)"
        );
        for fallback in [false, true] {
            let sent = payload("<!channel> 회의", &text, 0, fallback).to_string();
            assert!(
                !["<!", "<@", "<#"].iter().any(|m| sent.contains(m)),
                "{sent}"
            );
        }
    }

    #[test]
    fn sections_split_at_line_ends_within_3000_characters() {
        let line = "가".repeat(999);
        let text = [line.as_str(); 4].join("\n");
        let parts = sections(&text);
        assert_eq!(parts, [[line.as_str(); 3].join("\n"), line.clone()]);
        // One line longer than a section is cut.
        let long = "나".repeat(7_000);
        let lengths: Vec<usize> = sections(&format!("머리\n{long}"))
            .iter()
            .map(|s| s.chars().count())
            .collect();
        assert_eq!(lengths, [2, 3_000, 3_000, 1_000]);
        // Escaping can only grow a line; the limit holds after it.
        assert!(
            sections(&"&".repeat(2_000))
                .iter()
                .all(|s| s.chars().count() <= SECTION_CHARS)
        );
    }

    #[test]
    fn edit_count_pairs_a_removed_line_with_an_added_one() {
        let generated =
            "# 주간 회의\n\n## 결정사항\n- 목요일 배포\n## 액션 아이템\n- [미정] 노트\n";
        let body = "## 결정사항\n- 목요일 배포\n## 액션 아이템\n- [미정] 노트";
        assert_eq!(edit_count(generated, "주간 회의", body), 0);
        // One line changed.
        let changed = body.replace("[미정]", "[민수]");
        assert_eq!(edit_count(generated, "주간 회의", &changed), 1);
        // One added, then one removed elsewhere: two.
        let moved = "## 결정사항\n- 목요일 배포\n- 금요일 회고\n## 액션 아이템";
        assert_eq!(edit_count(generated, "주간 회의", moved), 2);
        // Two lines replaced by three: three.
        let rewritten = "## 결정사항\n- 하나\n- 둘\n- 셋\n- [미정] 노트";
        assert_eq!(edit_count(generated, "주간 회의", rewritten), 3);
        // A new title adds one.
        assert_eq!(edit_count(generated, "주간 개발 회의", &changed), 2);
        // The generated title compares as stored: one line.
        assert_eq!(
            edit_count("#  주간   회의\n- 없음", "주간 회의", "- 없음"),
            0
        );
        // Nothing generated: every line is new, and so is any title.
        assert_eq!(edit_count("", "", "- 하나\n- 둘"), 2);
        assert_eq!(edit_count("", "제목", ""), 1);
    }

    #[test]
    fn failures_are_classified_and_show_a_bare_code_only() {
        let cases = [
            (404, "no_service", "Gone"),
            (410, "channel_is_archived", "Gone"),
            (400, "channel_not_found", "Gone"),
            (403, "action_prohibited", "Forbidden"),
            (403, "posting_to_general_channel_denied", "Forbidden"),
            (429, "rate_limited", "RateLimited"),
            (400, "invalid_blocks", "Blocks"),
            (400, "unsupported block type: markdown", "Blocks"),
            (400, "invalid_payload", "Other"),
            (500, "oops", "Other"),
        ];
        for (status, body, kind) in cases {
            let failure = classify(status, body).unwrap_err();
            assert!(
                format!("{failure:?}").starts_with(kind),
                "{status} {body}: {failure:?}"
            );
        }
        assert_eq!(classify(200, "ok"), Ok(()));
        assert_eq!(classify(200, "ok\n"), Ok(()));
        // Not Slack's own success: a proxy's or a login page's answer, maybe posted, maybe not.
        for (status, body) in [
            (200, "<html>로그인</html>"),
            (200, ""),
            (204, ""),
            (201, "ok"),
            (302, ""),
        ] {
            assert_eq!(
                classify(status, body),
                Err(Failure::Unconfirmed(status)),
                "{status} {body}"
            );
        }
        assert_eq!(
            classify(404, " no_service\n").unwrap_err().message(),
            "웹훅이 없거나 삭제됐습니다(HTTP 404: no_service). 설정 → Slack에서 웹훅 주소를 확인해 주세요"
        );
        // Anything but a bare code shows the status alone: an echo of the URL need not be verbatim.
        let url = format!("https://hooks.slack.com{SECRET}");
        let echoes = [
            format!("bad request to {url}"),
            format!("<html><body>Proxy: access to {SECRET} denied</body></html>"),
            url[..url.len() - 6].to_string(),
            url.replace('/', "\\/"),
            url.replace('/', "%2F"),
        ];
        for body in echoes {
            let failure = classify(500, &body).unwrap_err();
            assert_eq!(failure.message(), "Slack 오류: HTTP 500", "{body}");
            assert!(!format!("{failure:?}").contains("B000"), "{body}");
        }
    }

    #[test]
    fn only_https_or_this_machine_is_a_webhook() {
        let ok = |url: &str| webhook_url(Some(url.into()));
        assert_eq!(
            ok(" https://hooks.slack.com/services/a\n"),
            Ok("https://hooks.slack.com/services/a".into())
        );
        assert!(ok("http://127.0.0.1:9/services/a").is_ok());
        assert!(ok("http://localhost:9/services/a").is_ok());
        assert!(ok("http://[::1]:9/services/a").is_ok());
        // A resolver may send a subdomain of `localhost` anywhere.
        assert_eq!(
            ok("http://hooks.localhost/services/a"),
            Err(Failure::BadUrl)
        );
        assert_eq!(
            ok("http://hooks.slack.com/services/a"),
            Err(Failure::BadUrl)
        );
        assert_eq!(ok("hooks.slack.com/services/a"), Err(Failure::BadUrl));
        assert_eq!(webhook_url(None), Err(Failure::NoUrl));
        assert_eq!(webhook_url(Some(" ".into())), Err(Failure::NoUrl));
    }

    fn webhook(stub: &Stub) -> String {
        format!("{}{SECRET}", stub.url.trim_end_matches("/v1"))
    }

    fn sent_blocks(r: &Request) -> Vec<&str> {
        r.body["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["type"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn a_silent_server_times_out_once_and_a_dropped_one_is_not_retried() {
        let silent = serve(|_| None);
        let started = Instant::now();
        let failure = block_on(post(&webhook(&silent), &json!({}), Duration::from_secs(1)));
        assert_eq!(failure, Err(Failure::Timeout));
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(silent.connections.load(Ordering::SeqCst), 1);

        // Reads the request and hangs up without answering: it may have posted.
        let dropped = serve(|_| Some(Vec::new()));
        let failure = block_on(post(&webhook(&dropped), &json!({}), TIMEOUT)).unwrap_err();
        assert!(matches!(failure, Failure::NoAnswer(_)), "{failure:?}");
        assert!(
            failure
                .message()
                .contains("채널에 메시지가 올라갔는지 확인한 뒤")
        );
        assert!(!failure.message().contains("secret-token"), "{failure:?}");
        thread::sleep(Duration::from_millis(100));
        assert_eq!(dropped.connections.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_refused_connection_never_shows_the_url() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let url = format!("http://127.0.0.1:{port}{SECRET}");
        let failure = block_on(post(&url, &json!({}), TIMEOUT)).unwrap_err();
        // Nothing went out, so no channel to check.
        assert!(matches!(failure, Failure::Network(_)), "{failure:?}");
        assert!(failure.message().starts_with("Slack에 연결하지 못했습니다"));
        assert!(!failure.message().contains("secret-token"), "{failure:?}");
        assert!(!format!("{failure:?}").contains("secret-token"));
    }

    /// A proxy or an SSO gateway in the way: its redirect is not followed (nor its Referer sent),
    /// and neither it nor a login page's 200 counts as sent or takes the mrkdwn fallback.
    #[test]
    fn a_redirect_or_a_200_that_is_not_ok_stays_in_review_after_one_request() {
        let redirect = serve(|r| {
            Some(if r.head.starts_with("post /services/") {
                b"HTTP/1.1 302 Found\r\nLocation: /login\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec()
            } else {
                reply("200 OK", "ok")
            })
        });
        let login = serve(|_| Some(reply("200 OK", "<html><title>로그인</title></html>")));
        for (stub, status) in [(redirect, 302), (login, 200)] {
            let f = fixture();
            f.point_at(&stub);
            let error = f.send("주간 회의", "- 본문").unwrap_err();
            assert_eq!(error, Failure::Unconfirmed(status).message());
            let meta = f.store.get(&f.id).unwrap();
            assert_eq!(
                (meta.status, meta.error.as_deref()),
                (Status::Review, Some(error.as_str()))
            );
            // A followed redirect would be a second connection, and the fallback a second request.
            assert_eq!(stub.connections.load(Ordering::SeqCst), 1);
            assert_eq!(stub.requests.lock().unwrap().len(), 1);
        }
    }

    /// A meeting in review with its generated summary, two webhooks, and a `Slack` whose keychain
    /// holds `url` once `point_at` sets it.
    struct Fixture {
        store: Store,
        settings: SettingsStore,
        slack: Slack,
        url: Arc<Mutex<Option<String>>>,
        id: String,
        _tmp: tempfile::TempDir,
    }

    const GENERATED: &str =
        "# 주간 회의\n\n## 결정사항\n- 목요일 배포\n## 액션 아이템\n- [미정] 노트";

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("meetings")).unwrap();
        let id = store.create("", false).unwrap().id;
        store
            .update(&id, |m| {
                m.status = Status::Review;
                m.title = "주간 회의".into();
                m.created_at = "2026-10-13T05:00:03Z".into();
                m.duration_ms = 3_900_000;
                Ok(())
            })
            .unwrap();
        fs::write(
            store.dir_for(&id).unwrap().join("summary.generated.md"),
            GENERATED,
        )
        .unwrap();
        let settings = SettingsStore::new(tmp.path().join("settings.json"));
        let webhooks = ["#회의록", "#개발"].map(|name| Webhook { name: name.into() });
        settings
            .save(Settings {
                recorder_name: "홍길동".into(),
                webhooks: webhooks.into(),
                ..Settings::default()
            })
            .unwrap();
        let url = Arc::new(Mutex::new(None));
        let keychain = url.clone();
        // Never the real keychain.
        let slack = Slack {
            sending: Mutex::default(),
            url: Box::new(move |_| Ok(keychain.lock().unwrap().clone())),
            timeout: Duration::from_secs(5),
        };
        Fixture {
            store,
            settings,
            slack,
            url,
            id,
            _tmp: tmp,
        }
    }

    impl Fixture {
        fn point_at(&self, stub: &Stub) {
            *self.url.lock().unwrap() = Some(webhook(stub));
        }

        fn send(&self, title: &str, body: &str) -> Result<Meta, String> {
            block_on(self.slack.send(
                &self.store,
                &self.settings,
                &self.id,
                "#개발",
                title,
                body,
                540,
            ))
        }
    }

    #[test]
    fn a_send_saves_the_edit_first_then_marks_it_sent() {
        let f = fixture();
        let summary = f.store.dir_for(&f.id).unwrap().join("summary.md");
        let on_disk = Arc::new(Mutex::new(None));
        let seen = on_disk.clone();
        let stub = serve(move |_| {
            *seen.lock().unwrap() = fs::read_to_string(&summary).ok();
            Some(reply("200 OK", "ok"))
        });
        f.point_at(&stub);
        let body = "## 결정사항\n- 금요일 배포\n## 액션 아이템\n- [미정] 노트";

        let meta = f.send("주간\n회의 ", body).unwrap();

        // What went out was already on disk, as the screen showed it.
        assert_eq!(on_disk.lock().unwrap().as_deref(), Some(body));
        assert_eq!(f.store.summary(&f.id).unwrap(), body);
        assert_eq!(
            (meta.status, meta.title.as_str()),
            (Status::Sent, "주간 회의")
        );
        let sent = meta.sent.unwrap();
        assert_eq!((sent.channel.as_str(), sent.edit_count), ("#개발", 1));
        assert!(
            OffsetDateTime::parse(&sent.at, &Rfc3339).is_ok(),
            "{}",
            sent.at
        );
        assert_eq!(f.store.get(&f.id).unwrap().status, Status::Sent);
        assert_eq!(
            f.settings.load().unwrap().settings.last_channel.as_deref(),
            Some("#개발")
        );

        let requests = stub.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .head
                .starts_with("post /services/t000/b000/secret-token ")
        );
        assert_eq!(sent_blocks(&requests[0]), ["markdown", "context"]);
        let block = requests[0].body["blocks"][0]["text"].as_str().unwrap();
        assert_eq!(
            block,
            format!("**주간 회의**\n2026-10-13 (화) 14:00 · 1시간 5분 · 녹음: 홍길동\n\n{body}")
        );
        assert_eq!(
            requests[0].body["blocks"][1]["elements"][0]["text"],
            "AI 요약 · 녹음자 검토 완료 (수정 1곳)"
        );
    }

    /// [S5] handled at runtime: the `markdown` block is refused, so mrkdwn sections go, once.
    #[test]
    fn a_webhook_that_refuses_the_markdown_block_gets_sections_once() {
        let stub = serve(|r| {
            Some(if sent_blocks(r)[0] == "markdown" {
                reply("400 Bad Request", "invalid_blocks")
            } else {
                reply("200 OK", "ok")
            })
        });
        let f = fixture();
        f.point_at(&stub);

        let meta = f
            .send("주간 회의", "## 결정사항\n- **목요일** 배포")
            .unwrap();

        assert_eq!(meta.status, Status::Sent);
        let requests = stub.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(sent_blocks(&requests[1]), ["section", "context"]);
        assert_eq!(
            requests[1].body["blocks"][0]["text"]["text"],
            "*주간 회의*\n2026-10-13 (화) 14:00 · 1시간 5분 · 녹음: 홍길동\n\n*결정사항*\n- *목요일* 배포"
        );
    }

    #[test]
    fn a_failed_send_stays_in_review_with_the_cause_until_a_resend_works() {
        let f = fixture();
        let gone = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let answer = gone.clone();
        // The cause on disk while each request is out.
        let meta_json = f.store.dir_for(&f.id).unwrap().join("meta.json");
        let causes = Arc::new(Mutex::new(Vec::new()));
        let seen = causes.clone();
        let stub = serve(move |_| {
            let meta: Meta = serde_json::from_slice(&fs::read(&meta_json).unwrap()).unwrap();
            seen.lock().unwrap().push(meta.error);
            Some(if answer.load(Ordering::SeqCst) {
                reply("404 Not Found", "no_service")
            } else {
                reply("200 OK", "ok")
            })
        });
        f.point_at(&stub);

        let error = f.send("주간 회의", "- 고친 본문").unwrap_err();
        assert!(error.contains("웹훅이 없거나 삭제됐습니다"), "{error}");
        assert!(!error.contains("secret-token"));
        let meta = f.store.get(&f.id).unwrap();
        assert_eq!(
            (meta.status, meta.error.as_deref()),
            (Status::Review, Some(error.as_str()))
        );
        assert_eq!(f.settings.load().unwrap().settings.last_channel, None);

        // 다시 보내기.
        gone.store(false, Ordering::SeqCst);
        let meta = f.send("주간 회의", "- 고친 본문").unwrap();
        assert_eq!((meta.status, meta.error), (Status::Sent, None));
        assert_eq!(stub.requests.lock().unwrap().len(), 2);
        // The old cause was gone as soon as the new attempt started.
        assert_eq!(*causes.lock().unwrap(), [None, None]);
    }

    #[test]
    fn no_url_in_the_keychain_says_where_to_put_it() {
        let f = fixture();
        let error = f.send("주간 회의", "- 본문").unwrap_err();
        assert!(error.contains("설정 → Slack"), "{error}");
        assert_eq!(f.store.get(&f.id).unwrap().error, Some(error));
    }

    /// ONB-06: a URL typed but not saved goes first; a blank one means the keychain's.
    #[test]
    fn a_test_message_goes_to_the_typed_url_or_else_the_saved_one() {
        let ok = |_: &Request| Some(reply("200 OK", "ok"));
        let (saved, typed) = (serve(ok), serve(ok));
        let f = fixture();
        f.point_at(&saved);
        let test = |url: Option<String>| block_on(f.slack.send_test(&f.settings, "#개발", url));

        test(Some(webhook(&typed))).unwrap();
        test(Some(" ".into())).unwrap();
        test(None).unwrap();

        let body = |stub: &Stub| stub.requests.lock().unwrap()[0].body.clone();
        assert_eq!(typed.requests.lock().unwrap().len(), 1);
        assert_eq!(saved.requests.lock().unwrap().len(), 2);
        assert_eq!(
            body(&typed),
            json!({ "text": "회의 요약 테스트 메시지입니다. 이 채널로 회의록이 전송됩니다.\n녹음: 홍길동" })
        );
        // Nothing about a meeting changes.
        assert_eq!(f.store.get(&f.id).unwrap().status, Status::Review);

        // The same checks and messages as a meeting's send.
        let error = test(Some("http://hooks.slack.com/services/a".into())).unwrap_err();
        assert!(
            error.starts_with("웹훅 주소가 올바르지 않습니다"),
            "{error}"
        );
        *f.url.lock().unwrap() = None;
        assert!(test(None).unwrap_err().starts_with("웹훅 주소가 없습니다"));
        let gone = serve(|_| Some(reply("404 Not Found", "no_service")));
        let error = test(Some(webhook(&gone))).unwrap_err();
        assert!(error.contains("HTTP 404: no_service"), "{error}");
        assert!(!error.contains("secret-token"), "{error}");
    }

    #[test]
    fn the_test_message_keeps_a_name_from_becoming_a_mention() {
        assert_eq!(
            test_payload("  <!channel> & co ")["text"],
            "회의 요약 테스트 메시지입니다. 이 채널로 회의록이 전송됩니다.\n녹음: &lt;!channel&gt; &amp; co"
        );
        assert_eq!(
            test_payload(" ")["text"],
            "회의 요약 테스트 메시지입니다. 이 채널로 회의록이 전송됩니다."
        );
    }

    #[test]
    fn exactly_12000_characters_go_and_one_more_does_not() {
        let stub = serve(|_| Some(reply("200 OK", "ok")));
        let f = fixture();
        f.point_at(&stub);
        let header = "**주간 회의**\n2026-10-13 (화) 14:00 · 1시간 5분 · 녹음: 홍길동\n\n";
        let room = MAX_CHARS - header.chars().count();

        let error = f.send("주간 회의", &"가".repeat(room + 1)).unwrap_err();
        assert_eq!(error, "Slack 한도 12,000자를 넘었습니다. 1자를 줄여 주세요");
        // A defused mention is one character longer, and the limit counts it as sent.
        let mention = format!("<!here>{}", "가".repeat(room - 7));
        let error = f.send("주간 회의", &mention).unwrap_err();
        assert_eq!(error, "Slack 한도 12,000자를 넘었습니다. 1자를 줄여 주세요");
        assert!(stub.requests.lock().unwrap().is_empty());

        f.send("주간 회의", &"가".repeat(room)).unwrap();
        let requests = stub.requests.lock().unwrap();
        let block = requests[0].body["blocks"][0]["text"].as_str().unwrap();
        // The footer is a block of its own and does not count.
        assert_eq!(block.chars().count(), MAX_CHARS);
    }

    #[test]
    fn a_second_click_while_sending_is_refused() {
        let (release, wait) = mpsc::channel::<()>();
        let wait = Mutex::new(wait);
        let stub = serve(move |_| {
            let _ = wait.lock().unwrap().recv();
            Some(reply("200 OK", "ok"))
        });
        let f = fixture();
        f.point_at(&stub);
        thread::scope(|s| {
            let first = s.spawn(|| f.send("주간 회의", "- 본문"));
            let until = Instant::now() + Duration::from_secs(5);
            while stub.requests.lock().unwrap().is_empty() {
                assert!(Instant::now() < until, "the first send never arrived");
                thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(
                f.send("주간 회의", "- 본문").unwrap_err(),
                "이미 보내는 중입니다"
            );
            // What a review screen opened now is told.
            assert!(f.slack.is_sending(&f.id));
            release.send(()).unwrap();
            assert_eq!(first.join().unwrap().unwrap().status, Status::Sent);
        });
        assert!(!f.slack.is_sending(&f.id));
        assert_eq!(stub.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn no_speech_and_meetings_out_of_review_are_never_sent() {
        let stub = serve(|_| Some(reply("200 OK", "ok")));
        let f = fixture();
        f.point_at(&stub);
        f.store
            .update(&f.id, |m| {
                m.no_speech = true;
                Ok(())
            })
            .unwrap();
        assert!(
            f.send("", "")
                .unwrap_err()
                .contains("음성이 거의 감지되지 않아")
        );

        f.store
            .update(&f.id, |m| {
                m.no_speech = false;
                m.status = Status::Archived;
                Ok(())
            })
            .unwrap();
        assert!(f.send("주간 회의", "- 본문").is_err());
        assert_eq!(f.store.get(&f.id).unwrap().status, Status::Archived);
        assert!(stub.requests.lock().unwrap().is_empty());
    }
}
