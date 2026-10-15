//! Summaries (architecture 6, SUM-01..10): one OpenAI-compatible Chat Completions client for cloud
//! providers and local Ollama / LM Studio alike. A long meeting is summarized in parts that are then
//! merged, every request within its token budget (6.3).
//!
//! Pure and tested without a server: `route` (which connection), `split_title`, `too_short` and the
//! budget (`tokens`, `pack`). The requests are tested against local stub servers.

use crate::settings::{LlmConnection, Settings};
use crate::store::{Meta, Status};
use crate::transcribe::engine::Segment;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Architecture 6.1. The whole answer has the connection's `timeout_secs`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The answer room of a partial summary, and the most a final summary gets (6.3).
const PART_TOKENS: u64 = 1500;
const FINAL_TOKENS: u64 = 4000;
/// What a reasoning model may spend thinking. It counts against `max_completion_tokens`, so the
/// compatible request (6.1) adds it to the answer room.
const REASONING_TOKENS: u64 = 8000;
/// SUM-10: below this many characters (whitespace aside) the LLM is not asked at all.
const MIN_CHARS: usize = 30;

/// Architecture 6.2. The glossary line is added only when there is a glossary.
const SYSTEM: &str = "\
너는 회의록 작성자다. 한국어 회의 전사본을 읽고, 회의에 없던 사람이 읽을 요약을 쓴다.
출력은 아래 Markdown 형식만 쓴다.

# {회의 제목: 20자 이내}
## 결정사항
- ...
## 액션 아이템
- [담당자] 할 일 (기한)
## 주요 논의
- ...
## 미해결 질문
- ...

규칙:
- 전사본에는 말한 사람 표시가 없다. 전사본에 명시되지 않은 담당자나 기한은 \"미정\"으로 쓴다. 추정하지 않는다.
- 해당 내용이 없는 섹션은 \"- 없음\"으로 쓴다.
- 결정사항에는 회의에서 정한 것만 쓴다. 기각된 제안과 \"~하지 않기로\" 한 것은 결정사항이 아니다. 필요하면 주요 논의에 쓴다.
- 회의 중에 결정이 바뀌었으면 마지막 상태만 결정사항에 쓴다.
- 전사 오류로 보이는 단어는 문맥상 확실할 때만 고친다.";

/// What comes before the content in the user message.
const WHOLE: &str = "회의 전사본이다.";
const PART: &str = "회의 전사본을 앞에서부터 나눈 한 구간이다. 이 구간만 같은 형식으로 요약한다.";
const MERGE: &str = "한 회의를 앞에서부터 차례로 나눠 만든 부분 요약들이다(구분선 ---). 같은 형식의 요약 하나로 합친다. 겹치는 내용은 한 번만 쓰고, 앞에서 정한 것이 뒤에서 바뀌었으면 마지막 상태만 결정사항에 쓴다.";
const SEPARATOR: &str = "\n\n---\n\n";

/// Why a summary failed (architecture 6.4). `message` is what the recorder sees.
#[derive(Debug, PartialEq)]
pub enum Failure {
    /// A cloud connection with no key in the keychain, found before any request.
    NoKey,
    /// Could not connect or the connection broke.
    Network(String),
    Timeout,
    /// A local connection refused: Ollama or LM Studio is not running.
    LocalDown,
    /// 401 or 403.
    Key(u16),
    /// 429.
    Quota,
    /// Any other HTTP error, or an answer that is not a summary.
    Provider(String),
    /// Even one part of the meeting does not fit a request (6.3).
    TooSmall,
    /// The server read far fewer tokens than it was sent: it cut the input to its own context
    /// (Ollama's `/v1` and LM Studio ignore ours).
    Truncated,
    /// One partial summary of a long meeting failed (6.3): the whole summary fails with it.
    Part {
        n: usize,
        of: usize,
        cause: Box<Failure>,
    },
}

impl Failure {
    pub fn message(&self) -> String {
        match self {
            Failure::NoKey => {
                "API 키가 설정되지 않았습니다. 설정 → 요약(LLM)에서 API 키를 입력해 주세요".into()
            }
            Failure::Network(e) => {
                format!("LLM 서버에 연결하지 못했습니다. 인터넷 연결을 확인해 주세요: {e}")
            }
            Failure::Timeout => {
                "LLM 응답 시간 초과입니다. 연결을 확인하고 다시 실행해 주세요".into()
            }
            Failure::LocalDown => {
                "로컬 LLM에 연결할 수 없습니다. Ollama·LM Studio가 실행 중인지 확인해 주세요".into()
            }
            Failure::Key(status) => format!(
                "API 키가 거부됐습니다(HTTP {status}). 설정 → 요약(LLM)에서 API 키를 확인해 주세요"
            ),
            Failure::Quota => "LLM 사용 한도를 넘었습니다(HTTP 429). 잠시 후 다시 시도해 주세요".into(),
            Failure::Provider(detail) => format!("LLM 공급자 오류: {detail}"),
            Failure::TooSmall => "컨텍스트 토큰 설정이 너무 작습니다. 설정 → 요약(LLM)에서 모델의 실제 컨텍스트 길이를 입력해 주세요".into(),
            Failure::Truncated => "LLM 서버가 입력을 잘랐습니다. 서버의 컨텍스트 길이(Ollama: OLLAMA_CONTEXT_LENGTH, LM Studio: 모델 로드 시 Context Length)를 연결 설정의 컨텍스트 토큰 이상으로 맞춰 주세요".into(),
            Failure::Part { n, of, cause } => format!("{} (구간 {n}/{of}에서 실패)", cause.message()),
        }
    }

    fn at(self, n: usize, of: usize) -> Self {
        Failure::Part {
            n,
            of,
            cause: Box::new(self),
        }
    }
}

/// Which connection summarizes a meeting (SUM-08, architecture 6.1). A choice already made (a
/// retry's, or an earlier start's) stands. "로컬로 요약" never falls back to the cloud: the recorder
/// asked to keep the transcript on this machine.
pub fn route<'a>(settings: &'a Settings, meta: &Meta) -> Result<&'a LlmConnection, String> {
    let all = &settings.llm_connections;
    if all.is_empty() {
        return Err("LLM 연결이 없습니다. 설정에서 추가한 뒤 다시 실행해 주세요".into());
    }
    let named = |name: &str| all.iter().find(|c| c.name == name);
    if let Some(name) = &meta.llm_connection {
        return named(name).ok_or_else(|| format!("LLM 연결 '{name}'을(를) 찾을 수 없습니다"));
    }
    let default = settings.default_llm.as_deref().and_then(named);
    if meta.summarize_local {
        return default
            .filter(|c| c.is_local)
            .or_else(|| all.iter().find(|c| c.is_local))
            .ok_or_else(|| "로컬 LLM 연결이 없습니다".into());
    }
    Ok(default.unwrap_or(&all[0]))
}

/// `summary_route`: where a meeting's summary goes, for the SUM-07 notice and the SUM-08 toggle.
#[derive(Serialize, Debug, PartialEq)]
pub struct SummaryRoute {
    /// `None` without connections, when the meeting's choice cannot be met, or when nothing was
    /// sent (no speech, or reviewed without a connection). Holds no secret.
    pub connection: Option<LlmConnection>,
    /// The meeting's "로컬로 요약".
    pub local: bool,
    /// Some connection is local, so the toggle is offered.
    pub local_available: bool,
    /// Summarizing has started: the route no longer changes.
    pub locked: bool,
}

impl SummaryRoute {
    pub fn of(settings: &Settings, meta: &Meta) -> Self {
        // Nothing went anywhere: SUM-07 must not say "전송됨".
        let sent_nothing = meta.no_speech
            || (meta.llm_connection.is_none()
                && matches!(
                    meta.status,
                    Status::Review | Status::Sent | Status::Archived
                ));
        Self {
            connection: (!sent_nothing)
                .then(|| route(settings, meta).ok().cloned())
                .flatten(),
            local: meta.summarize_local,
            local_available: settings.llm_connections.iter().any(|c| c.is_local),
            locked: meta.summary_started(),
        }
    }
}

/// SUM-10: the filtered transcript is under 30 characters, whitespace aside.
pub fn too_short(lines: &[String]) -> bool {
    lines
        .iter()
        .flat_map(|l| l.chars())
        .filter(|c| !c.is_whitespace())
        .count()
        < MIN_CHARS
}

/// The first non-empty line, when it is `# …`, is the title; the rest is the body.
pub fn split_title(markdown: &str) -> (Option<String>, String) {
    let text = markdown.trim_start();
    let Some(rest) = text.strip_prefix("# ") else {
        return (None, text.to_string());
    };
    let (title, body) = rest.split_once('\n').unwrap_or((rest, ""));
    let body: Vec<&str> = body.lines().skip_while(|l| l.trim().is_empty()).collect();
    (Some(title.trim().to_string()), body.join("\n"))
}

/// REV-05 `.md` file: the summary, then the transcript with `[mm:ss]` times.
pub fn markdown(title: &str, summary: &str, segments: &[Segment]) -> String {
    let mut text = format!("# {title}\n\n{}\n\n---\n\n## 전사본\n\n", summary.trim());
    for s in segments {
        let secs = s.start_ms / 1000;
        text += &format!("[{:02}:{:02}] {}\n", secs / 60, secs % 60, s.text.trim());
    }
    text
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

/// Architecture 6.3: one token per character. Korean runs about 0.6–1.0 tokens a character on
/// common tokenizers, so this is the safe side.
fn tokens(chars: usize) -> u64 {
    chars as u64
}

fn message(intro: &str, content: &str) -> String {
    format!("{intro}\n\n{content}")
}

/// Greedy: items into packs that, joined by `sep`, hold at most `room` characters, in order. An
/// item longer than `room` is a pack of its own; callers cut or reject such items first.
fn pack(items: Vec<String>, sep: &str, room: usize) -> Vec<Vec<String>> {
    let mut packs: Vec<Vec<String>> = Vec::new();
    let mut len = 0;
    for item in items {
        let n = chars(&item);
        match packs.last_mut() {
            Some(pack) if len + chars(sep) + n <= room => {
                pack.push(item);
                len += chars(sep) + n;
            }
            _ => {
                packs.push(vec![item]);
                len = n;
            }
        }
    }
    packs
}

/// A line longer than `room` cut by characters, so a part never overflows (6.3).
fn cut(line: &str, room: usize) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    chars.chunks(room).map(|c| c.iter().collect()).collect()
}

/// Requests that never go through a proxy: a local connection's (SUM-08 keeps the transcript off
/// the network), and any to a loopback address. With `system-proxy` they would otherwise reach a
/// company proxy in cleartext: hyper-util ignores the macOS proxy exceptions, and Windows'
/// `<local>` does not match `localhost`.
fn direct(connection: &LlmConnection) -> bool {
    let loopback = reqwest::Url::parse(connection.base_url.trim())
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some_and(|host| {
            host == "localhost"
                || host.ends_with(".localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
    connection.is_local || loopback
}

/// The server rejected `max_tokens` or `temperature`: OpenAI's reasoning models do (6.1).
fn wants_compat(body: &str) -> bool {
    let body = body.to_lowercase();
    body.contains("max_completion_tokens")
        || ((body.contains("max_tokens") || body.contains("temperature"))
            && (body.contains("unsupported") || body.contains("not supported")))
}

/// Nothing listens on the port: for a local connection, the LLM app is not running.
fn refused(e: &reqwest::Error) -> bool {
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        if s.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused)
        {
            return true;
        }
        source = s.source();
    }
    false
}

/// A short part of an error body for the recorder: 200 characters at most, and never our key.
fn excerpt(body: &str, key: Option<&str>) -> String {
    let body = key.map_or_else(|| body.to_string(), |key| body.replace(key, "***"));
    let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut short: String = body.chars().take(200).collect();
    if chars(&body) > 200 {
        short.push('…');
    }
    short
}

/// A local reasoning model's thinking is not part of the answer: everything up to the last
/// `</think>`. Its chat template may open `<think>` itself (R1, QwQ), so the content can start
/// with the thinking and no tag. An unclosed `<think>` is thinking only.
fn strip_think(text: &str) -> &str {
    match text.rsplit_once("</think>") {
        Some((_, answer)) => answer.trim(),
        None if text.trim_start().starts_with("<think>") => "",
        None => text.trim(),
    }
}

/// The answer of a Chat Completions reply to `input_chars` characters of input. A cut input, a
/// cut answer (`finish_reason: "length"`) and an empty one are failures, not summaries.
fn content(body: &str, key: Option<&str>, input_chars: usize) -> Result<String, Failure> {
    #[derive(Deserialize)]
    struct Reply {
        choices: Vec<Choice>,
        usage: Option<Usage>,
    }
    #[derive(Deserialize)]
    struct Usage {
        prompt_tokens: Option<u64>,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: Message,
        finish_reason: Option<String>,
    }
    #[derive(Deserialize)]
    struct Message {
        content: Option<String>,
    }
    let unreadable =
        || Failure::Provider(format!("응답을 해석할 수 없습니다: {}", excerpt(body, key)));
    let reply: Reply = serde_json::from_str(body).map_err(|_| unreadable())?;
    // Even English is about 4 characters a token, so fewer than that were never read.
    let read = reply.usage.and_then(|u| u.prompt_tokens);
    if read.is_some_and(|tokens| tokens < input_chars as u64 / 4) {
        return Err(Failure::Truncated);
    }
    let choice = reply.choices.into_iter().next().ok_or_else(unreadable)?;
    if choice.finish_reason.as_deref() == Some("length") {
        return Err(Failure::Provider(
            "요약이 길이 제한에서 잘렸습니다. 컨텍스트 토큰 설정을 확인해 주세요".into(),
        ));
    }
    let text = strip_think(choice.message.content.as_deref().unwrap_or_default());
    if text.is_empty() {
        return Err(Failure::Provider(format!(
            "빈 요약을 받았습니다(finish_reason: {})",
            choice.finish_reason.as_deref().unwrap_or("없음")
        )));
    }
    Ok(text.to_string())
}

/// One summary's requests to one connection.
struct Llm<'a> {
    http: reqwest::Client,
    url: String,
    connection: &'a LlmConnection,
    key: Option<&'a str>,
    /// The server takes `max_completion_tokens` and no `temperature`: OpenAI's reasoning models
    /// reject `max_tokens` and a non-default temperature. Learned from its first 400, then kept.
    compat: bool,
}

impl<'a> Llm<'a> {
    fn new(connection: &'a LlmConnection, key: Option<&'a str>) -> Result<Self, Failure> {
        let mut http = reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT);
        if direct(connection) {
            http = http.no_proxy();
        }
        let http = http.build().map_err(|e| Failure::Network(e.to_string()))?;
        let base = connection.base_url.trim().trim_end_matches('/');
        Ok(Self {
            http,
            url: format!("{base}/chat/completions"),
            connection,
            key,
            compat: false,
        })
    }

    /// A network failure (a timeout too) is tried once more; nothing else is (6.4).
    async fn ask(&mut self, system: &str, user: &str, max_tokens: u64) -> Result<String, Failure> {
        match self.send(system, user, max_tokens).await {
            Err(Failure::Network(_) | Failure::Timeout) => {
                self.send(system, user, max_tokens).await
            }
            result => result,
        }
    }

    async fn send(&mut self, system: &str, user: &str, max_tokens: u64) -> Result<String, Failure> {
        loop {
            let mut body = serde_json::json!({
                "model": self.connection.model,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user },
                ],
            });
            if self.compat {
                body["max_completion_tokens"] = (max_tokens + REASONING_TOKENS).into();
            } else {
                body["max_tokens"] = max_tokens.into();
                body["temperature"] = 0.2.into();
            }
            let mut request = self
                .http
                .post(&self.url)
                .timeout(Duration::from_secs(self.connection.timeout_secs.into()))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_string());
            // Local servers need no key; a header with nothing in it is never sent.
            if let Some(key) = self.key {
                request = request.bearer_auth(key);
            }
            let response = request.send().await.map_err(|e| self.network(e))?;
            let status = response.status();
            let text = response.text().await.map_err(|e| self.network(e))?;
            if status == reqwest::StatusCode::BAD_REQUEST && !self.compat && wants_compat(&text) {
                self.compat = true;
                continue;
            }
            return match status.as_u16() {
                200..=299 => content(&text, self.key, chars(system) + chars(user)),
                code @ (401 | 403) => Err(Failure::Key(code)),
                429 => Err(Failure::Quota),
                code => Err(Failure::Provider(format!(
                    "HTTP {code}: {}",
                    excerpt(&text, self.key)
                ))),
            };
        }
    }

    fn network(&self, e: reqwest::Error) -> Failure {
        if e.is_timeout() {
            Failure::Timeout
        } else if self.connection.is_local && refused(&e) {
            Failure::LocalDown
        } else {
            Failure::Network(e.to_string())
        }
    }
}

/// Summarizes `lines` (the transcript, one segment per line) with `connection` (architecture 6).
/// A transcript over 60% of the context goes in parts whose partial summaries are merged, in
/// groups again while one request cannot hold them all (6.3). `on_step(done, known)` counts
/// requests: `known` grows when a merge needs another pass.
pub async fn summarize(
    connection: &LlmConnection,
    key: Option<String>,
    glossary: &[String],
    lines: &[String],
    on_step: impl Fn(u32, u32),
) -> Result<String, Failure> {
    let key = key.filter(|k| !k.is_empty());
    if !connection.is_local && key.is_none() {
        return Err(Failure::NoKey);
    }
    let mut llm = Llm::new(connection, key.as_deref())?;
    let mut system = SYSTEM.to_string();
    if !glossary.is_empty() {
        system += &format!("\n- 용어집의 표기를 따른다: {}", glossary.join(", "));
    }
    let context = u64::from(connection.context_tokens);
    let budget = context * 9 / 10;
    let input = |user: &str| tokens(chars(&system) + chars(user));
    let within = |user: &str| input(user) <= context * 6 / 10;
    let last = |user: &str| FINAL_TOKENS.min(budget.saturating_sub(input(user)));
    // Characters of content a request may carry with `answer` tokens left for the answer: every
    // request keeps `input + answer ≤ budget`.
    let room = |intro: &str, answer: u64| {
        (budget.saturating_sub(answer) as usize)
            .saturating_sub(chars(&system) + chars(&message(intro, "")))
    };

    let whole = message(WHOLE, &lines.join("\n"));
    if within(&whole) {
        on_step(0, 1);
        let summary = llm.ask(&system, &whole, last(&whole)).await?;
        on_step(1, 1);
        return Ok(summary);
    }
    let (part_room, merge_room) = (room(PART, PART_TOKENS), room(MERGE, PART_TOKENS));
    // A request must hold one partial summary at its full length, or merging can never fit.
    if merge_room < PART_TOKENS as usize {
        return Err(Failure::TooSmall);
    }
    let pieces = lines.iter().flat_map(|l| cut(l, part_room)).collect();
    let parts: Vec<String> = pack(pieces, "\n", part_room)
        .into_iter()
        .map(|p| p.join("\n"))
        .collect();
    let (mut done, mut known) = (0, parts.len() as u32 + 1);
    on_step(done, known);
    let mut summaries = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let summary = llm.ask(&system, &message(PART, part), PART_TOKENS).await;
        summaries.push(summary.map_err(|e| e.at(i + 1, parts.len()))?);
        done += 1;
        on_step(done, known);
    }
    loop {
        let merged = message(MERGE, &summaries.join(SEPARATOR));
        // Within 60%, or one request holds them all: another pass would only hand the final
        // merge a single summary of them.
        if within(&merged) || input(&merged) + PART_TOKENS <= budget {
            let summary = llm.ask(&system, &merged, last(&merged)).await?;
            on_step(known, known);
            return Ok(summary);
        }
        // Merge neighbours into partial summaries again. Each pass must shrink their number, so
        // this ends.
        let groups = pack(summaries.clone(), SEPARATOR, merge_room);
        if groups.len() >= summaries.len() || summaries.iter().any(|s| chars(s) > merge_room) {
            return Err(Failure::TooSmall);
        }
        // A group of one stays as it is: summarizing one summary again only loses detail.
        let merging = groups.iter().filter(|g| g.len() > 1).count();
        known += merging as u32;
        on_step(done, known);
        summaries.clear();
        let mut n = 0;
        for mut group in groups {
            if group.len() == 1 {
                summaries.append(&mut group);
                continue;
            }
            n += 1;
            let merged = message(MERGE, &group.join(SEPARATOR));
            let summary = llm.ask(&system, &merged, PART_TOKENS).await;
            summaries.push(summary.map_err(|e| e.at(n, merging))?);
            done += 1;
            on_step(done, known);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::Status;
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Instant;
    use tauri::async_runtime::block_on;

    #[derive(Clone)]
    pub(crate) struct Request {
        /// Lowercase.
        pub head: String,
        pub body: serde_json::Value,
    }

    /// A local OpenAI-compatible server.
    pub(crate) struct Stub {
        pub url: String,
        pub requests: Arc<Mutex<Vec<Request>>>,
        pub connections: Arc<AtomicUsize>,
    }

    /// Starts a stub: `answer` gets each request and returns the response bytes, or `None` to hold
    /// the connection without ever answering. Each request is recorded before it is answered, on a
    /// connection of its own. It also serves as an HTTP proxy: the head then names the full URL.
    pub(crate) fn serve(
        answer: impl Fn(&Request) -> Option<Vec<u8>> + Send + Sync + 'static,
    ) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let (seen, count, answer) = (requests.clone(), connections.clone(), Arc::new(answer));
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                count.fetch_add(1, Ordering::SeqCst);
                let (seen, answer) = (seen.clone(), answer.clone());
                thread::spawn(move || {
                    let mut data = Vec::new();
                    let mut buf = [0; 4096];
                    let head_end = loop {
                        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                            break at + 4;
                        }
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => data.extend_from_slice(&buf[..n]),
                        }
                    };
                    let head = String::from_utf8_lossy(&data[..head_end]).to_lowercase();
                    let length: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map_or(0, |v| v.trim().parse().unwrap());
                    while data.len() < head_end + length {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => data.extend_from_slice(&buf[..n]),
                        }
                    }
                    let request = Request {
                        head,
                        body: serde_json::from_slice(&data[head_end..]).unwrap(),
                    };
                    seen.lock().unwrap().push(request.clone());
                    match answer(&request) {
                        Some(bytes) => {
                            let _ = stream.write_all(&bytes);
                        }
                        None => thread::sleep(Duration::from_secs(60)),
                    }
                });
            }
        });
        Stub {
            url,
            requests,
            connections,
        }
    }

    pub(crate) fn reply(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    pub(crate) fn completion(content: &str) -> Vec<u8> {
        let body = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }]
        });
        reply("200 OK", &body.to_string())
    }

    pub(crate) const SUMMARY: &str = "# 주간 회의\n\n## 결정사항\n- 목요일에 배포한다\n## 액션 아이템\n- [민수] 릴리스 노트 초안 (금요일)\n## 주요 논의\n- 배포 일정\n## 미해결 질문\n- 없음";

    pub(crate) fn connection(url: &str, name: &str, is_local: bool) -> LlmConnection {
        LlmConnection {
            name: name.into(),
            base_url: url.into(),
            model: "test-model".into(),
            context_tokens: 128_000,
            is_local,
            timeout_secs: 30,
        }
    }

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(String::from).collect()
    }

    fn run(
        connection: &LlmConnection,
        key: Option<&str>,
        lines: &[String],
    ) -> Result<String, Failure> {
        block_on(summarize(
            connection,
            key.map(String::from),
            &[],
            lines,
            |_, _| {},
        ))
    }

    const SPEECH: &str =
        "이번 주 배포는 목요일 오후에 하고 릴리스 노트는 민수 씨가 금요일까지 씁니다";

    #[test]
    fn one_request_with_the_prompt_the_transcript_and_the_key() {
        let stub = serve(|_| Some(completion(SUMMARY)));
        let glossary = ["누리패스".to_string()];
        let summary = block_on(summarize(
            &connection(&format!("{}/", stub.url), "cloud", false),
            Some("sk-test".into()),
            &glossary,
            &lines(&format!("{SPEECH}\n둘째 줄")),
            |_, _| {},
        ))
        .unwrap();
        assert_eq!(summary, SUMMARY);
        let requests = stub.requests.lock().unwrap();
        let request = &requests[0];
        assert!(
            request.head.starts_with("post /v1/chat/completions "),
            "{}",
            request.head
        );
        assert!(request.head.contains("authorization: bearer sk-test"));
        let body = &request.body;
        assert_eq!(
            (&body["model"], &body["temperature"]),
            (&"test-model".into(), &0.2.into())
        );
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("## 액션 아이템") && system.contains("\"미정\""));
        assert!(system.ends_with("용어집의 표기를 따른다: 누리패스"));
        assert_eq!(
            body["messages"][1]["content"],
            format!("{WHOLE}\n\n{SPEECH}\n둘째 줄")
        );
        // Plenty of context: the final answer gets its full 4000 tokens.
        assert_eq!(body["max_tokens"], 4000);
    }

    #[test]
    fn no_glossary_line_without_a_glossary_and_no_key_header_without_a_key() {
        let stub = serve(|_| Some(completion(SUMMARY)));
        run(&connection(&stub.url, "local", true), None, &lines(SPEECH)).unwrap();
        let requests = stub.requests.lock().unwrap();
        assert!(!requests[0].head.contains("authorization"));
        let system = requests[0].body["messages"][0]["content"].as_str().unwrap();
        assert!(!system.contains("용어집"));
    }

    #[test]
    fn a_cloud_connection_without_a_key_sends_nothing() {
        let stub = serve(|_| Some(completion(SUMMARY)));
        for key in [None, Some("")] {
            let result = run(&connection(&stub.url, "cloud", false), key, &lines(SPEECH));
            assert_eq!(result, Err(Failure::NoKey));
        }
        thread::sleep(Duration::from_millis(100));
        assert_eq!(stub.connections.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn http_errors_are_classified_and_not_retried() {
        let cases = [
            ("401 Unauthorized", "{}", Failure::Key(401)),
            ("403 Forbidden", "{}", Failure::Key(403)),
            ("429 Too Many Requests", "{}", Failure::Quota),
        ];
        for (status, body, expected) in cases {
            let stub = serve(move |_| Some(reply(status, body)));
            let result = run(
                &connection(&stub.url, "c", false),
                Some("k"),
                &lines(SPEECH),
            );
            assert_eq!(result, Err(expected), "{status}");
            assert_eq!(stub.connections.load(Ordering::SeqCst), 1);
        }
        // Other errors show a short part of the body, never the key.
        let long = format!(
            "model overloaded, key sk-secret-1 rejected {}",
            "x".repeat(500)
        );
        let stub = serve(move |_| Some(reply("500 Internal Server Error", &long)));
        let Err(Failure::Provider(detail)) = run(
            &connection(&stub.url, "c", false),
            Some("sk-secret-1"),
            &lines(SPEECH),
        ) else {
            panic!("not a provider error");
        };
        assert!(detail.starts_with("HTTP 500: model overloaded, key *** rejected"));
        assert!(!detail.contains("sk-secret-1"));
        assert!(chars(&detail) < 220, "{detail}");
        assert!(
            Failure::Provider(detail)
                .message()
                .starts_with("LLM 공급자 오류")
        );
    }

    #[test]
    fn refused_is_a_local_llm_not_running_but_a_network_error_for_the_cloud() {
        // A port nothing listens on.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let url = format!("http://127.0.0.1:{port}/v1");
        let local = run(&connection(&url, "l", true), None, &lines(SPEECH));
        assert_eq!(local, Err(Failure::LocalDown));
        assert!(local.unwrap_err().message().contains("Ollama·LM Studio"));
        let cloud = run(&connection(&url, "c", false), Some("k"), &lines(SPEECH));
        assert!(matches!(cloud, Err(Failure::Network(_))), "{cloud:?}");
    }

    /// Architecture 6.4: a server that accepts and never answers is a timeout, tried once more.
    #[test]
    fn a_silent_server_times_out_and_is_tried_exactly_once_more() {
        let stub = serve(|_| None);
        let quick = LlmConnection {
            timeout_secs: 1,
            ..connection(&stub.url, "c", false)
        };
        let started = Instant::now();
        let result = run(&quick, Some("k"), &lines(SPEECH));
        assert_eq!(result, Err(Failure::Timeout));
        assert!(result.unwrap_err().message().contains("시간 초과"));
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(stub.connections.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn max_completion_tokens_servers_get_the_compatible_request() {
        let stub = serve(|r| {
            Some(if r.body.get("max_tokens").is_some() {
                reply(
                    "400 Bad Request",
                    r#"{"error":{"message":"Unsupported parameter: 'max_tokens'. Use 'max_completion_tokens' instead."}}"#,
                )
            } else {
                completion(SUMMARY)
            })
        });
        let summary = run(
            &connection(&stub.url, "c", false),
            Some("k"),
            &lines(SPEECH),
        );
        assert_eq!(summary.unwrap(), SUMMARY);
        let requests = stub.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        // Reasoning tokens count against the limit, so the answer room gets them on top.
        let second = &requests[1].body;
        assert_eq!(second["max_completion_tokens"], 4000 + REASONING_TOKENS);
        assert!(second.get("temperature").is_none() && second.get("max_tokens").is_none());
    }

    #[test]
    fn an_unsupported_temperature_also_switches_to_the_compatible_request() {
        let stub = serve(|r| {
            Some(if r.body.get("temperature").is_some() {
                reply(
                    "400 Bad Request",
                    r#"{"error":{"message":"Unsupported value: 'temperature' does not support 0.2 with this model."}}"#,
                )
            } else {
                completion(SUMMARY)
            })
        });
        let summary = run(
            &connection(&stub.url, "c", false),
            Some("k"),
            &lines(SPEECH),
        );
        assert_eq!(summary.unwrap(), SUMMARY);
        assert_eq!(stub.requests.lock().unwrap().len(), 2);
        // Any other 400 is just an error.
        assert!(!wants_compat("Invalid model name"));
        assert!(!wants_compat("temperature must be between 0 and 2"));
    }

    #[test]
    fn a_second_400_after_the_compatible_request_is_an_error() {
        let body = r#"{"error":{"message":"Use 'max_completion_tokens' instead."}}"#;
        let stub = serve(move |_| Some(reply("400 Bad Request", body)));
        let result = run(
            &connection(&stub.url, "c", false),
            Some("k"),
            &lines(SPEECH),
        );
        let Err(Failure::Provider(detail)) = result else {
            panic!("{result:?}");
        };
        assert!(detail.starts_with("HTTP 400"), "{detail}");
        assert_eq!(stub.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn thinking_is_dropped_whether_or_not_the_model_opened_the_tag() {
        let tagged = format!("<think>\n음, 결정을 정리하면…\n</think>\n\n{SUMMARY}");
        assert_eq!(strip_think(&tagged), SUMMARY);
        // The chat template opened `<think>` (R1, QwQ): only the close is in the content.
        let prefilled = format!("결정을 정리하면… </think> 아니, 다시.\n</think>\n{SUMMARY}\n");
        assert_eq!(strip_think(&prefilled), SUMMARY);
        assert_eq!(strip_think("<think>끝나지 않은 생각"), "");
        assert_eq!(strip_think(&format!("\n{SUMMARY}\n")), SUMMARY);

        let stub = serve(move |_| Some(completion(&tagged)));
        let summary = run(&connection(&stub.url, "l", true), None, &lines(SPEECH));
        assert_eq!(summary.unwrap(), SUMMARY);
    }

    #[test]
    fn a_cut_or_empty_answer_fails() {
        let answer = |content: &str, finish: &str| {
            serde_json::json!({
                "choices": [{ "message": { "content": content }, "finish_reason": finish }]
            })
            .to_string()
        };
        let detail = |body: String| {
            let stub = serve(move |_| Some(reply("200 OK", &body)));
            match run(&connection(&stub.url, "l", true), None, &lines(SPEECH)) {
                Err(Failure::Provider(detail)) => detail,
                other => panic!("{other:?}"),
            }
        };
        let cut = detail(answer("# 회의\n## 결정사항\n- 배포는", "length"));
        assert!(cut.contains("길이 제한에서 잘렸습니다"), "{cut}");
        let thought = detail(answer("<think>끝나지 않은 생각", "length"));
        assert!(thought.contains("길이 제한에서 잘렸습니다"), "{thought}");
        let empty = detail(answer("  ", "stop"));
        assert!(
            empty.contains("빈 요약") && empty.contains("stop"),
            "{empty}"
        );
    }

    /// Ollama's `/v1` and LM Studio cut a long input to their own context without saying so;
    /// `usage.prompt_tokens` gives it away.
    #[test]
    fn a_server_that_read_far_less_than_it_was_sent_cut_the_input() {
        let with_usage = |prompt_tokens: u64| {
            let body = serde_json::json!({
                "choices": [{ "message": { "content": SUMMARY }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": prompt_tokens, "completion_tokens": 50 }
            })
            .to_string();
            let stub = serve(move |_| Some(reply("200 OK", &body)));
            run(&connection(&stub.url, "l", true), None, &lines(SPEECH))
        };
        let cut = with_usage(10);
        assert_eq!(cut, Err(Failure::Truncated));
        assert!(cut.unwrap_err().message().contains("OLLAMA_CONTEXT_LENGTH"));
        assert_eq!(with_usage(300).unwrap(), SUMMARY);
    }

    #[test]
    fn a_reply_that_is_not_a_completion_never_shows_the_key() {
        // Echoes the Authorization header back with 200.
        let stub = serve(|r| {
            let auth = r
                .head
                .lines()
                .find(|l| l.starts_with("authorization"))
                .unwrap();
            Some(reply(
                "200 OK",
                &serde_json::json!({ "echo": auth }).to_string(),
            ))
        });
        let result = run(
            &connection(&stub.url, "c", false),
            Some("sk-echo-secret"),
            &lines(SPEECH),
        );
        let Err(Failure::Provider(detail)) = result else {
            panic!("{result:?}");
        };
        assert!(detail.contains("bearer ***"), "{detail}");
        assert!(!detail.contains("sk-echo-secret"));
    }

    /// A dropped connection that is not a timeout is tried exactly once more too. (A refused
    /// connection is the same kind of error, but there is no listener to count its attempts.)
    #[test]
    fn a_broken_connection_is_tried_exactly_once_more() {
        // Reads the request and hangs up without answering.
        let stub = serve(|_| Some(Vec::new()));
        let result = run(
            &connection(&stub.url, "c", false),
            Some("k"),
            &lines(SPEECH),
        );
        assert!(matches!(result, Err(Failure::Network(_))), "{result:?}");
        assert_eq!(stub.connections.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn local_and_loopback_requests_skip_the_proxy() {
        let to = |url: &str, is_local| direct(&connection(url, "c", is_local));
        for url in [
            "http://localhost:11434/v1",
            "http://LOCALHOST:1234/v1",
            "http://ollama.localhost/v1",
            "http://127.0.0.1:1234/v1",
            "http://127.1.2.3/v1",
            "http://[::1]:8080/v1",
        ] {
            assert!(to(url, false), "{url}");
        }
        assert!(to("http://gpu-box.lan:11434/v1", true));
        assert!(!to("https://api.openai.com/v1", false));
        assert!(!to("http://10.0.0.5:11434/v1", false));
        assert!(!to("not a url", false));
    }

    /// SUM-08 with a real proxy setting. `HTTP_PROXY` is set only in a child run of this test, so
    /// no other test sees it: a local connection to another machine still goes direct, and a cloud
    /// host goes through the proxy (so the setting is really in force there).
    #[test]
    fn a_local_connection_never_goes_through_the_proxy() {
        const CHILD: &str = "SUMMARIZE_PROXY_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let llm = serve(|_| Some(completion(SUMMARY)));
            let local = run(&connection(&llm.url, "l", true), None, &lines(SPEECH));
            assert_eq!(local.unwrap(), SUMMARY);
            // No such host: this fails, but never at the proxy.
            let remote_local = connection("http://llm-box.invalid/v1", "l", true);
            assert!(run(&remote_local, None, &lines(SPEECH)).is_err());
            let cloud = connection("http://llm.invalid/v1", "c", false);
            assert_eq!(run(&cloud, Some("k"), &lines(SPEECH)).unwrap(), SUMMARY);
            return;
        }
        let proxy = serve(|_| Some(completion(SUMMARY)));
        let proxy_url = proxy.url.trim_end_matches("/v1").to_string();
        let name = "summarize::tests::a_local_connection_never_goes_through_the_proxy";
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env(CHILD, "1")
            .env("HTTP_PROXY", &proxy_url)
            .env("http_proxy", &proxy_url)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .env_remove("ALL_PROXY")
            .env_remove("all_proxy")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let seen: Vec<String> = proxy
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.head.lines().next().unwrap().to_string())
            .collect();
        assert_eq!(
            seen,
            ["post http://llm.invalid/v1/chat/completions http/1.1"]
        );
    }

    #[test]
    fn token_estimate_is_one_per_character() {
        assert_eq!(tokens(0), 0);
        assert_eq!(tokens(chars("회의록 작성")), 6);
    }

    #[test]
    fn under_thirty_characters_is_too_short() {
        assert!(too_short(&[]));
        assert!(too_short(&lines("음\n  네 네  \n시청해 주셔서")));
        // Whitespace does not count toward the 30.
        assert!(too_short(&[format!("{} ", "가 ".repeat(29))]));
        assert!(!too_short(&["가".repeat(30)]));
    }

    #[test]
    fn title_is_split_from_the_body() {
        assert_eq!(
            split_title("\n# 주간 회의 \n\n\n## 결정사항\n- 없음"),
            (Some("주간 회의".into()), "## 결정사항\n- 없음".into())
        );
        assert_eq!(
            split_title("## 결정사항\n- 없음"),
            (None, "## 결정사항\n- 없음".into())
        );
        assert_eq!(split_title("# 제목만"), (Some("제목만".into()), "".into()));
    }

    #[test]
    fn markdown_export_has_the_summary_then_the_timed_transcript() {
        let segments = [
            Segment {
                start_ms: 5_000,
                end_ms: 9_000,
                text: " 시작합니다".into(),
            },
            Segment {
                start_ms: 4_530_000,
                end_ms: 4_531_000,
                text: "<b>끝</b>".into(),
            },
        ];
        assert_eq!(
            markdown("주간 회의", "## 결정사항\n- 없음\n", &segments),
            "# 주간 회의\n\n## 결정사항\n- 없음\n\n---\n\n## 전사본\n\n[00:05] 시작합니다\n[75:30] <b>끝</b>\n"
        );
    }

    fn settings(connections: Vec<LlmConnection>, default: Option<&str>) -> Settings {
        Settings {
            llm_connections: connections,
            default_llm: default.map(String::from),
            ..Settings::default()
        }
    }

    fn meta(llm_connection: Option<&str>, summarize_local: bool) -> Meta {
        Meta {
            schema_version: 1,
            id: "m".into(),
            title: String::new(),
            status: Status::Transcribed,
            created_at: String::new(),
            duration_ms: 0,
            gaps: vec![],
            backend: None,
            chunks_done: 0,
            llm_connection: llm_connection.map(String::from),
            summarize_local,
            no_speech: false,
            error: None,
            sent: None,
        }
    }

    #[test]
    fn route_follows_the_choice_then_local_then_the_default() {
        let all = vec![
            connection("u", "cloud", false),
            connection("u", "ollama", true),
            connection("u", "lmstudio", true),
        ];
        let name = |s: &Settings, m: &Meta| route(s, m).map(|c| c.name.clone());
        let s = settings(all.clone(), Some("cloud"));
        assert_eq!(name(&s, &meta(None, false)), Ok("cloud".into()));
        // "로컬로 요약": the first local one unless the default is local.
        assert_eq!(name(&s, &meta(None, true)), Ok("ollama".into()));
        let s = settings(all.clone(), Some("lmstudio"));
        assert_eq!(name(&s, &meta(None, true)), Ok("lmstudio".into()));
        // A choice made earlier stands, even against the toggle.
        assert_eq!(name(&s, &meta(Some("cloud"), true)), Ok("cloud".into()));
        assert_eq!(
            name(&s, &meta(Some("gone"), false)),
            Err("LLM 연결 'gone'을(를) 찾을 수 없습니다".into())
        );
        // No default, or one that no longer exists: the first connection.
        for default in [None, Some("gone")] {
            let s = settings(all.clone(), default);
            assert_eq!(name(&s, &meta(None, false)), Ok("cloud".into()));
        }
        // Local asked, none there: never the cloud.
        let s = settings(vec![connection("u", "cloud", false)], Some("cloud"));
        assert_eq!(
            name(&s, &meta(None, true)),
            Err("로컬 LLM 연결이 없습니다".into())
        );
        assert!(name(&settings(vec![], None), &meta(None, false)).is_err());
    }

    #[test]
    fn summary_route_tells_where_and_whether_it_can_still_change() {
        let s = settings(
            vec![
                connection("u", "cloud", false),
                connection("u", "ollama", true),
            ],
            None,
        );
        let route = SummaryRoute::of(&s, &meta(None, false));
        assert_eq!(
            (
                route.connection.map(|c| c.name),
                route.local_available,
                route.locked
            ),
            (Some("cloud".into()), true, false)
        );
        assert!(SummaryRoute::of(&s, &meta(Some("cloud"), false)).locked);
        let review = Meta {
            status: Status::Review,
            ..meta(None, false)
        };
        assert!(SummaryRoute::of(&s, &review).locked);
        let none = SummaryRoute::of(&settings(vec![], None), &meta(None, true));
        assert_eq!(
            (none.connection, none.local, none.local_available),
            (None, true, false)
        );
        // Nothing was sent: no speech, or a review that never had a connection.
        let no_speech = Meta {
            status: Status::Review,
            no_speech: true,
            ..meta(Some("cloud"), false)
        };
        assert_eq!(SummaryRoute::of(&s, &no_speech).connection, None);
        assert_eq!(SummaryRoute::of(&s, &review).connection, None);
        let sent = Meta {
            status: Status::Review,
            ..meta(Some("ollama"), true)
        };
        let route = SummaryRoute::of(&s, &sent).connection;
        assert_eq!(route.map(|c| c.name), Some("ollama".into()));
    }

    /// 6.3 with a small context: every request stays within budget, parts end at line ends, and
    /// an overlong line is cut.
    #[test]
    fn a_long_meeting_goes_in_parts_each_within_budget() {
        let stub = serve(|r| {
            Some(completion(if r.body["max_tokens"] == PART_TOKENS {
                "## 결정사항\n- 부분 요약"
            } else {
                SUMMARY
            }))
        });
        let small = LlmConnection {
            context_tokens: 6000,
            ..connection(&stub.url, "l", true)
        };
        let mut transcript: Vec<String> = (0..40)
            .map(|i| format!("{i}번째 발화입니다. {}", "회의 내용 ".repeat(40)))
            .collect();
        transcript.insert(20, "끊김 없는 긴 발화".repeat(1000));
        let steps = Mutex::new(Vec::new());
        let summary = block_on(summarize(&small, None, &[], &transcript, |done, known| {
            steps.lock().unwrap().push((done, known))
        }));
        assert_eq!(summary.unwrap(), SUMMARY);

        let requests = stub.requests.lock().unwrap();
        assert!(requests.len() > 3, "{}", requests.len());
        for r in requests.iter() {
            let messages = &r.body["messages"];
            let input = chars(messages[0]["content"].as_str().unwrap())
                + chars(messages[1]["content"].as_str().unwrap());
            let answer = r.body["max_tokens"].as_u64().unwrap();
            assert!(
                tokens(input) + answer <= 6000 * 9 / 10,
                "{input} + {answer}"
            );
        }
        let (parts, last) = requests.split_at(requests.len() - 1);
        for r in parts {
            let user = r.body["messages"][1]["content"].as_str().unwrap();
            let content = user.strip_prefix(&message(PART, "")).unwrap();
            for line in content.lines() {
                assert!(
                    transcript.contains(&line.to_string()) || transcript[20].contains(line),
                    "not a whole line: {line}"
                );
            }
        }
        let user = last[0].body["messages"][1]["content"].as_str().unwrap();
        assert!(user.starts_with(MERGE));
        let n = parts.len() as u32;
        let steps = steps.lock().unwrap();
        assert_eq!(steps.first(), Some(&(0, n + 1)));
        assert_eq!(steps.last(), Some(&(n + 1, n + 1)));
    }

    fn talk(lines: usize) -> Vec<String> {
        (0..lines)
            .map(|i| format!("{i}번째 발화입니다. {}", "회의 내용 ".repeat(40)))
            .collect()
    }

    /// A stub whose partial summaries are `len` characters long.
    fn partials(len: usize) -> Stub {
        serve(move |r| {
            Some(completion(&if r.body["max_tokens"] == PART_TOKENS {
                format!("## 결정사항\n- {}", "가".repeat(len))
            } else {
                SUMMARY.to_string()
            }))
        })
    }

    fn small(stub: &Stub) -> LlmConnection {
        LlmConnection {
            context_tokens: 6000,
            ..connection(&stub.url, "l", true)
        }
    }

    fn user(r: &Request) -> &str {
        r.body["messages"][1]["content"].as_str().unwrap()
    }

    /// Partials over 60% that one request can still hold go straight to the final merge.
    #[test]
    fn partials_one_request_can_hold_are_merged_at_once() {
        let stub = partials(1600);
        assert_eq!(run(&small(&stub), None, &talk(20)).unwrap(), SUMMARY);
        let requests = stub.requests.lock().unwrap();
        let merges: Vec<&Request> = requests
            .iter()
            .filter(|r| user(r).starts_with(MERGE))
            .collect();
        assert_eq!((requests.len(), merges.len()), (3, 1));
        let input = chars(SYSTEM) + chars(user(merges[0]));
        assert!(tokens(input) > 6000 * 6 / 10, "{input}");
    }

    /// Partial summaries too long for one request are merged in groups first, a group of one is
    /// never summarized alone, and ones that cannot even pair up fail cleanly instead of looping.
    #[test]
    fn long_partials_are_reduced_again_and_a_hopeless_context_fails() {
        let transcript = talk(120);
        let stub = partials(1500);
        assert_eq!(run(&small(&stub), None, &transcript).unwrap(), SUMMARY);
        let requests = stub.requests.lock().unwrap();
        let merges: Vec<&Request> = requests
            .iter()
            .filter(|r| user(r).starts_with(MERGE))
            .collect();
        // Reduce passes, then the final merge; each merges two or more.
        assert!(merges.len() >= 3, "{}", merges.len());
        assert!(merges.iter().all(|r| user(r).contains(SEPARATOR)));
        for r in requests.iter() {
            let input = chars(r.body["messages"][0]["content"].as_str().unwrap()) + chars(user(r));
            let answer = r.body["max_tokens"].as_u64().unwrap();
            assert!(
                tokens(input) + answer <= 6000 * 9 / 10,
                "{input} + {answer}"
            );
        }

        let stub = partials(2000);
        assert_eq!(
            run(&small(&stub), None, &transcript),
            Err(Failure::TooSmall)
        );

        // Too small for even one part: nothing is sent.
        let stub = partials(10);
        let tiny = LlmConnection {
            context_tokens: 2048,
            ..small(&stub)
        };
        assert_eq!(run(&tiny, None, &transcript), Err(Failure::TooSmall));
        assert_eq!(stub.connections.load(Ordering::SeqCst), 0);
        assert!(Failure::TooSmall.message().contains("컨텍스트 토큰"));
    }

    #[test]
    fn a_failed_part_fails_the_summary_and_says_which() {
        let calls = AtomicUsize::new(0);
        let stub = serve(move |_| {
            Some(if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                reply("429 Too Many Requests", "{}")
            } else {
                completion("## 결정사항\n- 부분")
            })
        });
        let transcript: Vec<String> = (0..40)
            .map(|i| format!("{i}번째 발화입니다. {}", "회의 내용 ".repeat(40)))
            .collect();
        let small = LlmConnection {
            context_tokens: 6000,
            ..connection(&stub.url, "l", true)
        };
        let failure = run(&small, None, &transcript).unwrap_err();
        let Failure::Part { n: 2, of, .. } = &failure else {
            panic!("{failure:?}");
        };
        assert!(
            failure
                .message()
                .ends_with(&format!("(구간 2/{of}에서 실패)"))
        );
        assert!(failure.message().starts_with("LLM 사용 한도"));
    }

    // Summary eval (architecture 12.3): `cargo test -- --ignored summary_eval`.

    #[derive(Deserialize, Default)]
    #[serde(default, deny_unknown_fields)]
    struct Expected {
        glossary: Vec<String>,
        /// Section → keyword groups: every keyword of a group must be in that section.
        must: BTreeMap<String, Vec<Vec<String>>>,
        /// Keyword groups no line of 결정사항 may hold all of: a rejected proposal, a "~하지 않기로",
        /// the decision before it was reversed.
        must_not_in_decisions: Vec<Vec<String>>,
        /// Keyword groups whose action item must have the owner 미정.
        unassigned: Vec<Vec<String>>,
    }

    const SECTIONS: [&str; 4] = ["결정사항", "액션 아이템", "주요 논의", "미해결 질문"];

    /// The lines under `## {name}`, up to the next heading.
    fn section(summary: &str, name: &str) -> Option<String> {
        let heading = format!("## {name}");
        let mut lines = summary.lines().skip_while(|l| l.trim() != heading);
        lines.next()?;
        let body: Vec<&str> = lines
            .take_while(|l| !l.trim_start().starts_with('#'))
            .collect();
        Some(body.join("\n"))
    }

    /// `- [담당자] 할 일 (기한)` → 담당자.
    fn owner(line: &str) -> Option<&str> {
        let rest = line.trim_start().strip_prefix("- [")?;
        rest.split_once(']').map(|(owner, _)| owner.trim())
    }

    fn has_all(text: &str, group: &[String]) -> bool {
        group.iter().all(|k| text.contains(k.as_str()))
    }

    /// Every way a summary misses its fixture, so one run reports them all.
    fn check(summary: &str, transcript: &str, expected: &Expected) -> Vec<String> {
        let mut failures = Vec::new();
        match split_title(summary).0 {
            None => failures.push("제목(# …) 없음".to_string()),
            Some(title) if chars(&title) > 20 => {
                failures.push(format!("제목이 20자 초과: {title}"))
            }
            Some(_) => {}
        }
        for name in SECTIONS {
            if section(summary, name).is_none() {
                failures.push(format!("섹션 없음: {name}"));
            }
        }
        let actions = section(summary, "액션 아이템").unwrap_or_default();
        for line in actions.lines() {
            for name in owner(line)
                .into_iter()
                .flat_map(|o| o.split([',', '·', '/']))
            {
                let name = name.trim();
                if name != "미정"
                    && !transcript.contains(name)
                    && !expected.glossary.iter().any(|g| g == name)
                {
                    failures.push(format!("전사본·용어집에 없는 담당자 '{name}': {line}"));
                }
            }
        }
        for (name, groups) in &expected.must {
            let text = section(summary, name).unwrap_or_default();
            for group in groups.iter().filter(|g| !has_all(&text, g)) {
                failures.push(format!("{name}에 없음: {group:?}"));
            }
        }
        let decisions = section(summary, "결정사항").unwrap_or_default();
        for group in &expected.must_not_in_decisions {
            if let Some(line) = decisions.lines().find(|l| has_all(l, group)) {
                failures.push(format!("결정사항에 들어가면 안 됨 {group:?}: {line}"));
            }
        }
        for group in &expected.unassigned {
            match actions.lines().find(|l| has_all(l, group)) {
                None => failures.push(format!("액션 아이템에 없음: {group:?}")),
                Some(line) if owner(line) != Some("미정") => {
                    failures.push(format!("담당자가 미정이 아님: {line}"))
                }
                Some(_) => {}
            }
        }
        failures
    }

    const TRANSCRIPT: &str = "배포는 화요일로 하죠\n아니다 목요일로 바꾸죠\n민수 씨가 릴리스 노트 써 주세요\n대시보드는 누가 할지 미정이에요\n외주는 맡기지 않기로 했습니다";

    fn expected() -> Expected {
        serde_json::from_str(
            r#"{
                "glossary": ["누리패스"],
                "must": {"결정사항": [["목요일", "배포"]], "액션 아이템": [["릴리스 노트"]]},
                "must_not_in_decisions": [["외주"], ["화요일"]],
                "unassigned": [["대시보드"]]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn expected_json_rejects_a_misspelt_key() {
        let typo = serde_json::from_str::<Expected>(r#"{"must_not_in_decision": [["외주"]]}"#);
        assert!(typo.is_err());
    }

    #[test]
    fn eval_check_passes_a_good_summary() {
        let good = "# 주간 회의\n## 결정사항\n- 배포는 목요일\n## 액션 아이템\n- [민수] 릴리스 노트 (금요일)\n- [미정] 대시보드 개선 (미정)\n## 주요 논의\n- 외주는 맡기지 않기로 함\n## 미해결 질문\n- 없음";
        assert_eq!(check(good, TRANSCRIPT, &expected()), Vec::<String>::new());
    }

    #[test]
    fn eval_check_reports_every_miss_at_once() {
        let bad = "# 아주아주아주아주아주아주 긴 회의 제목입니다\n## 결정사항\n- 배포는 화요일\n- 외주는 하지 않는다\n## 액션 아이템\n- [철수] 릴리스 노트\n- [민수] 대시보드 개선\n## 주요 논의\n- 없음";
        let failures = check(bad, TRANSCRIPT, &expected());
        let has = |s: &str| failures.iter().any(|f| f.contains(s));
        assert!(has("제목이 20자 초과"), "{failures:?}");
        assert!(has("섹션 없음: 미해결 질문"));
        assert!(has("담당자 '철수'"));
        assert!(!has("담당자 '민수'"));
        assert!(has(r#"결정사항에 없음: ["목요일", "배포"]"#));
        assert!(has(r#"들어가면 안 됨 ["외주"]"#) && has(r#"들어가면 안 됨 ["화요일"]"#));
        assert!(has("담당자가 미정이 아님: - [민수] 대시보드"));
        assert_eq!(failures.len(), 7, "{failures:?}");
        // An owner may come from the glossary, and an action item may be missing entirely.
        let glossary_owner = "# 회의\n## 결정사항\n- 없음\n## 액션 아이템\n- [누리패스] 점검\n## 주요 논의\n- 없음\n## 미해결 질문\n- 없음";
        let failures = check(glossary_owner, TRANSCRIPT, &expected());
        assert!(
            !failures.iter().any(|f| f.contains("누리패스")),
            "{failures:?}"
        );
        assert!(failures.iter().any(|f| f.contains("액션 아이템에 없음")));
    }

    /// Calls a real LLM, so it is left out of the normal run. Reads every `tests/fixtures/*.txt`
    /// (one utterance per line) with its `*.expected.json`.
    #[test]
    #[ignore]
    fn summary_eval() {
        let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
        let (Some(base_url), Some(model)) =
            (var("SUMMARY_EVAL_BASE_URL"), var("SUMMARY_EVAL_MODEL"))
        else {
            panic!(
                "SUMMARY_EVAL_BASE_URL과 SUMMARY_EVAL_MODEL을 설정하세요. 선택: SUMMARY_EVAL_API_KEY, SUMMARY_EVAL_CONTEXT_TOKENS(기본 128000), SUMMARY_EVAL_LOCAL=1"
            );
        };
        let is_local = var("SUMMARY_EVAL_LOCAL").as_deref() == Some("1");
        let connection = LlmConnection {
            name: "eval".into(),
            base_url,
            model,
            context_tokens: var("SUMMARY_EVAL_CONTEXT_TOKENS").map_or(128_000, |v| {
                v.parse().expect("SUMMARY_EVAL_CONTEXT_TOKENS는 숫자")
            }),
            is_local,
            timeout_secs: if is_local { 900 } else { 300 },
        };
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut fixtures: Vec<PathBuf> = fs::read_dir(&dir)
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        fixtures.retain(|p| p.extension().is_some_and(|e| e == "txt"));
        fixtures.sort();
        assert!(
            !fixtures.is_empty(),
            "{}에 고정 전사본(*.txt)과 *.expected.json을 넣으세요",
            dir.display()
        );
        let mut report = Vec::new();
        for path in fixtures {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let transcript = fs::read_to_string(&path).unwrap();
            let expected_path = path.with_extension("expected.json");
            let expected: Expected = serde_json::from_slice(
                &fs::read(&expected_path)
                    .unwrap_or_else(|e| panic!("{}: {e}", expected_path.display())),
            )
            .unwrap_or_else(|e| panic!("{}: {e}", expected_path.display()));
            let lines: Vec<String> = transcript
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            let summary = block_on(summarize(
                &connection,
                var("SUMMARY_EVAL_API_KEY"),
                &expected.glossary,
                &lines,
                |done, known| eprintln!("{name}: 요청 {done}/{known}"),
            ));
            match summary {
                Ok(summary) => {
                    eprintln!("=== {name} ===\n{summary}\n");
                    let failures = check(&summary, &transcript, &expected);
                    if !failures.is_empty() {
                        report.push(format!("{name}:\n  {}", failures.join("\n  ")));
                    }
                }
                Err(e) => report.push(format!("{name}: 요약 실패: {}", e.message())),
            }
        }
        assert!(report.is_empty(), "\n{}", report.join("\n"));
    }
}
