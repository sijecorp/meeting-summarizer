//! Slack (architecture 7, SND-01..06, ONB-04): signing in, the recorder's channels, the message,
//! its length (REV-02), the edit count (SND-03) and the post. Nothing posts but `send_to_slack`,
//! the review screen's button (SND-05).
//!
//! Pure and tested without a server: `markdown` (header and length), `payload`, `sections` (the
//! mrkdwn fallback), `edit_count` and `classify`. The requests are tested against local stub
//! servers.

use crate::settings::{self, BUILTIN, BUILTIN_LABEL, Settings, SettingsStore};
use crate::store::{self, Meta, Sent, Status, Store, can_transition, one_line};
use crate::summarize::{cut, pack, split_title};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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

/// The footer (SND-03, plan 5.8 H3): the edit count, then for the pilot's tally the grounding
/// check, "미정 처리 M곳" when it replaced any or "근거 검사 못 함" when it could not read the action
/// items, and where the summary was made.
fn footer(edits: u32, meta: &Meta, settings: &Settings) -> String {
    let mut parts = vec![format!("수정 {edits}곳")];
    match (meta.grounding_checked, meta.grounding_replaced) {
        (Some(false), _) => parts.push("근거 검사 못 함".into()),
        (_, 0) => {}
        (_, n) => parts.push(format!("미정 처리 {n}곳")),
    }
    parts.extend(route(meta, settings).map(|r| format!("{r} 요약")));
    format!("AI 요약 · 녹음자 검토 완료 ({})", parts.join(" · "))
}

/// Where the summary was made (DX9): 이 PC, the cloud or a local server, as it ran
/// (`summarize_local`). A local one by its connection's port first (Ollama 11434, LM Studio 1234),
/// then by its name, while that connection still exists; else "로컬 LLM". Nothing when no LLM was
/// asked.
fn route(meta: &Meta, settings: &Settings) -> Option<&'static str> {
    let name = meta.llm_connection.as_deref()?;
    if name == BUILTIN {
        return Some(BUILTIN_LABEL);
    }
    if !meta.summarize_local {
        return Some("클라우드");
    }
    let connection = settings.connections().into_iter().find(|c| c.name == name);
    let server = connection.and_then(|c| {
        let port = reqwest::Url::parse(c.base_url.trim())
            .ok()
            .and_then(|u| u.port());
        let name = c.name.to_lowercase().replace(' ', "");
        match port {
            Some(11434) => Some("Ollama"),
            Some(1234) => Some("LM Studio"),
            _ if name.contains("ollama") => Some("Ollama"),
            _ if name.contains("lmstudio") => Some("LM Studio"),
            _ => None,
        }
    });
    Some(server.unwrap_or("로컬 LLM"))
}

/// The message's `text` and `blocks` (7.1). `fallback`: `section` blocks in mrkdwn instead of the
/// `markdown` block [S5].
fn payload(title: &str, text: &str, footer: &str, fallback: bool) -> Value {
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
        "elements": [{ "type": "mrkdwn", "text": footer }],
    }));
    // The notification preview, in mrkdwn: an LLM-written `<!channel>` must stay text.
    json!({ "text": format!("{} 회의 요약", escape(heading(title))), "blocks": blocks })
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

/// Why a request failed (SND-06). `message` is what the recorder sees; none holds a token.
#[derive(Debug, PartialEq)]
enum Failure {
    /// Not signed in, or the sign-in ended (revoked, expired, scopes changed): sign in again.
    SignedOut,
    /// No connection: the only failure sure to have sent nothing.
    Network(String),
    Timeout,
    /// The request may have gone out, but its answer broke off.
    NoAnswer(String),
    /// An answer that is not Slack's (a redirect, a proxy's or a login page's): maybe posted.
    Unconfirmed(u16),
    /// `channel_not_found`, `is_archived`.
    Gone(String),
    /// `not_in_channel`.
    NotIn,
    /// `restricted_action` and its kind: the workspace forbids it.
    Forbidden(String),
    RateLimited,
    /// `invalid_blocks`: the `markdown` block is refused [S5].
    Blocks(String),
    Other(String),
}

impl Failure {
    fn message(&self) -> String {
        match self {
            Failure::SignedOut => {
                "Slack 로그인이 필요합니다. 설정 → Slack에서 다시 로그인해 주세요".into()
            }
            Failure::Network(e) => {
                format!("Slack에 연결하지 못했습니다. 인터넷 연결을 확인해 주세요: {e}")
            }
            // These three may have posted all the same.
            Failure::Timeout => {
                "Slack 응답 시간 초과입니다. 채널에 메시지가 올라갔는지 확인한 뒤 다시 보내 주세요"
                    .into()
            }
            Failure::NoAnswer(e) => format!(
                "Slack 응답을 받지 못했습니다({e}). 채널에 메시지가 올라갔는지 확인한 뒤 다시 보내 주세요"
            ),
            Failure::Unconfirmed(status) => format!(
                "Slack 응답을 확인하지 못했습니다(HTTP {status}). 채널에 메시지가 올라갔는지 확인한 뒤 다시 보내 주세요"
            ),
            Failure::Gone(code) => {
                format!("채널이 없거나 보관됐습니다({code}). 다른 채널을 골라 주세요")
            }
            Failure::NotIn => {
                "이 채널의 멤버가 아닙니다. Slack에서 채널에 들어간 뒤 다시 보내 주세요".into()
            }
            Failure::Forbidden(code) => format!(
                "Slack이 이 채널로 보내는 것을 막았습니다({code}). Slack 관리자에게 확인해 주세요"
            ),
            Failure::RateLimited => "Slack 전송 한도를 넘었습니다. 잠시 후 다시 보내 주세요".into(),
            Failure::Blocks(detail) | Failure::Other(detail) => format!("Slack 오류: {detail}"),
        }
    }

    /// For a call that posts nothing (signing in, the channels, a token refresh): there is no
    /// channel to check.
    fn elsewhere(&self) -> String {
        match self {
            Failure::Timeout | Failure::NoAnswer(_) | Failure::Unconfirmed(_) => {
                "Slack 응답을 받지 못했습니다. 잠시 후 다시 시도해 주세요".into()
            }
            f => f.message(),
        }
    }
}

/// A code from Slack, shown only as the snake_case word Slack's codes are: anything else could
/// echo what it was sent.
fn code(code: &str) -> &str {
    if (1..=64).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        code
    } else {
        "unknown_error"
    }
}

/// An error code from Slack's answer, as its failure.
fn failure(code_word: &str) -> Failure {
    let code = code(code_word);
    match code {
        "not_authed"
        | "invalid_auth"
        | "account_inactive"
        | "token_revoked"
        | "token_expired"
        | "missing_scope"
        | "no_permission"
        | "invalid_refresh_token" => Failure::SignedOut,
        "channel_not_found" | "is_archived" => Failure::Gone(code.into()),
        "not_in_channel" => Failure::NotIn,
        "ratelimited" => Failure::RateLimited,
        "invalid_blocks" | "invalid_blocks_format" => Failure::Blocks(code.into()),
        _ if code.starts_with("restricted_action") || code == "ekm_access_denied" => {
            Failure::Forbidden(code.into())
        }
        _ => Failure::Other(code.into()),
    }
}

fn network(e: reqwest::Error) -> Failure {
    // The innermost cause says the most (DNS, TLS, refused).
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

/// Slack's answer: its JSON with `ok: true`, or the failure its error code names. Any other answer
/// is not Slack's, so whether it went through is unknown.
fn classify(status: u16, body: &str) -> Result<Value, Failure> {
    if status == 429 {
        return Err(Failure::RateLimited);
    }
    match serde_json::from_str::<Value>(body)
        .ok()
        .filter(|v| v["ok"].is_boolean())
    {
        Some(v) if v["ok"] == true => Ok(v),
        Some(v) => Err(failure(v["error"].as_str().unwrap_or_default())),
        None if status < 400 => Err(Failure::Unconfirmed(status)),
        None => Err(Failure::Other(format!("HTTP {status}"))),
    }
}

/// The company workspace's Slack app (docs/slack-app.md). Not a secret: PKCE stands in for the
/// client secret a desktop app could not keep.
const CLIENT_ID: &str = "5097318913044.12342682223031";
/// The sign-in's loopback redirect, registered with the app (docs/slack-app.md): Slack wants it
/// exactly.
const PORT: u16 = 47815;
const CALLBACK: &str = "/slack/callback";
/// User scopes: the recorder's channels, public and private, and posting as the recorder.
const SCOPES: &str = "channels:read,groups:read,chat:write";
const AUTHORIZE: &str = "https://slack.com/oauth/v2/authorize";
const API: &str = "https://slack.com/api";
/// The recorder has this long to finish in the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
/// One request to the redirect, in all (`answer`).
const ANSWER_TIME: Duration = Duration::from_secs(2);
/// A token this close to its end (seconds) is refreshed first. Slack's last 12 hours.
const REFRESH_MARGIN: i64 = 300;
/// users.conversations pages of 200: 5,000 channels at most.
const PAGES: usize = 25;
/// What the browser tab says when Slack sends it back.
const SIGNED_IN_PAGE: &str = "Slack 로그인을 마쳤습니다. 이 창을 닫고 회의 요약으로 돌아가 주세요.";
const NOT_SIGNED_IN_PAGE: &str =
    "Slack 로그인을 하지 못했습니다. 회의 요약에서 다시 시도해 주세요.";

/// A channel the recorder is in (SND-04), from `users.conversations`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub private: bool,
}

/// The user token and its rotation (keychain, 9.2). PKCE tokens always rotate: 12 hours each,
/// the refresh token once.
#[derive(Serialize, Deserialize, PartialEq)]
struct Tokens {
    access: String,
    refresh: Option<String>,
    /// Unix seconds.
    expires_at: Option<i64>,
}

/// Never the tokens themselves, in a log or a failed test.
impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tokens {{ expires_at: {:?}, .. }}", self.expires_at)
    }
}

/// The tokens in an `oauth.v2.access` answer: a user token's are under `authed_user` when signing
/// in, at the top when refreshing.
fn tokens_of(v: &Value, now: i64) -> Option<Tokens> {
    let t = if v["authed_user"]["access_token"].is_string() {
        &v["authed_user"]
    } else {
        v
    };
    Some(Tokens {
        access: t["access_token"].as_str()?.into(),
        refresh: t["refresh_token"].as_str().map(Into::into),
        expires_at: t["expires_in"].as_i64().map(|secs| now + secs),
    })
}

/// 32 random bytes as base64url: PKCE's verifier and the sign-in's state.
fn random() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("난수를 만들 수 없습니다: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// The redirect's listeners: `localhost` may resolve to either address, so both, when this machine
/// has IPv6. Port 0 (tests) takes a free one, the same on both. Another program on either one is
/// refused: the browser could take the code there.
fn listen(port: u16) -> Result<(Vec<TcpListener>, u16), String> {
    let busy = |e: std::io::Error| {
        format!(
            "Slack 로그인에 쓰는 포트 {port}를 열 수 없습니다. 다른 프로그램이 쓰고 있는지 확인해 주세요: {e}"
        )
    };
    let v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(busy)?;
    let port = v4.local_addr().map_err(|e| e.to_string())?.port();
    let v6 = match TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
        Ok(v6) => Some(v6),
        Err(e) if e.kind() == ErrorKind::AddrInUse => return Err(busy(e)),
        // No IPv6 on this machine.
        Err(_) => None,
    };
    let listeners = [Some(v4), v6]
        .into_iter()
        .flatten()
        .filter(|l| l.set_nonblocking(true).is_ok())
        .collect();
    Ok((listeners, port))
}

/// Waits for the browser's return to the redirect and answers it. The code, or why not. `None`:
/// cancelled.
fn wait_for_code(
    listeners: &[TcpListener],
    state: &str,
    cancel: &AtomicBool,
) -> Option<Result<String, String>> {
    let deadline = Instant::now() + SIGN_IN_TIMEOUT;
    while !cancel.load(Ordering::SeqCst) {
        if Instant::now() > deadline {
            return Some(Err(
                "Slack 로그인을 기다리다 시간이 지났습니다. 다시 로그인해 주세요".into(),
            ));
        }
        for listener in listeners {
            if let Ok((stream, _)) = listener.accept()
                && let Some(result) = answer(stream, state)
            {
                return Some(result);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// One request to the redirect: the callback's code or Slack's refusal, both with our state, or
/// `None` for anything else, like a favicon or another sign-in's callback. A request gets 2 s in all,
/// so a slow one never holds up the wait (or the port) for long.
fn answer(mut stream: TcpStream, state: &str) -> Option<Result<String, String>> {
    let _ = stream.set_nonblocking(false);
    let until = Instant::now() + ANSWER_TIME;
    let mut head = Vec::new();
    let mut buf = [0; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 8192 {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            break;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let head = String::from_utf8_lossy(&head);
    let target = head.split(' ').nth(1).unwrap_or_default();
    let url = reqwest::Url::parse(&format!("http://localhost{target}")).ok();
    let Some(url) = url.filter(|u| u.path() == CALLBACK) else {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return None;
    };
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    let ours = param("state").as_deref() == Some(state);
    let result = match (ours, param("code"), param("error")) {
        (true, Some(code), _) => Some(Ok(code)),
        (true, _, Some(error)) => Some(Err(if error == "access_denied" {
            "Slack에서 로그인을 허용하지 않았습니다".into()
        } else {
            format!("Slack 로그인을 하지 못했습니다({})", code(&error))
        })),
        // Not this sign-in's (an old tab, or a page made to look like Slack's answer): this one
        // goes on waiting.
        _ => None,
    };
    let page = if matches!(result, Some(Ok(_))) {
        SIGNED_IN_PAGE
    } else {
        NOT_SIGNED_IN_PAGE
    };
    let html = format!(
        "<!doctype html><html lang=\"ko\"><meta charset=\"utf-8\"><title>회의 요약</title>\
         <body style=\"margin:0;height:100vh;display:grid;place-items:center;background:#f7f5f0;\
         color:#1f1d1a;font:16px -apple-system,'Apple SD Gothic Neo','Malgun Gothic',sans-serif\">\
         <p>{page}</p></body></html>"
    );
    let _ = stream.write_all(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
            html.len()
        )
        .as_bytes(),
    );
    result
}

/// The keychain's tokens (JSON); tests put a stand-in here.
type Load = Box<dyn Fn() -> Result<Option<String>, String> + Send + Sync>;
/// Writes them, or with `None` deletes them.
type Save = Box<dyn Fn(Option<&str>) -> Result<(), String> + Send + Sync>;

/// Signing in, the channels and sending from the review screen.
pub struct Slack {
    /// Meetings being sent right now: a second click while one is out must not post twice.
    sending: Mutex<HashSet<String>>,
    /// One token read at a time: a refresh token works once, so two refreshes would sign out.
    tokens: tauri::async_runtime::Mutex<()>,
    /// The sign-in waiting for the browser, to stop it.
    signing_in: Mutex<Option<Arc<AtomicBool>>>,
    load: Load,
    save: Save,
    api: String,
    client_id: &'static str,
    port: u16,
    timeout: Duration,
}

impl Default for Slack {
    fn default() -> Self {
        Self {
            sending: Mutex::default(),
            tokens: Default::default(),
            signing_in: Mutex::default(),
            load: Box::new(settings::slack_tokens),
            save: Box::new(settings::set_slack_tokens),
            api: API.into(),
            client_id: CLIENT_ID,
            port: PORT,
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

    /// One Web API call with a form body, never retried: a post may have gone through though its
    /// answer was lost (7.1). The token goes in the header, never in a URL.
    async fn call(
        &self,
        method: &str,
        token: Option<&str>,
        form: &[(&str, &str)],
    ) -> Result<Value, Failure> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(self.timeout)
            // A redirect is an answer of its own (`classify`): a login page's 200 is not a post.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Failure::Other(e.to_string()))?;
        let mut request = http
            .post(format!("{}/{method}", self.api))
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(
                form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(form)
                    .finish(),
            );
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(network)?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        classify(status, &text)
    }

    /// ONB-04: signs the recorder in, in the browser: OAuth with PKCE and a loopback redirect
    /// (docs/slack-app.md). `open` opens the authorize page. The workspace's name, or `None` when
    /// cancelled. A sign-in still waiting is cancelled first.
    pub async fn sign_in(
        &self,
        settings: &SettingsStore,
        open: impl FnOnce(&str) -> Result<(), String>,
    ) -> Result<Option<String>, String> {
        if self.client_id.is_empty() {
            return Err("이 빌드에는 Slack 앱이 설정되지 않았습니다".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(earlier) = self.signing_in.lock().unwrap().replace(cancel.clone()) {
            earlier.store(true, Ordering::SeqCst);
        }
        // The earlier one lets go of the port once it stops: within its 50 ms poll, or after the
        // request it is answering (`ANSWER_TIME`).
        let port = self.port;
        let (listeners, port) = tauri::async_runtime::spawn_blocking(move || {
            let until = Instant::now() + ANSWER_TIME + Duration::from_secs(1);
            loop {
                match listen(port) {
                    Err(_) if Instant::now() < until => {
                        std::thread::sleep(Duration::from_millis(50))
                    }
                    bound => return bound,
                }
            }
        })
        .await
        .map_err(|e| e.to_string())??;
        let (verifier, state) = (random()?, random()?);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let redirect = format!("http://localhost:{port}{CALLBACK}");
        let url = reqwest::Url::parse_with_params(
            AUTHORIZE,
            [
                ("client_id", self.client_id),
                ("user_scope", SCOPES),
                ("redirect_uri", &redirect),
                ("state", &state),
                ("code_challenge", &challenge),
                ("code_challenge_method", "S256"),
            ],
        )
        .map_err(|e| e.to_string())?;
        open(url.as_str())?;
        let waiting = cancel.clone();
        let code = tauri::async_runtime::spawn_blocking(move || {
            wait_for_code(&listeners, &state, &waiting)
        })
        .await
        .map_err(|e| e.to_string())?;
        self.signing_in
            .lock()
            .unwrap()
            .take_if(|c| Arc::ptr_eq(c, &cancel));
        let Some(code) = code.transpose()? else {
            return Ok(None);
        };
        let answer = self
            .call(
                "oauth.v2.access",
                None,
                &[
                    ("client_id", self.client_id),
                    ("code", &code),
                    ("code_verifier", &verifier),
                    ("redirect_uri", &redirect),
                ],
            )
            .await
            .map_err(|f| match f {
                Failure::Other(detail) => format!("Slack 로그인을 마치지 못했습니다({detail})"),
                f => f.elsewhere(),
            })?;
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let tokens = tokens_of(&answer, now).ok_or("Slack이 토큰을 주지 않았습니다")?;
        let team = answer["team"]["name"]
            .as_str()
            .unwrap_or("Slack")
            .to_string();
        let _one = self.tokens.lock().await;
        (self.save)(Some(
            &serde_json::to_string(&tokens).map_err(|e| e.to_string())?,
        ))?;
        settings.set_slack_team(Some(&team))?;
        Ok(Some(team))
    }

    /// Stops a sign-in waiting for the browser.
    pub fn cancel_sign_in(&self) {
        if let Some(cancel) = self.signing_in.lock().unwrap().take() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    /// The token is revoked when Slack can still be told (refreshed first if it has ended), and
    /// leaves this PC either way.
    pub async fn sign_out(&self, settings: &SettingsStore) -> Result<(), String> {
        let _one = self.tokens.lock().await;
        match self.current().await {
            Ok(tokens) => {
                if let Err(e) = self.call("auth.revoke", Some(&tokens.access), &[]).await {
                    log::warn!("Slack 토큰 폐기 실패: {}", e.message());
                }
            }
            Err(e) => log::warn!("Slack 토큰 폐기 전 갱신 실패: {}", e.message()),
        }
        (self.save)(None)?;
        settings.set_slack_team(None)
    }

    /// The stored tokens, refreshed first when the access token is about to end. The new pair
    /// replaces the old at once: the old refresh token no longer works. Under `tokens`.
    async fn current(&self) -> Result<Tokens, Failure> {
        let stored = (self.load)().map_err(Failure::Other)?;
        let tokens = stored
            .and_then(|t| serde_json::from_str::<Tokens>(&t).ok())
            .ok_or(Failure::SignedOut)?;
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let (Some(at), Some(refresh)) = (tokens.expires_at, &tokens.refresh) else {
            return Ok(tokens);
        };
        if at - now > REFRESH_MARGIN {
            return Ok(tokens);
        }
        let answer = self
            .call(
                "oauth.v2.access",
                None,
                &[
                    ("client_id", self.client_id),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh),
                ],
            )
            .await?;
        let fresh = tokens_of(&answer, now).ok_or(Failure::SignedOut)?;
        let saved = serde_json::to_string(&fresh).map_err(|e| e.to_string());
        if let Err(e) = saved.and_then(|t| (self.save)(Some(&t))) {
            // This request still works; the next one asks for a sign-in.
            log::error!("새 Slack 토큰을 키체인에 저장하지 못했습니다: {e}");
        }
        Ok(fresh)
    }

    /// The access token, one read at a time (`current`).
    async fn token(&self, settings: &SettingsStore) -> Result<String, String> {
        let current = {
            let _one = self.tokens.lock().await;
            self.current().await
        };
        match current {
            Ok(tokens) => Ok(tokens.access),
            Err(f) => Err(self.failed(settings, f, false).await),
        }
    }

    /// What a failure says. A sign-in Slack no longer takes (expired, revoked, scopes changed) is
    /// forgotten here too, so every screen offers to sign in again instead of looking signed in.
    /// `posted`: the call might have posted, so the recorder checks the channel.
    async fn failed(&self, settings: &SettingsStore, f: Failure, posted: bool) -> String {
        if f == Failure::SignedOut {
            let _one = self.tokens.lock().await;
            if let Err(e) = (self.save)(None).and_then(|()| settings.set_slack_team(None)) {
                log::warn!("끝난 Slack 로그인을 지우지 못했습니다: {e}");
            }
        }
        if posted { f.message() } else { f.elsewhere() }
    }

    /// SND-04: the channels the recorder is in, by name.
    pub async fn channels(&self, settings: &SettingsStore) -> Result<Vec<Channel>, String> {
        let token = self.token(settings).await?;
        let mut channels = Vec::new();
        let mut cursor = String::new();
        for _ in 0..PAGES {
            let page = self
                .call(
                    "users.conversations",
                    Some(&token),
                    &[
                        ("types", "public_channel,private_channel"),
                        ("exclude_archived", "true"),
                        ("limit", "200"),
                        ("cursor", &cursor),
                    ],
                )
                .await;
            let page = match page {
                Ok(page) => page,
                Err(f) => return Err(self.failed(settings, f, false).await),
            };
            channels.extend(
                page["channels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| {
                        Some(Channel {
                            id: c["id"].as_str()?.into(),
                            name: c["name"].as_str()?.into(),
                            private: c["is_private"].as_bool().unwrap_or(false),
                        })
                    }),
            );
            cursor = page["response_metadata"]["next_cursor"]
                .as_str()
                .unwrap_or_default()
                .into();
            if cursor.is_empty() {
                break;
            }
        }
        channels.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(channels)
    }

    /// `send_to_slack`: saves the edit the way autosave does, so what is sent is what is stored,
    /// then posts it to `channel` as the recorder. Success: `sent` with the channel, time and edit
    /// count, and `channel` becomes `last_channel` (SND-04). Failure: still `review`, with the cause
    /// in `error` for 다시 보내기 (SND-06).
    #[allow(clippy::too_many_arguments)]
    pub async fn send(
        &self,
        store: &Store,
        settings: &SettingsStore,
        id: &str,
        channel: &Channel,
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
            .deliver(store, settings, &meta, channel, body, offset_minutes)
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
                if let Err(e) = settings.set_last_channel(&channel.id) {
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

    /// Between the save and the result: the message, the token, the post.
    async fn deliver(
        &self,
        store: &Store,
        settings: &SettingsStore,
        meta: &Meta,
        channel: &Channel,
        body: &str,
        offset_minutes: i32,
    ) -> Result<Sent, String> {
        let stored = settings;
        let settings = stored.load()?.settings;
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
        let footer = footer(edits, meta, &settings);
        let token = self.token(stored).await?;
        let mut sent = self
            .post(
                &token,
                &channel.id,
                &payload(&meta.title, &text, &footer, false),
            )
            .await;
        // [S5] A refused `markdown` block gets mrkdwn sections, once.
        if matches!(sent, Err(Failure::Blocks(_))) {
            sent = self
                .post(
                    &token,
                    &channel.id,
                    &payload(&meta.title, &text, &footer, true),
                )
                .await;
        }
        if let Err(f) = sent {
            return Err(self.failed(stored, f, true).await);
        }
        Ok(Sent {
            channel: format!("#{}", channel.name),
            at: store::utc(OffsetDateTime::now_utc()),
            edit_count: edits,
        })
    }

    /// `chat.postMessage` with the payload's text and blocks. No link previews: a summary's links
    /// are references, not the message.
    async fn post(&self, token: &str, channel: &str, payload: &Value) -> Result<(), Failure> {
        let text = payload["text"].as_str().unwrap_or_default();
        let blocks = payload["blocks"].to_string();
        self.call(
            "chat.postMessage",
            Some(token),
            &[
                ("channel", channel),
                ("text", text),
                ("blocks", &blocks),
                ("unfurl_links", "false"),
                ("unfurl_media", "false"),
            ],
        )
        .await
        .map(|_| ())
    }
}

/// `summary.generated.md`, or nothing when the meeting has none.
fn generated(store: &Store, id: &str) -> Result<String, String> {
    match std::fs::read_to_string(store.dir_for(id)?.join("summary.generated.md")) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("summary.generated.md을 읽을 수 없습니다: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::LlmConnection;
    use crate::summarize::tests::{Request, Stub, reply, serve};
    use std::collections::HashMap;
    use std::fs;
    use std::sync::mpsc;
    use std::thread;
    use tauri::async_runtime::block_on;

    const TOKEN: &str = "xoxp-secret-token";

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
            grounding_replaced: 0,
            grounding_checked: None,
            wait: None,
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
        let footer = "AI 요약 · 녹음자 검토 완료 (수정 3곳)";
        assert_eq!(
            payload("<주간> 회의", text, footer, false),
            json!({
                "text": "&lt;주간&gt; 회의 회의 요약",
                "blocks": [
                    { "type": "markdown", "text": text },
                    { "type": "context", "elements": [
                        { "type": "mrkdwn", "text": footer }
                    ]}
                ]
            })
        );
        let fallback = payload("<주간> 회의", text, footer, true);
        assert_eq!(
            fallback["blocks"][0],
            json!({ "type": "section", "text": { "type": "mrkdwn",
                "text": "*&lt;주간&gt; 회의*\n2026-10-13 (화) 14:00 · 42분\n\n- 없음" } })
        );
        assert_eq!(fallback["blocks"][1]["type"], "context");
    }

    /// Plan 5.8 H3, DX9: the grounding check and the route next to the edit count.
    #[test]
    fn the_footer_tells_the_grounding_and_where_the_summary_was_made() {
        let local = |name: &str, url: &str| LlmConnection {
            name: name.into(),
            base_url: url.into(),
            model: "m".into(),
            context_tokens: 8192,
            is_local: true,
            timeout_secs: 900,
        };
        let settings = Settings {
            llm_connections: vec![
                local("내 Ollama", "http://gpu-box.lan:8080/v1"),
                local("맥미니", "http://localhost:11434/v1"),
                local("모델 서버", "http://127.0.0.1:1234/v1"),
                local("vLLM", "http://localhost:8000/v1"),
                local("Ollama 대신 LM Studio", "http://localhost:1234/v1"),
                LlmConnection {
                    is_local: false,
                    ..local("회사 클라우드", "https://api.example.com/v1")
                },
            ],
            ..Settings::default()
        };
        let footer = |connection: Option<&str>, local: bool, replaced, checked| {
            let meta = Meta {
                llm_connection: connection.map(String::from),
                summarize_local: local,
                grounding_replaced: replaced,
                grounding_checked: checked,
                ..meta("2026-10-13T05:00:03Z", 0)
            };
            footer(3, &meta, &settings)
        };
        let done = "AI 요약 · 녹음자 검토 완료";
        assert_eq!(
            footer(Some(BUILTIN), true, 1, Some(true)),
            format!("{done} (수정 3곳 · 미정 처리 1곳 · 이 PC 요약)")
        );
        // Nothing replaced: no count. A cloud one, and one deleted since that was not local.
        for name in ["회사 클라우드", "지운 연결"] {
            assert_eq!(
                footer(Some(name), false, 0, Some(true)),
                format!("{done} (수정 3곳 · 클라우드 요약)")
            );
        }
        // A local server by its port first, else by its connection's name.
        for (name, server) in [
            ("맥미니", "Ollama"),
            ("내 Ollama", "Ollama"),
            ("모델 서버", "LM Studio"),
            ("Ollama 대신 LM Studio", "LM Studio"),
            ("vLLM", "로컬 LLM"),
            ("지운 로컬 연결", "로컬 LLM"),
        ] {
            assert_eq!(
                footer(Some(name), true, 2, Some(true)),
                format!("{done} (수정 3곳 · 미정 처리 2곳 · {server} 요약)"),
                "{name}"
            );
        }
        // What ran decides local or cloud: a connection changed since does not.
        assert_eq!(
            footer(Some("회사 클라우드"), true, 0, Some(true)),
            format!("{done} (수정 3곳 · 로컬 LLM 요약)")
        );
        assert_eq!(
            footer(Some("맥미니"), false, 0, Some(true)),
            format!("{done} (수정 3곳 · 클라우드 요약)")
        );
        // The action items could not be read: said, whatever the count.
        assert_eq!(
            footer(Some(BUILTIN), true, 0, Some(false)),
            format!("{done} (수정 3곳 · 근거 검사 못 함 · 이 PC 요약)")
        );
        // No LLM asked, or a summary from before the check: as it always was.
        assert_eq!(footer(None, false, 0, None), format!("{done} (수정 3곳)"));
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
            let sent = payload("<!channel> 회의", &text, "", fallback).to_string();
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
    fn answers_are_classified_and_show_a_bare_code_only() {
        let ok = classify(200, r#"{"ok": true, "ts": "1.2"}"#).unwrap();
        assert_eq!(ok["ts"], "1.2");
        let cases = [
            ("invalid_auth", "SignedOut"),
            ("token_revoked", "SignedOut"),
            ("token_expired", "SignedOut"),
            ("missing_scope", "SignedOut"),
            ("invalid_refresh_token", "SignedOut"),
            ("channel_not_found", "Gone"),
            ("is_archived", "Gone"),
            ("not_in_channel", "NotIn"),
            ("restricted_action", "Forbidden"),
            ("restricted_action_read_only_channel", "Forbidden"),
            ("ratelimited", "RateLimited"),
            ("invalid_blocks", "Blocks"),
            ("msg_too_long", "Other"),
        ];
        for (code, kind) in cases {
            let failure = classify(200, &json!({ "ok": false, "error": code }).to_string());
            let failure = failure.unwrap_err();
            assert!(
                format!("{failure:?}").starts_with(kind),
                "{code}: {failure:?}"
            );
        }
        assert_eq!(classify(429, ""), Err(Failure::RateLimited));
        // Not Slack's answer: a proxy's or a login page's, maybe posted, maybe not.
        for (status, body) in [
            (200, "<html>로그인</html>"),
            (200, ""),
            (302, ""),
            (204, ""),
        ] {
            assert_eq!(
                classify(status, body),
                Err(Failure::Unconfirmed(status)),
                "{status} {body}"
            );
        }
        assert_eq!(
            classify(502, "<html>"),
            Err(Failure::Other("HTTP 502".into()))
        );
        // A code that is not a code is not shown: it could echo what was sent.
        let echo = json!({ "ok": false, "error": format!("bad token {TOKEN}") }).to_string();
        assert_eq!(
            classify(200, &echo),
            Err(Failure::Other("unknown_error".into()))
        );
        assert_eq!(
            classify(200, r#"{"ok": false, "error": "channel_not_found"}"#)
                .unwrap_err()
                .message(),
            "채널이 없거나 보관됐습니다(channel_not_found). 다른 채널을 골라 주세요"
        );
    }

    #[test]
    fn tokens_come_from_the_sign_in_and_the_refresh_answers_alike() {
        let signed_in = json!({ "ok": true, "authed_user": {
            "id": "U1", "access_token": "xoxe.xoxp-1", "refresh_token": "xoxe-1", "expires_in": 43200
        }, "team": { "name": "SIJE" } });
        assert_eq!(
            tokens_of(&signed_in, 1_000),
            Some(Tokens {
                access: "xoxe.xoxp-1".into(),
                refresh: Some("xoxe-1".into()),
                expires_at: Some(44_200)
            })
        );
        let refreshed = json!({ "ok": true, "access_token": "xoxe.xoxp-2", "refresh_token": "xoxe-2", "expires_in": 43200 });
        assert_eq!(tokens_of(&refreshed, 0).unwrap().access, "xoxe.xoxp-2");
        // A token that does not rotate.
        let lasting = json!({ "ok": true, "authed_user": { "access_token": "xoxp-3" } });
        assert_eq!(
            tokens_of(&lasting, 0),
            Some(Tokens {
                access: "xoxp-3".into(),
                refresh: None,
                expires_at: None
            })
        );
        assert_eq!(tokens_of(&json!({ "ok": true }), 0), None);
    }

    fn sent_blocks(r: &Request) -> Vec<String> {
        let blocks: Value = serde_json::from_str(&form(r)["blocks"]).unwrap();
        blocks
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["type"].as_str().unwrap().to_string())
            .collect()
    }

    /// A request's form body.
    fn form(r: &Request) -> HashMap<String, String> {
        form_urlencoded::parse(r.body.as_str().unwrap_or_default().as_bytes())
            .into_owned()
            .collect()
    }

    fn ok(fields: Value) -> Vec<u8> {
        let mut body = json!({ "ok": true });
        body.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        reply("200 OK", &body.to_string())
    }

    fn error(code: &str) -> Vec<u8> {
        reply("200 OK", &json!({ "ok": false, "error": code }).to_string())
    }

    /// A meeting in review with its generated summary, a signed-in workspace, and a `Slack` whose
    /// API is `stub` and whose keychain is `tokens`.
    struct Fixture {
        store: Store,
        settings: SettingsStore,
        slack: Slack,
        tokens: Arc<Mutex<Option<String>>>,
        id: String,
        _tmp: tempfile::TempDir,
    }

    const GENERATED: &str =
        "# 주간 회의\n\n## 결정사항\n- 목요일 배포\n## 액션 아이템\n- [미정] 노트";

    fn fixture(stub: &Stub) -> Fixture {
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
        settings
            .save(Settings {
                recorder_name: "홍길동".into(),
                ..Settings::default()
            })
            .unwrap();
        settings.set_slack_team(Some("SIJE")).unwrap();
        let tokens = Arc::new(Mutex::new(Some(
            json!({ "access": TOKEN, "refresh": "xoxe-1", "expires_at": i64::MAX }).to_string(),
        )));
        let (read, write) = (tokens.clone(), tokens.clone());
        // Never the real keychain.
        let slack = Slack {
            load: Box::new(move || Ok(read.lock().unwrap().clone())),
            save: Box::new(move |t| {
                *write.lock().unwrap() = t.map(String::from);
                Ok(())
            }),
            api: stub.url.clone(),
            client_id: "123.456",
            port: 0,
            timeout: Duration::from_secs(5),
            ..Slack::default()
        };
        Fixture {
            store,
            settings,
            slack,
            tokens,
            id,
            _tmp: tmp,
        }
    }

    fn channel() -> Channel {
        Channel {
            id: "C2".into(),
            name: "개발".into(),
            private: false,
        }
    }

    impl Fixture {
        fn send(&self, title: &str, body: &str) -> Result<Meta, String> {
            block_on(self.slack.send(
                &self.store,
                &self.settings,
                &self.id,
                &channel(),
                title,
                body,
                540,
            ))
        }

        fn stored(&self) -> Tokens {
            serde_json::from_str(self.tokens.lock().unwrap().as_deref().unwrap()).unwrap()
        }
    }

    #[test]
    fn a_send_saves_the_edit_first_then_posts_it_as_the_recorder() {
        let summary = Arc::new(Mutex::new(None::<std::path::PathBuf>));
        let on_disk = Arc::new(Mutex::new(None));
        let (path, seen) = (summary.clone(), on_disk.clone());
        let stub = serve(move |_| {
            let path = path.lock().unwrap().clone().unwrap();
            *seen.lock().unwrap() = fs::read_to_string(path).ok();
            Some(ok(json!({ "ts": "1.2" })))
        });
        let f = fixture(&stub);
        *summary.lock().unwrap() = Some(f.store.dir_for(&f.id).unwrap().join("summary.md"));
        let body = "## 결정사항\n- 금요일 배포\n## 액션 아이템\n- [미정] 노트";

        let meta = f.send("주간\n회의 ", body).unwrap();

        // What went out was already on disk, as the screen showed it.
        assert_eq!(on_disk.lock().unwrap().as_deref(), Some(body));
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
        assert_eq!(
            f.settings.load().unwrap().settings.last_channel.as_deref(),
            Some("C2")
        );

        let requests = stub.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].head.starts_with("post /v1/chat.postmessage "));
        assert!(
            requests[0]
                .head
                .contains(&format!("authorization: bearer {TOKEN}"))
        );
        let sent = form(&requests[0]);
        assert_eq!(sent["channel"], "C2");
        assert_eq!(sent["text"], "주간 회의 회의 요약");
        assert_eq!(
            (sent["unfurl_links"].as_str(), sent["unfurl_media"].as_str()),
            ("false", "false")
        );
        assert_eq!(sent_blocks(&requests[0]), ["markdown", "context"]);
        let blocks: Value = serde_json::from_str(&sent["blocks"]).unwrap();
        assert_eq!(
            blocks[0]["text"],
            format!("**주간 회의**\n2026-10-13 (화) 14:00 · 1시간 5분 · 녹음: 홍길동\n\n{body}")
        );
        assert_eq!(
            blocks[1]["elements"][0]["text"],
            "AI 요약 · 녹음자 검토 완료 (수정 1곳)"
        );
    }

    /// [S5] handled at runtime: the `markdown` block is refused, so mrkdwn sections go, once.
    #[test]
    fn a_refused_markdown_block_gets_sections_once() {
        let stub = serve(|r| {
            Some(if sent_blocks(r)[0] == "markdown" {
                error("invalid_blocks")
            } else {
                ok(json!({}))
            })
        });
        let f = fixture(&stub);

        let meta = f
            .send("주간 회의", "## 결정사항\n- **목요일** 배포")
            .unwrap();

        assert_eq!(meta.status, Status::Sent);
        let requests = stub.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(sent_blocks(&requests[1]), ["section", "context"]);
        let blocks: Value = serde_json::from_str(&form(&requests[1])["blocks"]).unwrap();
        assert_eq!(
            blocks[0]["text"]["text"],
            "*주간 회의*\n2026-10-13 (화) 14:00 · 1시간 5분 · 녹음: 홍길동\n\n*결정사항*\n- *목요일* 배포"
        );
    }

    #[test]
    fn a_failed_send_stays_in_review_with_the_cause_until_a_resend_works() {
        let gone = Arc::new(AtomicBool::new(true));
        let answer = gone.clone();
        let stub = serve(move |_| {
            Some(if answer.load(Ordering::SeqCst) {
                error("channel_not_found")
            } else {
                ok(json!({}))
            })
        });
        let f = fixture(&stub);

        let error = f.send("주간 회의", "- 고친 본문").unwrap_err();
        assert!(error.starts_with("채널이 없거나 보관됐습니다"), "{error}");
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
    }

    #[test]
    fn signed_out_says_where_to_sign_in_and_asks_nothing() {
        let stub = serve(|_| Some(ok(json!({}))));
        let f = fixture(&stub);
        *f.tokens.lock().unwrap() = None;
        let error = f.send("주간 회의", "- 본문").unwrap_err();
        assert_eq!(error, Failure::SignedOut.message());
        assert_eq!(f.store.get(&f.id).unwrap().error, Some(error));
        assert!(
            block_on(f.slack.channels(&f.settings))
                .unwrap_err()
                .contains("설정 → Slack")
        );
        assert!(stub.requests.lock().unwrap().is_empty());
        // The workspace goes too, so every screen offers to sign in again.
        assert_eq!(f.settings.load().unwrap().settings.slack_team, None);
    }

    /// A token Slack revoked (or whose scopes changed) is forgotten, like a refused refresh.
    #[test]
    fn a_revoked_token_is_forgotten_and_a_lasting_one_is_never_refreshed() {
        let stub = serve(|r| {
            Some(if r.head.starts_with("post /v1/users.conversations ") {
                error("token_revoked")
            } else {
                ok(json!({}))
            })
        });
        let f = fixture(&stub);
        // No end and no refresh token: used as it is.
        *f.tokens.lock().unwrap() = Some(json!({ "access": TOKEN }).to_string());
        f.send("주간 회의", "- 본문").unwrap();
        assert_eq!(stub.requests.lock().unwrap().len(), 1);

        assert_eq!(
            block_on(f.slack.channels(&f.settings)).unwrap_err(),
            Failure::SignedOut.message()
        );
        assert_eq!(*f.tokens.lock().unwrap(), None);
        assert_eq!(f.settings.load().unwrap().settings.slack_team, None);
    }

    /// A refresh token works once: the new pair replaces the old before the post, and a token
    /// with time left is not refreshed again.
    #[test]
    fn an_ending_token_is_refreshed_once_and_the_new_pair_kept() {
        let stub = serve(|r| {
            Some(if r.head.starts_with("post /v1/oauth.v2.access ") {
                ok(
                    json!({ "access_token": "xoxe.xoxp-2", "refresh_token": "xoxe-2", "expires_in": 43200 }),
                )
            } else {
                ok(json!({}))
            })
        });
        let f = fixture(&stub);
        let soon = OffsetDateTime::now_utc().unix_timestamp() + 60;
        *f.tokens.lock().unwrap() =
            Some(json!({ "access": TOKEN, "refresh": "xoxe-1", "expires_at": soon }).to_string());

        f.send("주간 회의", "- 본문").unwrap();
        let stored = f.stored();
        assert_eq!(
            (stored.access.as_str(), stored.refresh.as_deref()),
            ("xoxe.xoxp-2", Some("xoxe-2"))
        );
        assert!(stored.expires_at.unwrap() > soon + 40_000);

        let requests = stub.requests.lock().unwrap();
        let refresh = form(&requests[0]);
        assert_eq!(
            (
                refresh["grant_type"].as_str(),
                refresh["refresh_token"].as_str(),
                refresh["client_id"].as_str()
            ),
            ("refresh_token", "xoxe-1", "123.456")
        );
        // No client secret: PKCE (docs/slack-app.md).
        assert!(!refresh.contains_key("client_secret"));
        assert!(
            requests[1]
                .head
                .contains("authorization: bearer xoxe.xoxp-2")
        );
        drop(requests);

        block_on(f.slack.channels(&f.settings)).unwrap();
        let requests = stub.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            requests[2]
                .head
                .starts_with("post /v1/users.conversations ")
        );
    }

    #[test]
    fn a_refused_refresh_signs_out() {
        let stub = serve(|_| Some(error("invalid_refresh_token")));
        let f = fixture(&stub);
        *f.tokens.lock().unwrap() =
            Some(json!({ "access": TOKEN, "refresh": "xoxe-1", "expires_at": 0 }).to_string());
        assert_eq!(
            f.send("주간 회의", "- 본문").unwrap_err(),
            Failure::SignedOut.message()
        );
        assert_eq!(stub.requests.lock().unwrap().len(), 1);
        assert_eq!(*f.tokens.lock().unwrap(), None);
        assert_eq!(f.settings.load().unwrap().settings.slack_team, None);
    }

    #[test]
    fn channels_come_page_by_page_sorted_by_name() {
        let stub = serve(|r| {
            let cursor = form(r).get("cursor").cloned().unwrap_or_default();
            Some(if cursor.is_empty() {
                ok(json!({
                    "channels": [
                        { "id": "C3", "name": "회의록", "is_private": false },
                        { "id": "G1", "name": "경영", "is_private": true },
                    ],
                    "response_metadata": { "next_cursor": "page2" }
                }))
            } else {
                ok(json!({
                    "channels": [{ "id": "C1", "name": "개발", "is_private": false }],
                    "response_metadata": { "next_cursor": "" }
                }))
            })
        });
        let f = fixture(&stub);

        let channels = block_on(f.slack.channels(&f.settings)).unwrap();

        let names: Vec<_> = channels
            .iter()
            .map(|c| (c.name.as_str(), c.private))
            .collect();
        assert_eq!(names, [("개발", false), ("경영", true), ("회의록", false)]);
        let requests = stub.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let first = form(&requests[0]);
        assert_eq!(first["types"], "public_channel,private_channel");
        assert_eq!(first["exclude_archived"], "true");
        assert_eq!(form(&requests[1])["cursor"], "page2");
    }

    /// The browser's side of a sign-in: a favicon first, then each redirect back to it.
    fn browse(authorize: &str, queries: &[fn(&str) -> String]) -> mpsc::Receiver<String> {
        let url = reqwest::Url::parse(authorize).unwrap();
        let param = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .unwrap()
                .1
                .into_owned()
        };
        let redirect = reqwest::Url::parse(&param("redirect_uri")).unwrap();
        let port = redirect.port().unwrap();
        let state = param("state");
        let callbacks = queries
            .iter()
            .map(|query| format!("{}?{}", redirect.path(), query(&state)));
        let paths: Vec<String> = ["/favicon.ico".to_string()]
            .into_iter()
            .chain(callbacks)
            .collect();
        let (page, pages) = mpsc::channel();
        thread::spawn(move || {
            for path in paths {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
                let mut answer = String::new();
                let _ = stream.read_to_string(&mut answer);
                page.send(answer).unwrap();
            }
        });
        pages
    }

    #[test]
    fn a_sign_in_uses_pkce_and_keeps_the_tokens_and_the_workspace() {
        let stub = serve(|_| {
            Some(ok(json!({
                "authed_user": { "id": "U1", "access_token": "xoxe.xoxp-1", "refresh_token": "xoxe-1", "expires_in": 43200 },
                "team": { "id": "T1", "name": "SIJE" }
            })))
        });
        let f = fixture(&stub);
        f.settings.set_slack_team(None).unwrap();
        let opened = Arc::new(Mutex::new(String::new()));
        let mut pages = None;

        let team = block_on(f.slack.sign_in(&f.settings, |url| {
            *opened.lock().unwrap() = url.to_string();
            pages = Some(browse(url, &[|state| format!("code=abc&state={state}")]));
            Ok(())
        }))
        .unwrap();

        assert_eq!(team.as_deref(), Some("SIJE"));
        assert_eq!(
            f.settings.load().unwrap().settings.slack_team.as_deref(),
            Some("SIJE")
        );
        assert_eq!(f.stored().access, "xoxe.xoxp-1");
        let pages = pages.unwrap();
        assert!(pages.recv().unwrap().starts_with("HTTP/1.1 404"));
        assert!(pages.recv().unwrap().contains(SIGNED_IN_PAGE));

        let authorize = reqwest::Url::parse(&opened.lock().unwrap()).unwrap();
        let param = |name: &str| {
            authorize
                .query_pairs()
                .find(|(k, _)| k == name)
                .map(|p| p.1.into_owned())
        };
        assert_eq!(param("client_id").as_deref(), Some("123.456"));
        assert_eq!(param("user_scope").as_deref(), Some(SCOPES));
        assert_eq!(param("code_challenge_method").as_deref(), Some("S256"));
        assert!(param("scope").is_none(), "no bot scopes from a desktop app");
        let exchange = form(&stub.requests.lock().unwrap()[0]);
        assert_eq!(exchange["code"], "abc");
        assert_eq!(exchange["redirect_uri"], param("redirect_uri").unwrap());
        assert!(!exchange.contains_key("client_secret"));
        // The challenge was the verifier's SHA-256.
        let verifier = &exchange["code_verifier"];
        assert_eq!(
            param("code_challenge").unwrap(),
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
        );
    }

    #[test]
    fn a_sign_in_refused_in_slack_keeps_nothing() {
        let stub = serve(|_| Some(ok(json!({}))));
        let f = fixture(&stub);
        f.settings.set_slack_team(None).unwrap();
        *f.tokens.lock().unwrap() = None;
        let mut pages = None;
        let error = block_on(f.slack.sign_in(&f.settings, |url| {
            pages = Some(browse(
                url,
                &[|state| format!("error=access_denied&state={state}")],
            ));
            Ok(())
        }))
        .unwrap_err();
        assert_eq!(error, "Slack에서 로그인을 허용하지 않았습니다");
        let pages = pages.unwrap();
        pages.recv().unwrap();
        assert!(pages.recv().unwrap().contains(NOT_SIGNED_IN_PAGE));
        assert!(stub.requests.lock().unwrap().is_empty());
        assert_eq!(*f.tokens.lock().unwrap(), None);
        assert_eq!(f.settings.load().unwrap().settings.slack_team, None);
    }

    /// An old tab's callback, or a page posing as Slack's answer, cannot end the sign-in: it goes on
    /// waiting for its own.
    #[test]
    fn a_callback_with_another_state_is_turned_away_and_the_sign_in_waits_on() {
        let stub = serve(|_| {
            Some(ok(json!({
                "authed_user": { "access_token": "xoxe.xoxp-1" }, "team": { "name": "SIJE" }
            })))
        });
        let f = fixture(&stub);
        let mut pages = None;
        let team = block_on(f.slack.sign_in(&f.settings, |url| {
            pages = Some(browse(
                url,
                &[
                    |_| "code=stolen&state=other".to_string(),
                    |_| "code=nostate".to_string(),
                    |state| format!("code=abc&state={state}"),
                ],
            ));
            Ok(())
        }))
        .unwrap();
        assert_eq!(team.as_deref(), Some("SIJE"));
        let pages = pages.unwrap();
        pages.recv().unwrap();
        assert!(pages.recv().unwrap().contains(NOT_SIGNED_IN_PAGE));
        assert!(pages.recv().unwrap().contains(NOT_SIGNED_IN_PAGE));
        assert!(pages.recv().unwrap().contains(SIGNED_IN_PAGE));
        let requests = stub.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(form(&requests[0])["code"], "abc");
    }

    /// A request that trickles in gets 2 s in all, then the wait goes on.
    #[test]
    fn a_slow_request_never_holds_the_wait() {
        let (listeners, port) = listen(0).unwrap();
        let cancel = AtomicBool::new(false);
        let slow = thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            for _ in 0..10 {
                if stream.write_all(b"G").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(500));
            }
        });
        thread::scope(|s| {
            let waiting = s.spawn(|| wait_for_code(&listeners, "s", &cancel));
            thread::sleep(Duration::from_millis(3_000));
            cancel.store(true, Ordering::SeqCst);
            let started = Instant::now();
            assert_eq!(waiting.join().unwrap(), None);
            assert!(started.elapsed() < Duration::from_secs(1));
        });
        slow.join().unwrap();
    }

    #[test]
    fn a_cancelled_sign_in_ends_quietly_and_a_build_without_the_app_says_so() {
        let stub = serve(|_| Some(ok(json!({}))));
        let f = fixture(&stub);
        let signing_in = thread::scope(|s| {
            let waiting = s.spawn(|| block_on(f.slack.sign_in(&f.settings, |_| Ok(()))));
            let until = Instant::now() + Duration::from_secs(5);
            while f.slack.signing_in.lock().unwrap().is_none() {
                assert!(Instant::now() < until, "the sign-in never started");
                thread::sleep(Duration::from_millis(10));
            }
            f.slack.cancel_sign_in();
            waiting.join().unwrap()
        });
        assert_eq!(signing_in, Ok(None));

        let unset = Slack {
            client_id: "",
            ..Slack::default()
        };
        assert_eq!(
            block_on(unset.sign_in(&f.settings, |_| panic!("no browser without an app"))),
            Err("이 빌드에는 Slack 앱이 설정되지 않았습니다".into())
        );
    }

    #[test]
    fn signing_out_revokes_and_forgets_the_token_and_the_workspace() {
        let stub = serve(|r| {
            Some(if r.head.starts_with("post /v1/oauth.v2.access ") {
                ok(
                    json!({ "access_token": "xoxe.xoxp-2", "refresh_token": "xoxe-2", "expires_in": 43200 }),
                )
            } else {
                ok(json!({ "revoked": true }))
            })
        });
        let f = fixture(&stub);
        f.settings.set_last_channel("C2").unwrap();

        block_on(f.slack.sign_out(&f.settings)).unwrap();

        let requests = stub.requests.lock().unwrap();
        assert!(requests[0].head.starts_with("post /v1/auth.revoke "));
        assert!(
            requests[0]
                .head
                .contains(&format!("authorization: bearer {TOKEN}"))
        );
        assert_eq!(*f.tokens.lock().unwrap(), None);
        let settings = f.settings.load().unwrap().settings;
        assert_eq!((settings.slack_team, settings.last_channel), (None, None));
        drop(requests);

        // An ended token is refreshed first, so Slack can still be told.
        f.settings.set_slack_team(Some("SIJE")).unwrap();
        *f.tokens.lock().unwrap() =
            Some(json!({ "access": TOKEN, "refresh": "xoxe-1", "expires_at": 0 }).to_string());
        block_on(f.slack.sign_out(&f.settings)).unwrap();
        let requests = stub.requests.lock().unwrap();
        assert!(requests[1].head.starts_with("post /v1/oauth.v2.access "));
        assert!(
            requests[2]
                .head
                .contains("authorization: bearer xoxe.xoxp-2")
        );
        assert_eq!(*f.tokens.lock().unwrap(), None);
    }

    #[test]
    fn a_silent_server_times_out_once_and_a_dropped_one_is_not_retried() {
        let silent = serve(|_| None);
        let mut f = fixture(&silent);
        f.slack.timeout = Duration::from_secs(1);
        let started = Instant::now();
        assert_eq!(
            f.send("주간 회의", "- 본문").unwrap_err(),
            Failure::Timeout.message()
        );
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(silent.connections.load(Ordering::SeqCst), 1);
        // Reading the channels posts nothing: no channel to check.
        assert_eq!(
            block_on(f.slack.channels(&f.settings)).unwrap_err(),
            "Slack 응답을 받지 못했습니다. 잠시 후 다시 시도해 주세요"
        );

        // Reads the request and hangs up without answering: it may have posted.
        let dropped = serve(|_| Some(Vec::new()));
        let f = fixture(&dropped);
        let error = f.send("주간 회의", "- 본문").unwrap_err();
        assert!(
            error.contains("채널에 메시지가 올라갔는지 확인한 뒤"),
            "{error}"
        );
        assert!(!error.contains(TOKEN), "{error}");
        thread::sleep(Duration::from_millis(100));
        assert_eq!(dropped.connections.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_refused_connection_says_nothing_went_out() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let stub = serve(|_| None);
        let mut f = fixture(&stub);
        f.slack.api = format!("http://127.0.0.1:{port}/api");
        let error = f.send("주간 회의", "- 본문").unwrap_err();
        assert!(error.starts_with("Slack에 연결하지 못했습니다"), "{error}");
        assert!(!error.contains(TOKEN), "{error}");
    }

    /// A proxy or an SSO gateway in the way: its redirect is not followed, and neither it nor a
    /// login page's 200 counts as sent or takes the mrkdwn fallback.
    #[test]
    fn a_redirect_or_a_page_that_is_not_slacks_stays_in_review_after_one_request() {
        let redirect = serve(|_| {
            Some(
                b"HTTP/1.1 302 Found\r\nLocation: /login\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec(),
            )
        });
        let login = serve(|_| Some(reply("200 OK", "<html><title>로그인</title></html>")));
        for (stub, status) in [(redirect, 302), (login, 200)] {
            let f = fixture(&stub);
            let error = f.send("주간 회의", "- 본문").unwrap_err();
            assert_eq!(error, Failure::Unconfirmed(status).message());
            let meta = f.store.get(&f.id).unwrap();
            assert_eq!(
                (meta.status, meta.error.as_deref()),
                (Status::Review, Some(error.as_str()))
            );
            assert_eq!(stub.connections.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn exactly_12000_characters_go_and_one_more_does_not() {
        let stub = serve(|_| Some(ok(json!({}))));
        let f = fixture(&stub);
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
        let blocks: Value = serde_json::from_str(&form(&requests[0])["blocks"]).unwrap();
        // The footer is a block of its own and does not count.
        assert_eq!(
            blocks[0]["text"].as_str().unwrap().chars().count(),
            MAX_CHARS
        );
    }

    #[test]
    fn a_second_click_while_sending_is_refused() {
        let (release, wait) = mpsc::channel::<()>();
        let wait = Mutex::new(wait);
        let stub = serve(move |_| {
            let _ = wait.lock().unwrap().recv();
            Some(ok(json!({})))
        });
        let f = fixture(&stub);
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
        let stub = serve(|_| Some(ok(json!({}))));
        let f = fixture(&stub);
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
