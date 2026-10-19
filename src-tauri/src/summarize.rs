//! Summaries (architecture 6, SUM-01..10): one OpenAI-compatible Chat Completions client for cloud
//! providers and local Ollama / LM Studio alike. A long meeting is summarized in parts that are then
//! merged, every request within its token budget (6.3).
//!
//! Pure and tested without a server: `route` (which connection), `split_title`, `too_short` and the
//! budget (`tokens`, `pack`). The requests are tested against local stub servers, the built-in
//! route's against the sidecar tests' fake llama-server.

use crate::settings::{BUILTIN, LlmConnection, Settings};
use crate::sidecar;
use crate::store::{Meta, Status, Wait};
use crate::transcribe::engine::Segment;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Architecture 6.1. The whole answer has the connection's `timeout_secs`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The answer room of a partial summary (`part_tokens`), and the most a final summary gets (6.3).
const PART_TOKENS: u64 = 1500;
const FINAL_TOKENS: u64 = 4000;
/// What a reasoning model may spend thinking. It counts against `max_completion_tokens`, so the
/// compatible request (6.1) adds it to the answer room.
const REASONING_TOKENS: u64 = 8000;
/// SUM-10: below this many characters (whitespace aside) the LLM is not asked at all.
const MIN_CHARS: usize = 30;
/// llama-server's error `type` (with HTTP 400) for a request over its context, at the pinned
/// commit: "request (N tokens) exceeds the available context size (M tokens), try increasing it".
const CONTEXT_EXCEEDED: &str = "exceed_context_size_error";

/// Architecture 6.2. The glossary line is added only when there is a glossary.
const SYSTEM: &str = "\
너는 회의록 작성자다. 한국어 회의 전사본을 읽고, 회의에 없던 사람이 읽을 요약을 쓴다.
출력은 아래 Markdown 형식만 쓴다.

# {회의 제목: 날짜 없이 20자 이내}
## 결정사항
- ...
## 액션 아이템
- [담당자] 할 일 (기한)
## 주요 논의
- ...
## 미해결 질문
- ...

규칙:
- 한국어로만 쓰고 한자나 다른 언어를 섞지 않는다. 이름, 제품명, 영어 용어는 전사본이나 용어집 표기대로 쓴다.
- 전사본에는 말한 사람 표시가 없다. 전사본에 명시되지 않은 담당자나 기한은 \"미정\"으로 쓴다. 추정하지 않는다.
- 날짜는 회의에서 말한 표현만 쓰고, 회의에서 말하지 않은 연도는 붙이지 않는다.
- 해당 내용이 없는 섹션은 \"- 없음\"으로 쓴다.
- 결정사항에는 합의한 결과만 쓴다. 기각되거나 미룬 제안과 \"~하지 않기로\" 한 것은 결정사항에 쓰지 않는다. 기각된 것과 \"~하지 않기로\" 한 것은 필요하면 주요 논의에, 미룬 것과 결론이 나지 않은 것은 미해결 질문에 쓴다.
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
    /// The answer stopped at its token limit (`finish_reason: "length"`).
    Length,
    /// The built-in route's own failure (plan 5.3): its words are `sidecar::Failure::message`'s.
    Builtin(sidecar::Failure),
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
            Failure::Length => "LLM 공급자 오류: 요약이 길이 제한에서 잘렸습니다. 컨텍스트 토큰 설정을 확인해 주세요".into(),
            Failure::Builtin(engine) => engine.message(),
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

/// Which connection summarizes a meeting (SUM-08, plan 5.4, architecture 6.1), out of
/// `Settings::connections`: the one it is pinned to (a retry's, or an earlier start's); for
/// "로컬로 요약" the default if it is local, else the built-in route, else the first saved local
/// one; else the default, which is the built-in route when `default_llm` is `None` or names no
/// connection. The built-in route takes `builtin_ready` (the summary model is installed). Nothing
/// falls back to another connection unasked: "로컬로 요약" never goes to the cloud, and a meeting
/// pinned to a connection that is gone waits for another to be picked.
pub fn route(settings: &Settings, meta: &Meta, builtin_ready: bool) -> Result<LlmConnection, Wait> {
    let all = settings.connections();
    let usable = |c: &&LlmConnection| c.name != BUILTIN || builtin_ready;
    let named = |name: &str| all.iter().find(|c| c.name == name);
    let builtin = named(BUILTIN);
    if let Some(name) = &meta.llm_connection {
        let pinned = named(name).ok_or(Wait::ConnectionDeleted)?;
        return Some(pinned)
            .filter(usable)
            .cloned()
            .ok_or(Wait::SummaryModel);
    }
    let default = settings
        .default_connection()
        .and_then(|d| named(&d.name))
        .or(builtin);
    let chosen = if meta.summarize_local {
        default
            .filter(|c| c.is_local)
            .filter(usable)
            .or(builtin.filter(usable))
            .or_else(|| all.iter().filter(usable).find(|c| c.is_local))
    } else {
        default.filter(usable)
    };
    chosen.cloned().ok_or(Wait::SummaryModel)
}

/// SUM-08's toggle is offered (plan 5.4): the built-in route is ready, or a saved connection is
/// local.
pub fn local_available(settings: &Settings, builtin_ready: bool) -> bool {
    builtin_ready
        || settings
            .connections()
            .iter()
            .any(|c| c.is_local && c.name != BUILTIN)
}

/// `summary_route`: where a meeting's summary goes, for the SUM-07 notice and the SUM-08 toggle.
#[derive(Serialize, Debug, PartialEq)]
pub struct SummaryRoute {
    /// `None` while the meeting's route does not resolve (`route`), or when nothing was sent (no
    /// speech, or reviewed without a connection). The built-in route is `settings::BUILTIN`. Holds
    /// no secret.
    pub connection: Option<LlmConnection>,
    /// The meeting's "로컬로 요약".
    pub local: bool,
    /// `local_available`, so the toggle is offered.
    pub local_available: bool,
    /// Summarizing has started: the route no longer changes.
    pub locked: bool,
    /// Flipping "로컬로 요약" changes where this meeting's summary goes, or whether it waits: only
    /// then is the toggle worth offering (plan 5.4). Not with a local default, nor with this PC
    /// as the default and its model in.
    pub toggle_matters: bool,
}

impl SummaryRoute {
    pub fn of(settings: &Settings, meta: &Meta, builtin_ready: bool) -> Self {
        // Nothing went anywhere: SUM-07 must not say "전송됨".
        let sent_nothing = meta.no_speech
            || (meta.llm_connection.is_none()
                && matches!(
                    meta.status,
                    Status::Review | Status::Sent | Status::Archived
                ));
        Self {
            connection: (!sent_nothing)
                .then(|| route(settings, meta, builtin_ready).ok())
                .flatten(),
            local: meta.summarize_local,
            local_available: local_available(settings, builtin_ready),
            locked: meta.summary_started(),
            toggle_matters: {
                let flipped = Meta {
                    summarize_local: !meta.summarize_local,
                    ..meta.clone()
                };
                route(settings, &flipped, builtin_ready) != route(settings, meta, builtin_ready)
            },
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
pub(crate) fn pack(items: Vec<String>, sep: &str, room: usize) -> Vec<Vec<String>> {
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
pub(crate) fn cut(line: &str, room: usize) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    chars.chunks(room).map(|c| c.iter().collect()).collect()
}

/// A URL to this machine: `localhost`, `*.localhost`, `127.0.0.0/8`, `::1`.
pub(crate) fn loopback(url: &str) -> bool {
    reqwest::Url::parse(url.trim())
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .is_some_and(|host| {
            host == "localhost"
                || host.ends_with(".localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        })
}

/// Requests that never go through a proxy: a local connection's (SUM-08 keeps the transcript off
/// the network), and any to a loopback address. With `system-proxy` they would otherwise reach a
/// company proxy in cleartext: hyper-util ignores the macOS proxy exceptions, and Windows'
/// `<local>` does not match `localhost`.
fn direct(connection: &LlmConnection) -> bool {
    connection.is_local || loopback(&connection.base_url)
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

/// llama-server's `timings` of a summary's answers, added up (plan 5.3, 5.8): for the log, the eval
/// (DX7) and later the speed hint (E2). Zero from servers that send none.
#[derive(Deserialize, Default, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Timings {
    pub prompt_n: u64,
    pub prompt_ms: f64,
    pub predicted_n: u64,
    pub predicted_ms: f64,
}

impl std::ops::AddAssign for Timings {
    fn add_assign(&mut self, t: Self) {
        self.prompt_n += t.prompt_n;
        self.prompt_ms += t.prompt_ms;
        self.predicted_n += t.predicted_n;
        self.predicted_ms += t.predicted_ms;
    }
}

/// The answer of a Chat Completions reply to `input_chars` characters of input, with its timings.
/// A cut input, a cut answer (`finish_reason: "length"`) and an empty one are failures, not
/// summaries.
fn content(
    body: &str,
    key: Option<&str>,
    input_chars: usize,
) -> Result<(String, Timings), Failure> {
    #[derive(Deserialize)]
    struct Reply {
        choices: Vec<Choice>,
        usage: Option<Usage>,
        timings: Option<Timings>,
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
        return Err(Failure::Length);
    }
    let text = strip_think(choice.message.content.as_deref().unwrap_or_default());
    if text.is_empty() {
        return Err(Failure::Provider(format!(
            "빈 요약을 받았습니다(finish_reason: {})",
            choice.finish_reason.as_deref().unwrap_or("없음")
        )));
    }
    Ok((text.to_string(), reply.timings.unwrap_or_default()))
}

/// The built-in route's `on_failure` hook (plan 5.3, `Sidecar::on_failure`): told whether the
/// request timed out and whether the server answered since the last call, it gives the base URL
/// of a server to ask again, or why the summary ends.
pub type Recover = Box<
    dyn FnMut(bool, bool) -> Pin<Box<dyn Future<Output = Result<String, sidecar::Failure>> + Send>>
        + Send,
>;

fn completions(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim().trim_end_matches('/'))
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
    /// The built-in route's only.
    recover: Option<Recover>,
    /// An answer came since `recover` was last called.
    answered: bool,
    timings: Timings,
}

impl<'a> Llm<'a> {
    fn new(
        connection: &'a LlmConnection,
        key: Option<&'a str>,
        recover: Option<Recover>,
    ) -> Result<Self, Failure> {
        let mut http = reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT);
        if direct(connection) {
            http = http.no_proxy();
        }
        let http = http.build().map_err(|e| Failure::Network(e.to_string()))?;
        Ok(Self {
            http,
            url: completions(&connection.base_url),
            connection,
            key,
            compat: false,
            recover,
            answered: false,
            timings: Timings::default(),
        })
    }

    /// A network failure (a timeout too) is tried once more; nothing else is (6.4). On the built-in
    /// route, transport errors and timeouts go to `recover` instead (plan 5.3, ENG6): a server
    /// brought back after a death gets this request again, a timeout is not asked again. An HTTP
    /// status is a live server's answer and goes no further.
    async fn ask(&mut self, system: &str, user: &str, max_tokens: u64) -> Result<String, Failure> {
        if self.recover.is_none() {
            return match self.send(system, user, max_tokens).await {
                Err(Failure::Network(_) | Failure::Timeout) => {
                    self.send(system, user, max_tokens).await
                }
                result => result,
            };
        }
        loop {
            let timed_out = match self.send(system, user, max_tokens).await {
                Err(Failure::Timeout) => true,
                Err(Failure::Network(_) | Failure::LocalDown) => false,
                result => {
                    self.answered |= result.is_ok();
                    return result;
                }
            };
            let answered = std::mem::take(&mut self.answered);
            let recover = self.recover.as_mut().expect("the built-in route");
            let base_url = recover(timed_out, answered)
                .await
                .map_err(Failure::Builtin)?;
            self.url = completions(&base_url);
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
                200..=299 => {
                    let (text, timings) = content(&text, self.key, chars(system) + chars(user))?;
                    self.timings += timings;
                    Ok(text)
                }
                // Our engine refused an input over its `-c`: the budget's estimate fell short, which
                // is the cut input's failure (ENG12).
                400 if self.recover.is_some() && text.contains(CONTEXT_EXCEEDED) => {
                    Err(Failure::Truncated)
                }
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
/// requests: `known` grows when a merge needs another pass. `recover`: the built-in route's hook,
/// which also gives its failures the built-in words (`engine_failure`). Returns the summary and
/// its answers' timings.
pub async fn summarize(
    connection: &LlmConnection,
    key: Option<String>,
    glossary: &[String],
    lines: &[String],
    on_step: impl Fn(u32, u32),
    recover: Option<Recover>,
) -> Result<(String, Timings), Failure> {
    let builtin = recover.is_some();
    let key = key.filter(|k| !k.is_empty());
    if !connection.is_local && key.is_none() {
        return Err(Failure::NoKey);
    }
    let summary = match Llm::new(connection, key.as_deref(), recover) {
        Ok(mut llm) => parts(&mut llm, glossary, lines, on_step)
            .await
            .map(|text| (text, llm.timings)),
        Err(failure) => Err(failure),
    };
    if builtin {
        summary.map_err(engine_failure)
    } else {
        summary
    }
}

/// Plan 5.3, DX5, ENG12, ENG16: the built-in route's words for a failure. Its budget checks stay
/// with words of their own; nothing about Ollama, context tokens or the internet reaches it. An
/// HTTP error or an unreadable answer from our own server is the model's failure, logged.
fn engine_failure(failure: Failure) -> Failure {
    Failure::Builtin(match failure {
        Failure::Builtin(engine) => engine,
        Failure::Part { n, of, cause } => {
            return Failure::Part {
                n,
                of,
                cause: Box::new(engine_failure(*cause)),
            };
        }
        Failure::TooSmall => sidecar::Failure::GlossaryTooLong,
        Failure::Length => sidecar::Failure::Length,
        Failure::Truncated => {
            log::error!("이 PC 요약 엔진이 보낸 입력보다 훨씬 적게 읽었습니다");
            sidecar::Failure::Truncated
        }
        other => {
            log::error!("이 PC 요약 엔진의 답을 쓸 수 없습니다: {}", other.message());
            sidecar::Failure::Unreadable
        }
    })
}

/// A partial summary's answer room: `PART_TOKENS`, or a third of the budget when that is less, so a
/// 4,096-token context still holds the prompt, a glossary and a partial summary to merge. From a
/// 5,000-token context up it is `PART_TOKENS`.
fn part_tokens(budget: u64) -> u64 {
    PART_TOKENS.min(budget / 3)
}

async fn parts(
    llm: &mut Llm<'_>,
    glossary: &[String],
    lines: &[String],
    on_step: impl Fn(u32, u32),
) -> Result<String, Failure> {
    let mut system = SYSTEM.to_string();
    if !glossary.is_empty() {
        system += &format!("\n- 용어집의 표기를 따른다: {}", glossary.join(", "));
    }
    let context = u64::from(llm.connection.context_tokens);
    let budget = context * 9 / 10;
    let part = part_tokens(budget);
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
    let (part_room, merge_room) = (room(PART, part), room(MERGE, part));
    // A request must hold one partial summary at its full length, or merging can never fit.
    if merge_room < part as usize {
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
    for (i, piece) in parts.iter().enumerate() {
        let summary = llm.ask(&system, &message(PART, piece), part).await;
        summaries.push(summary.map_err(|e| e.at(i + 1, parts.len()))?);
        done += 1;
        on_step(done, known);
    }
    loop {
        let merged = message(MERGE, &summaries.join(SEPARATOR));
        // Within 60%, or one request holds them all: another pass would only hand the final
        // merge a single summary of them.
        if within(&merged) || input(&merged) + part <= budget {
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
            let summary = llm.ask(&system, &merged, part).await;
            summaries.push(summary.map_err(|e| e.at(n, merging))?);
            done += 1;
            on_step(done, known);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::grounding;
    use crate::store::Status;
    use crate::transcribe::engine::Device;
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
            None,
        ))
        .map(|(summary, _)| summary)
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
            None,
        ))
        .unwrap();
        // A server without `timings` adds nothing to them.
        assert_eq!(summary, (SUMMARY.to_string(), Timings::default()));
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
        assert!(!system.contains("용어집의 표기를 따른다"));
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
        let failure = |body: String| {
            let stub = serve(move |_| Some(reply("200 OK", &body)));
            run(&connection(&stub.url, "l", true), None, &lines(SPEECH)).unwrap_err()
        };
        let detail = |body: String| match failure(body) {
            Failure::Provider(detail) => detail,
            other => panic!("{other:?}"),
        };
        let cut = failure(answer("# 회의\n## 결정사항\n- 배포는", "length"));
        assert_eq!(cut, Failure::Length);
        assert!(cut.message().contains("길이 제한에서 잘렸습니다"));
        let thought = failure(answer("<think>끝나지 않은 생각", "length"));
        assert_eq!(thought, Failure::Length);
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
            grounding_replaced: 0,
            grounding_checked: None,
            wait: None,
        }
    }

    #[test]
    fn route_follows_the_pin_then_local_then_the_default_then_the_builtin_route() {
        let all = vec![
            connection("u", "cloud", false),
            connection("u", "ollama", true),
            connection("u", "lmstudio", true),
        ];
        let name = |s: &Settings, m: &Meta, ready| route(s, m, ready).map(|c| c.name);
        let both = |s: &Settings, m: &Meta| (name(s, m, true), name(s, m, false));
        let ok = |n: &str| Ok(n.to_string());
        let model = || Err(Wait::SummaryModel);

        let s = settings(all.clone(), Some("cloud"));
        assert_eq!(both(&s, &meta(None, false)), (ok("cloud"), ok("cloud")));
        // "로컬로 요약" with a cloud default: the built-in route when ready, else the first saved
        // local one.
        assert_eq!(both(&s, &meta(None, true)), (ok(BUILTIN), ok("ollama")));
        // A local default comes first.
        let s = settings(all.clone(), Some("lmstudio"));
        assert_eq!(
            both(&s, &meta(None, true)),
            (ok("lmstudio"), ok("lmstudio"))
        );
        // A pin stands, even against the toggle; the built-in route's needs the model.
        assert_eq!(
            both(&s, &meta(Some("cloud"), true)),
            (ok("cloud"), ok("cloud"))
        );
        assert_eq!(
            both(&s, &meta(Some(BUILTIN), false)),
            (ok(BUILTIN), model())
        );
        // ENG15: a pinned connection that is gone is never swapped for another.
        let deleted = || Err(Wait::ConnectionDeleted);
        assert_eq!(both(&s, &meta(Some("gone"), true)), (deleted(), deleted()));

        // No default (the settings' first connection does not make one), or one that is gone:
        // the built-in route, and while its model is missing the meeting waits for it.
        for default in [None, Some("gone")] {
            let s = settings(all.clone(), default);
            assert_eq!(both(&s, &meta(None, false)), (ok(BUILTIN), model()));
            assert_eq!(both(&s, &meta(None, true)), (ok(BUILTIN), ok("ollama")));
        }
        // Local asked, nothing local but the built-in route: never the cloud.
        let s = settings(vec![connection("u", "cloud", false)], Some("cloud"));
        assert_eq!(both(&s, &meta(None, true)), (ok(BUILTIN), model()));
        assert_eq!(
            both(&settings(vec![], None), &meta(None, false)),
            (ok(BUILTIN), model())
        );
        assert_eq!(
            route(&s, &meta(None, true), true),
            Ok(crate::settings::builtin())
        );
    }

    /// d6: saved connections named like the built-in route never take its place.
    #[test]
    fn a_saved_connection_with_a_reserved_name_is_out_of_every_route() {
        let s = settings(
            vec![
                connection("http://evil.example/v1", BUILTIN, false),
                connection("http://evil.example/v1", "이 PC", true),
            ],
            Some(BUILTIN),
        );
        let builtin = Ok(crate::settings::builtin());
        assert_eq!(route(&s, &meta(None, false), true), builtin);
        assert_eq!(route(&s, &meta(Some(BUILTIN), false), true), builtin);
        assert_eq!(route(&s, &meta(None, true), false), Err(Wait::SummaryModel));
        let pinned = route(&s, &meta(Some("이 PC"), false), true);
        assert_eq!(pinned, Err(Wait::ConnectionDeleted));
        assert!(!local_available(&s, false) && local_available(&s, true));
    }

    /// Plan 5.4: "로컬로 요약" is offered only where flipping it changes the route.
    #[test]
    fn the_local_toggle_matters_only_where_it_changes_the_route() {
        let ollama = connection("u", "ollama", true);
        let cloud = connection("u", "cloud", false);
        let matters = |s: &Settings, ready: bool| {
            let off = SummaryRoute::of(s, &meta(None, false), ready).toggle_matters;
            let on = SummaryRoute::of(s, &meta(None, true), ready).toggle_matters;
            assert_eq!(off, on, "the same either way");
            off
        };
        // This PC as the default, its model in: this PC either way.
        assert!(!matters(&settings(vec![ollama.clone()], None), true));
        // This PC without its model and a saved local one: on goes there, off waits.
        assert!(matters(&settings(vec![ollama.clone()], None), false));
        // Without either, it waits either way.
        assert!(!matters(&settings(vec![], None), false));
        // A cloud default.
        assert!(matters(
            &settings(vec![cloud.clone(), ollama.clone()], Some("cloud")),
            true
        ));
        // A saved local default.
        assert!(!matters(
            &settings(vec![cloud.clone(), ollama.clone()], Some("ollama")),
            false
        ));
        // A default whose name is gone is this PC.
        assert!(!matters(&settings(vec![cloud], Some("gone")), true));
        // Pinned: the toggle changes nothing.
        let s = settings(vec![ollama], Some("ollama"));
        assert!(!SummaryRoute::of(&s, &meta(Some("ollama"), false), true).toggle_matters);
    }

    #[test]
    fn summary_route_tells_where_and_whether_it_can_still_change() {
        let s = settings(
            vec![
                connection("u", "cloud", false),
                connection("u", "ollama", true),
            ],
            Some("cloud"),
        );
        let route = SummaryRoute::of(&s, &meta(None, false), false);
        assert_eq!(
            (
                route.connection.map(|c| c.name),
                route.local_available,
                route.locked
            ),
            (Some("cloud".into()), true, false)
        );
        assert!(SummaryRoute::of(&s, &meta(Some("cloud"), false), false).locked);
        let review = Meta {
            status: Status::Review,
            ..meta(None, false)
        };
        assert!(SummaryRoute::of(&s, &review, false).locked);
        // Nothing saved: local only once the summary model is in, and no route before it.
        let none = SummaryRoute::of(&settings(vec![], None), &meta(None, true), false);
        assert_eq!(
            (none.connection, none.local, none.local_available),
            (None, true, false)
        );
        // A cloud default: the toggle sends it to this PC or a local connection instead.
        assert!(route.toggle_matters);
        let ready = SummaryRoute::of(&settings(vec![], None), &meta(None, true), true);
        assert_eq!(
            (ready.connection.map(|c| c.name), ready.local_available),
            (Some(BUILTIN.into()), true)
        );
        // Nothing was sent: no speech, or a review that never had a connection.
        let no_speech = Meta {
            status: Status::Review,
            no_speech: true,
            ..meta(Some("cloud"), false)
        };
        assert_eq!(SummaryRoute::of(&s, &no_speech, true).connection, None);
        assert_eq!(SummaryRoute::of(&s, &review, true).connection, None);
        let sent = Meta {
            status: Status::Review,
            ..meta(Some("ollama"), true)
        };
        let route = SummaryRoute::of(&s, &sent, true).connection;
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
        let on_step = |done, known| steps.lock().unwrap().push((done, known));
        let summary = block_on(summarize(&small, None, &[], &transcript, on_step, None));
        assert_eq!(summary.unwrap().0, SUMMARY);

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
        // Two of them with the prompt just over 60% of the context: about 100 characters over.
        let stub = partials((6000 * 6 / 10 - chars(SYSTEM) - chars(MERGE)) / 2 + 50);
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

    /// Review: a 4,096-token connection with a short glossary still summarizes a long meeting in
    /// parts, its partial summaries given a third of the budget; from 5,000 tokens up nothing
    /// changes.
    #[test]
    fn a_small_context_with_a_glossary_still_goes_in_parts() {
        for (context, part) in [(4096, 1228), (5000, 1500), (8192, 1500), (16_384, 1500)] {
            assert_eq!(part_tokens(context * 9 / 10), part, "{context}");
        }
        let stub = serve(|r| {
            Some(completion(if user(r).starts_with(PART) {
                "## 결정사항\n- 부분 요약"
            } else {
                SUMMARY
            }))
        });
        let glossary = [
            "누리패스",
            "김민수",
            "박지현",
            "대시보드",
            "온보딩 문서",
            "결제 모듈",
            "회귀 테스트",
            "릴리스 노트",
            "QA 팀",
            "영업팀",
            "보안 점검",
            "이중 인증",
        ]
        .map(String::from);
        assert!((60..80).contains(&chars(&glossary.join(", "))));
        let small = LlmConnection {
            context_tokens: 4096,
            ..connection(&stub.url, "l", true)
        };
        let summary = block_on(summarize(
            &small,
            None,
            &glossary,
            &talk(60),
            |_, _| {},
            None,
        ));
        assert_eq!(summary.unwrap().0, SUMMARY);
        let requests = stub.requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .filter(|r| user(r).starts_with(PART))
                .count()
                > 2
        );
        assert!(requests.iter().any(|r| user(r).starts_with(MERGE)));
        for r in requests.iter() {
            let input = chars(r.body["messages"][0]["content"].as_str().unwrap()) + chars(user(r));
            let answer = r.body["max_tokens"].as_u64().unwrap();
            assert!(
                tokens(input) + answer <= 4096 * 9 / 10,
                "{input} + {answer}"
            );
            if user(r).starts_with(PART) {
                assert_eq!(answer, 1228);
            }
        }
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

    /// The built-in route (plan 5.3) against the sidecar tests' fake llama-server.
    mod builtin {
        use super::*;
        use crate::sidecar::tests::{asked, fake, fake_llama};
        use crate::sidecar::{LIMITS, Sidecar};
        use crate::transcribe::engine::Device;

        struct Ran {
            result: Result<(String, Timings), Failure>,
            /// Chat requests the fakes were sent.
            asked: usize,
            log: String,
            prev_log: String,
            /// `summary_gpu_failed` marks.
            marked: usize,
            sidecar: Arc<Sidecar>,
            _tmp: tempfile::TempDir,
        }

        /// One summary on the built-in route, its engine started on the GPU as the queue does, the
        /// server a fake doing `behave`.
        fn builtin(behave: &str, timeout_secs: u32, lines: &[String]) -> Ran {
            let tmp = tempfile::tempdir().unwrap();
            let f = fake(tmp.path(), "supervisor", fake_llama(behave), LIMITS.reap);
            let sidecar = Arc::new(f.sidecar);
            let endpoint = sidecar.start(sidecar.generation(), Device::Gpu).unwrap();
            let connection = LlmConnection {
                base_url: endpoint.base_url.clone(),
                timeout_secs,
                ..crate::settings::builtin()
            };
            let key = Some(endpoint.key.clone());
            let recover = Some(sidecar.on_failure(endpoint));
            let result = block_on(summarize(&connection, key, &[], lines, |_, _| {}, recover));
            let log = |name| fs::read_to_string(tmp.path().join("logs").join(name));
            Ran {
                result,
                asked: asked(&tmp.path().join("models")),
                log: log("llama-server.log").unwrap(),
                prev_log: log("llama-server.prev.log").unwrap_or_default(),
                marked: f.marked.load(Ordering::SeqCst),
                sidecar,
                _tmp: tmp,
            }
        }

        impl Drop for Ran {
            fn drop(&mut self) {
                self.sidecar.stop("테스트 끝");
            }
        }

        /// The engine's key (the fake refuses any other), and every answer's timings added up.
        #[test]
        fn a_summary_is_served_and_its_answers_timings_add_up() {
            let ran = builtin("ready", 30, &talk(80));
            let (summary, timings) = ran.result.as_ref().unwrap();
            assert_eq!(summary, SUMMARY);
            let n = ran.asked as u64;
            assert!(n > 1, "parts and a merge: {n}");
            let each = (120, 300.5, 40, 800.25);
            assert_eq!(
                *timings,
                Timings {
                    prompt_n: each.0 * n,
                    prompt_ms: each.1 * n as f64,
                    predicted_n: each.2 * n,
                    predicted_ms: each.3 * n as f64,
                }
            );
        }

        #[test]
        fn a_server_dying_mid_request_comes_back_on_the_same_device_and_is_asked_again() {
            let ran = builtin("dies-once", 30, &lines(SPEECH));
            assert_eq!(ran.result.as_ref().unwrap().0, SUMMARY);
            assert_eq!(ran.asked, 2);
            // The GPU once more, before any CPU try, and nothing recorded against it.
            assert!(ran.prev_log.contains("-ngl 99") && ran.log.contains("-ngl 99"));
            assert!(!ran.log.contains("--device none"));
            assert_eq!(ran.marked, 0);
        }

        /// ENG6: a slow server is not asked the same again; its own words, not the cloud's.
        #[test]
        fn a_timeout_stops_the_server_and_asks_nothing_again() {
            let started = Instant::now();
            let ran = builtin("stalls", 1, &lines(SPEECH));
            let failure = ran.result.as_ref().unwrap_err();
            assert_eq!(*failure, Failure::Builtin(sidecar::Failure::Timeout));
            assert!(failure.message().contains("요청 하나가 30분을 넘었습니다"));
            assert_eq!(ran.asked, 1);
            assert!(ran.prev_log.is_empty(), "{}", ran.prev_log);
            assert!(started.elapsed() < Duration::from_secs(20));
            let until = Instant::now() + Duration::from_secs(5);
            while ran.sidecar.running() {
                assert!(Instant::now() < until, "still running");
                thread::sleep(Duration::from_millis(10));
            }
        }

        /// A server that answers with an error is alive: no restart, no second request.
        #[test]
        fn an_http_error_is_not_recovered() {
            let ran = builtin("http-500", 30, &lines(SPEECH));
            let failure = ran.result.as_ref().unwrap_err();
            assert_eq!(*failure, Failure::Builtin(sidecar::Failure::Unreadable));
            assert_eq!(ran.asked, 1);
            assert!(ran.sidecar.running() && ran.prev_log.is_empty());
        }

        /// A connection that broke while its server lives on is asked once more on that server:
        /// no restart, no device change.
        #[test]
        fn a_dropped_request_on_a_live_server_is_asked_again_there() {
            let ran = builtin("drops-once", 30, &lines(SPEECH));
            assert_eq!(ran.result.as_ref().unwrap().0, SUMMARY);
            assert_eq!(ran.asked, 2);
            assert!(ran.prev_log.is_empty(), "{}", ran.prev_log);
            assert_eq!(ran.marked, 0);
        }

        /// The one more ask comes back once the server answers: a later broken connection on the
        /// same live server is asked again there too, not taken for a death.
        #[test]
        fn a_server_that_answered_gets_its_one_more_ask_back() {
            let ran = builtin("drops-1-and-3", 30, &talk(80));
            assert_eq!(ran.result.as_ref().unwrap().0, SUMMARY);
            // Dropped, asked again, answered, dropped, asked again, answered (and any more).
            assert!(ran.asked >= 4, "{}", ran.asked);
            assert!(ran.prev_log.is_empty(), "{}", ran.prev_log);
            assert_eq!(ran.marked, 0);
        }

        /// ENG12: our engine refusing an input over its context is the cut input's failure; other
        /// HTTP errors stay "could not make a summary".
        #[test]
        fn an_input_over_the_engines_context_gets_the_truncated_words() {
            let ran = builtin("context-400", 30, &lines(SPEECH));
            let failure = ran.result.as_ref().unwrap_err();
            assert_eq!(*failure, Failure::Builtin(sidecar::Failure::Truncated));
            assert!(failure.message().contains("입력을 다 읽지 못했습니다"));
            assert_eq!(ran.asked, 1);
            assert!(ran.sidecar.running() && ran.prev_log.is_empty());
        }

        #[test]
        fn a_cut_answer_gets_the_length_words() {
            let ran = builtin("length", 30, &lines(SPEECH));
            let failure = ran.result.as_ref().unwrap_err();
            assert_eq!(*failure, Failure::Builtin(sidecar::Failure::Length));
            assert!(
                failure
                    .message()
                    .starts_with("요약이 길이 제한에서 끊겼습니다")
            );
            assert_eq!(ran.asked, 1);
        }

        /// DX5, ENG12: the budget checks stay on the built-in route with words of its own, and no
        /// failure there says anything meant for Ollama, LM Studio, context tokens or the internet.
        #[test]
        fn no_external_words_reach_the_builtin_route() {
            // Too long for any request to hold: refused before one is sent.
            let glossary: Vec<String> = (0..100)
                .map(|i| format!("{i}번 용어 {}", "가".repeat(150)))
                .collect();
            let recover: Recover =
                Box::new(|_, _| Box::pin(async { Err(sidecar::Failure::Stopped) }));
            let builtin = crate::settings::builtin();
            let result = block_on(summarize(
                &builtin,
                None,
                &glossary,
                &lines(SPEECH),
                |_, _| {},
                Some(recover),
            ));
            let failure = result.unwrap_err();
            assert_eq!(failure, Failure::Builtin(sidecar::Failure::GlossaryTooLong));
            assert!(failure.message().contains("용어집"));

            let external = || {
                [
                    Failure::LocalDown,
                    Failure::TooSmall,
                    Failure::Truncated,
                    Failure::Network("connection reset".into()),
                    Failure::Timeout,
                ]
            };
            let words = external().map(|f| f.message());
            let all = external().into_iter().chain([
                Failure::NoKey,
                Failure::Key(401),
                Failure::Quota,
                Failure::Provider("HTTP 500: boom".into()),
                Failure::Length,
                Failure::Truncated.at(2, 3),
            ]);
            for failure in all {
                let shown = engine_failure(failure);
                let message = shown.message();
                assert!(!words.contains(&message), "{message}");
                for word in ["Ollama", "LM Studio", "컨텍스트 토큰", "인터넷"] {
                    assert!(!message.contains(word), "{message}");
                }
                let engine = match shown {
                    Failure::Part { cause, .. } => *cause,
                    shown => shown,
                };
                assert!(matches!(engine, Failure::Builtin(_)), "{engine:?}");
            }
        }
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

    /// The gold rule for an owner: in the transcript as written, or a glossary name.
    fn gold_owner(name: &str, transcript: &str, glossary: &[String]) -> bool {
        transcript.contains(name) || glossary.iter().any(|g| g == name)
    }

    /// ENG11: owners the gold rule accepts that the grounding check would make "미정", so its
    /// relaxed rule's misses show.
    fn false_undecided(summary: &str, transcript: &str, glossary: &[String]) -> usize {
        let heard = grounding::Heard::new(transcript);
        let actions = section(summary, "액션 아이템").unwrap_or_default();
        actions
            .lines()
            .filter_map(owner)
            .flat_map(|o| o.split(grounding::SEPARATORS))
            .map(str::trim)
            .filter(|name| {
                *name != "미정"
                    && gold_owner(name, transcript, glossary)
                    && !grounding::named(name, &heard, glossary)
            })
            .count()
    }

    /// Plan 5.3: a short question's raw answer holds thinking, as `<think>` in its content or as
    /// llama-server's `reasoning_content`.
    fn thinks(base_url: &str, key: &str) -> bool {
        let body = serde_json::json!({
            "messages": [{ "role": "user", "content": "회의록이 무엇인지 한 문장으로 답해 주세요." }],
            "max_tokens": 300,
        });
        let reply = block_on(async {
            let http = reqwest::Client::builder().no_proxy().build().unwrap();
            let request = http
                .post(completions(base_url))
                .bearer_auth(key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_string());
            request.send().await.unwrap().text().await.unwrap()
        });
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        let message = &reply["choices"][0]["message"];
        let content = message["content"].as_str().unwrap_or_default();
        eprintln!("생각 모드 확인 답: {content}");
        content.contains("<think>")
            || message["reasoning_content"]
                .as_str()
                .is_some_and(|r| !r.trim().is_empty())
    }

    /// Runs its closure when dropped, in a panic too.
    struct Defer<F: FnMut()>(F);

    impl<F: FnMut()> Drop for Defer<F> {
        fn drop(&mut self) {
            (self.0)()
        }
    }

    /// llama-server's largest resident memory in MB while `run` runs, by `ps` twice a second (unix
    /// only). Metal's own buffers may not all count there. A panic in `run` still ends the polling,
    /// or the scope would wait for it forever.
    fn peak_mb<T>(engine: &sidecar::Sidecar, run: impl FnOnce() -> T) -> (T, u64) {
        let (done, peak) = (AtomicBool::new(false), AtomicU64::new(0));
        thread::scope(|s| {
            let _done = Defer(|| done.store(true, Ordering::SeqCst));
            s.spawn(|| {
                while !done.load(Ordering::SeqCst) {
                    let pid = sidecar::tests::pid(engine).filter(|_| cfg!(unix));
                    let kb = pid.and_then(|pid| {
                        let ps = std::process::Command::new("ps")
                            .args(["-o", "rss=", "-p", &pid.to_string()])
                            .output()
                            .ok()?;
                        String::from_utf8_lossy(&ps.stdout).trim().parse().ok()
                    });
                    peak.fetch_max(kb.unwrap_or(0), Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(500));
                }
            });
            let result = run();
            (result, peak.load(Ordering::SeqCst) / 1024)
        })
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
                .flat_map(|o| o.split(grounding::SEPARATORS))
            {
                let name = name.trim();
                if name != "미정" && !gold_owner(name, transcript, &expected.glossary) {
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
    /// (one utterance per line) with its `*.expected.json`, and scores the LLM's own text. With
    /// `SUMMARY_EVAL_BUILTIN=1` that LLM is the built-in route's (DX7): a server per case, started
    /// as the app starts it, at the product's 16,384 context unless `SUMMARY_EVAL_CONTEXT_TOKENS`.
    #[test]
    #[ignore]
    fn summary_eval() {
        let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
        let context = var("SUMMARY_EVAL_CONTEXT_TOKENS").map(|v| {
            v.parse::<u32>()
                .expect("SUMMARY_EVAL_CONTEXT_TOKENS는 숫자")
        });
        let file = crate::transcribe::model::SUMMARY.file;
        let (connection, engine) = if var("SUMMARY_EVAL_BUILTIN").as_deref() == Some("1") {
            let model = PathBuf::from(
                var("SUMMARY_EVAL_MODEL_PATH")
                    .unwrap_or_else(|| panic!("SUMMARY_EVAL_MODEL_PATH에 {file} 경로를 주세요")),
            );
            assert!(
                model.ends_with(file) && model.is_file(),
                "SUMMARY_EVAL_MODEL_PATH가 {file} 파일이 아닙니다: {}",
                model.display()
            );
            let context = context.unwrap_or(sidecar::CONTEXT_TOKENS);
            let logs = std::env::temp_dir().join("summary-eval-logs");
            eprintln!("엔진 로그: {}", logs.display());
            let engine = sidecar::tests::engine(model.parent().unwrap().into(), logs, context);
            let connection = LlmConnection {
                context_tokens: context,
                ..crate::settings::builtin()
            };
            (connection, Some(Arc::new(engine)))
        } else {
            let (Some(base_url), Some(model)) =
                (var("SUMMARY_EVAL_BASE_URL"), var("SUMMARY_EVAL_MODEL"))
            else {
                panic!(
                    "SUMMARY_EVAL_BASE_URL과 SUMMARY_EVAL_MODEL을 설정하세요(선택: SUMMARY_EVAL_API_KEY, SUMMARY_EVAL_CONTEXT_TOKENS(기본 128000), SUMMARY_EVAL_LOCAL=1). 이 PC 요약 엔진이면 SUMMARY_EVAL_BUILTIN=1과 SUMMARY_EVAL_MODEL_PATH"
                );
            };
            let is_local = var("SUMMARY_EVAL_LOCAL").as_deref() == Some("1");
            let connection = LlmConnection {
                name: "eval".into(),
                base_url,
                model,
                context_tokens: context.unwrap_or(128_000),
                is_local,
                timeout_secs: if is_local { 900 } else { 300 },
            };
            (connection, None)
        };
        let device = match var("SUMMARY_EVAL_DEVICE").as_deref() {
            Some("cpu") => Device::Cpu,
            _ => Device::Gpu,
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
        let (mut report, mut rows) = (Vec::new(), Vec::new());
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
            let ask = |connection: &LlmConnection, key: Option<String>, recover| {
                let started = Instant::now();
                let on_step = |done, known| eprintln!("{name}: 요청 {done}/{known}");
                let summary = block_on(summarize(
                    connection,
                    key,
                    &expected.glossary,
                    &lines,
                    on_step,
                    recover,
                ));
                (summary, started.elapsed().as_secs_f64())
            };
            let mut row = format!("{name}:");
            let (summary, secs) = match &engine {
                None => ask(&connection, var("SUMMARY_EVAL_API_KEY"), None),
                Some(engine) => {
                    let started = Instant::now();
                    // A panic in a case still stops its server.
                    let stop = Defer(|| engine.stop("평가 끝"));
                    let (asked, peak) = peak_mb(engine, || {
                        let endpoint = engine
                            .start(engine.generation(), device)
                            .unwrap_or_else(|f| panic!("{name}: {}", f.message()));
                        let ready = started.elapsed().as_secs_f64();
                        let thinking = thinks(&endpoint.base_url, &endpoint.key);
                        let connection = LlmConnection {
                            base_url: endpoint.base_url.clone(),
                            ..connection.clone()
                        };
                        let key = Some(endpoint.key.clone());
                        (
                            ready,
                            thinking,
                            ask(&connection, key, Some(engine.on_failure(endpoint))),
                        )
                    });
                    drop(stop);
                    let (ready, thinking, asked) = asked;
                    let peak = match cfg!(unix) {
                        true => format!("{peak}MB"),
                        false => "측정 안 함".into(),
                    };
                    row += &format!(
                        " {} 준비 {ready:.1}초 · 최대 메모리 {peak} · 생각 모드 {} ·",
                        device.name(),
                        if thinking { "켜짐" } else { "꺼짐" }
                    );
                    asked
                }
            };
            row += &format!(" 요약 {secs:.1}초");
            match summary {
                Ok((summary, timings)) => {
                    eprintln!("=== {name} ===\n{summary}\n");
                    let speed = |n: u64, ms: f64| n as f64 / (ms / 1000.0).max(f64::EPSILON);
                    row += &format!(
                        " · 입력 {}토큰 {:.0}토큰/초 · 출력 {}토큰 {:.1}토큰/초",
                        timings.prompt_n,
                        speed(timings.prompt_n, timings.prompt_ms),
                        timings.predicted_n,
                        speed(timings.predicted_n, timings.predicted_ms)
                    );
                    let grounded = grounding::check(&summary, &transcript, &expected.glossary);
                    if grounded.text != summary {
                        eprintln!("=== {name} 근거 검사 뒤 ===\n{}\n", grounded.text);
                    }
                    row += &format!(
                        " · 근거 검사 {} 미정 처리 {}곳, 맞는 담당자를 미정으로 {}명",
                        if grounded.checked { "함" } else { "못 함" },
                        grounded.replaced,
                        false_undecided(&summary, &transcript, &expected.glossary)
                    );
                    let failures = check(&summary, &transcript, &expected);
                    row += &format!(" · 채점 실패 {}개", failures.len());
                    if !failures.is_empty() {
                        report.push(format!("{name}:\n  {}", failures.join("\n  ")));
                    }
                }
                Err(e) => report.push(format!("{name}: 요약 실패: {}", e.message())),
            }
            rows.push(row);
        }
        eprintln!("\n{}", rows.join("\n"));
        assert!(report.is_empty(), "\n{}", report.join("\n"));
    }
}
