//! The built-in summary engine (M6.5 plan 5.3, architecture 6.6): `llama-server` run by a
//! supervisor, which is this executable started with `--llm-supervisor` (dispatched before the app
//! builds, like the transcription worker). The app never starts llama-server itself: the supervisor
//! kills it as soon as its stdin closes, which happens when the app stops it, exits or dies, so no
//! 4 GB process outlives the app. On Windows the supervisor also sits in a kill-on-close Job Object.
//!
//! The supervisor writes one JSON object per line on stdout (`Report`): `{"pid","port":null}` once
//! llama-server runs, `{"pid","port"}` when it says it listens (it does so only with the model
//! loaded, so that is the ready signal), and last `{"exit","signal","oom","spawn_error",
//! "early_exit","stopped"}`. llama-server's own output goes to the supervisor's stderr, which the app
//! opens as `llama-server.log`.
//!
//! `Sidecar` (one per app, owned by the queue) starts and stops it; `Run` decides what follows a
//! death and is tested without processes. The lifecycle tests run this test binary as both the
//! supervisor and a fake llama-server (`tests::this`).

use crate::transcribe::engine::{self, Device};
use crate::transcribe::model;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// The built-in route's context (plan 5.6): llama-server's `-c`, and stage 4's `context_tokens`.
pub const CONTEXT_TOKENS: u32 = 16_384;
const LOG: &str = "llama-server.log";
/// ENG19: the run before, so a GPU's death is still there after the CPU's run starts.
const PREV_LOG: &str = "llama-server.prev.log";
/// One run's output kept in the log: a server looping on a warning stops filling the disk there.
const LOG_CAP: u64 = 5_000_000;
/// ENG5: llama-server ending this soon without a line of output never really ran (antivirus, a
/// missing DLL): "cannot start", not a GPU failure.
const EARLY_EXIT: Duration = Duration::from_secs(2);
/// A server whose request failed gets this long to report how it ended before it is stopped as hung.
const DYING: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(20);
/// Windows has no memory-killer signal (ENG5), so a GPU death there counts only with one of these in
/// its log: ggml-vulkan's lost device and failed device allocation, and vulkan-hpp's error names.
const VULKAN_ERRORS: [&str; 4] = [
    "ggml_vulkan: device lost",
    "failed to allocate Vulkan",
    "ErrorDeviceLost",
    "ErrorOutOfDeviceMemory",
];

/// How long the sidecar waits (plan 5.3). Tests pass shorter ones.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// For the "listening" line: the first load of the 2.7 GB model.
    pub ready: Duration,
    /// For a supervisor told to stop, before the pid fallback (ENG4).
    pub reap: Duration,
}

pub const LIMITS: Limits = Limits {
    ready: Duration::from_secs(120),
    reap: Duration::from_secs(10),
};

/// One line of the supervisor's stdout.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(untagged)]
enum Report {
    Started { pid: u32, port: Option<u16> },
    Ended(Ended),
}

/// How llama-server ended, as the supervisor saw it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
struct Ended {
    exit: Option<i32>,
    signal: Option<i32>,
    /// A SIGKILL the supervisor did not send: the memory killer (unix only).
    oom: bool,
    /// It could not be started at all: missing, quarantined, not executable.
    spawn_error: Option<String>,
    /// Ended within `EARLY_EXIT` without a line of output.
    early_exit: bool,
    /// The supervisor killed it because its stdin closed: our stop, never a memory or GPU failure.
    stopped: bool,
}

/// Why the built-in route could not summarize (plan 5.3, DX5, ENG6, ENG12, ENG16). `message` is
/// the one place its words live; the external connections' (`summarize::Failure`) never show here.
#[derive(Debug, PartialEq)]
pub enum Failure {
    /// A `stop` came after this summary's start: a recording, a delete, quitting. Not the engine's
    /// fault; the queue's own stop decides what the recorder sees.
    Stopped,
    /// llama-server could not run: `spawn_error`, or `early_exit`. No GPU branch (ENG5).
    CannotStart(PathBuf),
    /// It died again after the restarts. `gpu`: the GPU was tried too.
    Crashed {
        gpu: bool,
    },
    OutOfMemory,
    /// A request ran past its limit (ENG6): the server is stopped and nothing is asked again.
    Timeout,
    /// It would not load and the model's hash is wrong; the file is gone for "다시 받기".
    ModelDamaged,
    // ponytail: stage 4 maps the answers' failures to these four; drop their allows then.
    /// `finish_reason: "length"`.
    #[allow(dead_code)]
    Length,
    /// An empty answer, or one that is not a summary.
    #[allow(dead_code)]
    Unreadable,
    /// The glossary leaves no room for the transcript (`TooSmall`).
    #[allow(dead_code)]
    GlossaryTooLong,
    /// The server read less than it was sent (`Truncated`): an engine fault, logged.
    #[allow(dead_code)]
    Truncated,
}

const SEE_LOG: &str = "로그: 설정 → 일반 → 로그 폴더 열기";

impl Failure {
    #[allow(dead_code)] // ponytail: stage 4 shows it.
    pub fn message(&self) -> String {
        match self {
            Failure::Stopped => "요약을 멈췄습니다".into(),
            Failure::CannotStart(exe) => format!(
                "이 PC 요약 엔진을 실행할 수 없습니다. 백신이 {}를 막았는지 확인하고, 회사 PC라면 IT에 이 경로의 예외를 요청해 주세요. {SEE_LOG}",
                exe.display()
            ),
            Failure::Crashed { gpu } => format!(
                "이 PC 요약 엔진이 {} 멈췄습니다. 다른 연결로 다시 실행하거나 로그를 확인해 주세요. {SEE_LOG}",
                if *gpu { "GPU와 CPU에서 모두" } else { "CPU에서" }
            ),
            Failure::OutOfMemory => "메모리가 모자라 요약을 멈췄습니다. 다른 앱을 닫고 다시 실행하거나 다른 연결을 골라 주세요".into(),
            Failure::Timeout => "이 PC에서 요약이 너무 오래 걸립니다(요청 하나가 30분을 넘었습니다). 다른 연결로 다시 실행해 주세요".into(),
            Failure::ModelDamaged => "요약 모델 파일이 손상됐습니다. 설정 → 요약(LLM)에서 다시 받아 주세요".into(),
            Failure::Length => "요약이 길이 제한에서 끊겼습니다. 다시 실행하거나 다른 연결을 골라 주세요".into(),
            Failure::Unreadable => "이 PC 요약 모델이 요약을 만들지 못했습니다. 다시 실행하거나 다른 연결을 골라 주세요".into(),
            Failure::GlossaryTooLong => "용어집이 너무 길어 이 PC에서 요약할 수 없습니다. 설정 → 전사의 용어집을 줄여 주세요".into(),
            Failure::Truncated => format!(
                "이 PC 요약 엔진이 입력을 다 읽지 못했습니다. 다시 실행하거나 로그를 확인해 주세요. {SEE_LOG}"
            ),
        }
    }
}

/// One summary's servers (plan 5.3, ENG3, ENG5): the device, the deaths so far and whether one
/// counts against the GPU. In memory only, like the transcription's `ChunkRun`.
#[derive(Debug)]
struct Run {
    device: Device,
    /// Servers that died serving.
    deaths: u32,
    /// The GPU died of its own fault (`gpu_fault`): a CPU server that comes up records
    /// `summary_gpu_failed`.
    blamed: bool,
    gpu: bool,
    /// The model is hashed once per summary.
    hashed: bool,
}

impl Run {
    fn new(device: Device) -> Self {
        Self {
            device,
            deaths: 0,
            blamed: false,
            gpu: device == Device::Gpu,
            hashed: false,
        }
    }

    /// A server died. `starting`: before it was ready. `fault`: `gpu_fault`. Moves `device` to the
    /// next server's, or says why the summary fails.
    fn after(
        &mut self,
        end: &Ended,
        starting: bool,
        fault: bool,
        damaged: impl FnOnce() -> bool,
    ) -> Result<(), Failure> {
        self.blamed |= fault && self.device == Device::Gpu;
        // A model that will not load may be damaged; hashing 2.7 GB takes seconds, so once, and
        // never for the memory killer.
        if starting && !end.oom && !std::mem::replace(&mut self.hashed, true) && damaged() {
            return Err(Failure::ModelDamaged);
        }
        let next = if starting {
            // A GPU that will not start gets one CPU try: the GPU again would die the same way.
            (self.device == Device::Gpu).then_some(Device::Cpu)
        } else {
            // Died serving: the same device once more, then the CPU.
            self.deaths += 1;
            match (self.device, self.deaths) {
                (device, 1) => Some(device),
                (Device::Gpu, 2) => Some(Device::Cpu),
                _ => None,
            }
        };
        match next {
            Some(device) => {
                self.device = device;
                Ok(())
            }
            None if end.oom => Err(Failure::OutOfMemory),
            None => Err(Failure::Crashed { gpu: self.gpu }),
        }
    }

    /// A server came up. One on the GPU clears an earlier blame: that death was not the GPU's
    /// after all. True for a CPU server after the GPU's own death: record `summary_gpu_failed`.
    fn ready(&mut self) -> bool {
        std::mem::take(&mut self.blamed) && self.device == Device::Cpu
    }
}

/// Counts against the GPU (plan 5.3, ENG5): a death of its own, so neither the memory killer nor a
/// server we stopped (one that never got ready). On Windows only with a Vulkan error in its log.
fn gpu_fault(end: &Ended, log: impl FnOnce() -> Vec<u8>) -> bool {
    !end.oom && !end.stopped && (cfg!(not(windows)) || vulkan_error(&log()))
}

fn vulkan_error(log: &[u8]) -> bool {
    let log = String::from_utf8_lossy(log);
    VULKAN_ERRORS.iter().any(|e| log.contains(e))
}

/// llama-server's "listening on http://127.0.0.1:N" (tools/server/server.cpp at the pinned commit),
/// printed after the model loaded.
fn listening(line: &str) -> Option<u16> {
    let (_, address) = line.split_once("listening on http://")?;
    let (_, port) = address.trim().rsplit_once(':')?;
    port.parse().ok()
}

/// The bundled llama-server: next to this executable, where Tauri puts a `bundle.externalBin` with
/// its target triple dropped (`tauri-build` copies it into `target/debug` for dev builds too).
/// `MS_LLAMA_SERVER` replaces it in debug builds only (ENG17): a release build must not hand
/// transcripts and the key to another executable.
fn server_path(exe: &Path, debug: bool, var: Option<OsString>) -> PathBuf {
    match var.filter(|_| debug) {
        // The supervisor runs it from the models folder: a relative path would point there.
        Some(path) => std::path::absolute(&path).unwrap_or_else(|_| path.into()),
        None => exe.with_file_name(format!("llama-server{}", std::env::consts::EXE_SUFFIX)),
    }
}

/// llama-server's arguments (plan 5.3). `--port 0`: the OS picks a free port, and llama-server
/// names it in its listening line (ENG7). `--reasoning off`: Qwen3.5 thinks by default; the plan's
/// `--chat-template-kwargs` form is deprecated at the pinned commit (stage 1), as is `--no-webui`
/// for `--no-ui`. `-ngl 0` alone still
/// lets large operations go to the GPU backend, hence `--device none --no-op-offload`. The key is
/// not here: every user of the PC can read arguments.
fn server_args(device: Device) -> Vec<String> {
    let context = CONTEXT_TOKENS.to_string();
    let mut args = vec![
        "-m",
        model::SUMMARY.file,
        "-c",
        &context,
        "--host",
        "127.0.0.1",
        "--port",
        "0",
        "--parallel",
        "1",
        "--no-ui",
        "--no-slots",
        "--reasoning",
        "off",
    ];
    match device {
        Device::Gpu => args.extend(["-ngl", "99"]),
        Device::Cpu => args.extend(["-ngl", "0", "--device", "none", "--no-op-offload"]),
    }
    args.into_iter().map(String::from).collect()
}

/// A new key per summary: 32 bytes from the OS generator, as hex.
fn new_key() -> Option<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Windows: no console window for a child, as for the transcription worker.
fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Windows (plan 5.3): the supervisor in a job that ends with the app. The app holds the job's
/// only handle, so when the app ends, however it ends, Windows ends the supervisor and the
/// llama-server it started, even if the supervisor never saw its stdin close.
/// ponytail: assigned just after the spawn; a llama-server started within that instant would sit
/// outside the job (the supervisor still kills it). Spawn suspended if that ever matters.
#[cfg(windows)]
fn contain(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    // A HANDLE is a pointer, which a static cannot hold. 0: no job.
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: no name or security attributes; `limits` outlives the call it is read by.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if job.is_null() || set == 0 {
                0
            } else {
                job as usize
            }
        }
    });
    // SAFETY: the job lives as long as the app, the child's handle as long as `child`.
    if job == 0 || unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) } == 0 {
        log::warn!(
            "요약 엔진 감독을 Job Object에 넣지 못했습니다: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// The pid fallback (ENG4) with the OS's own tool: no FFI for a path only a broken supervisor takes.
fn kill_pid(pid: u32) {
    let pid = pid.to_string();
    let (program, args) = if cfg!(windows) {
        ("taskkill", vec!["/F", "/PID", &pid])
    } else {
        ("kill", vec!["-KILL", &pid])
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = hidden(&mut command).status();
}

fn signal_of(status: ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Polls `ready` until it gives `Some` or `limit` has passed.
fn wait_for<T>(limit: Duration, mut ready: impl FnMut() -> Option<T>) -> Option<T> {
    let since = Instant::now();
    loop {
        if let Some(value) = ready() {
            return Some(value);
        }
        if since.elapsed() >= limit {
            return None;
        }
        thread::sleep(POLL);
    }
}

/// `--llm-supervisor <llama-server> <its arguments…>`: runs it and exits inside; returns for anything
/// else. main.rs calls this before the app builds, so a supervisor opens no window and meets no
/// single-instance check.
pub fn supervisor_main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|a| a == "--llm-supervisor") {
        let mut command = Command::new(args.next().unwrap_or_default());
        command.args(args);
        supervise(command, std::io::stdin());
    }
}

/// Runs llama-server until it ends or `stdin` closes, then kills it at once (there is no state to
/// save), writes the last line and exits.
fn supervise(mut command: Command, mut stdin: impl Read + Send + 'static) -> ! {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Windows: a missing or quarantined vulkan-1.dll, or a crash, ends llama-server (it inherits
    // this mode) instead of opening a dialog that nobody sees behind CREATE_NO_WINDOW.
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
        };
        // SAFETY: sets this process's error mode and nothing else.
        unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) };
    }
    let since = Instant::now();
    let mut server = match hidden(&mut command).spawn() {
        Ok(server) => server,
        Err(e) => finish(Ended {
            spawn_error: Some(e.to_string()),
            ..Ended::default()
        }),
    };
    let pid = server.id();
    engine::print(&Report::Started { pid, port: None });
    let said = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicU64::new(0));
    let pipes: [Option<Box<dyn Read + Send>>; 2] = [
        server.stdout.take().map(|p| Box::new(p) as _),
        server.stderr.take().map(|p| Box::new(p) as _),
    ];
    let relays: Vec<_> = pipes
        .into_iter()
        .flatten()
        .map(|pipe| {
            let (said, written) = (said.clone(), written.clone());
            thread::spawn(move || relay(pipe, pid, &said, &written))
        })
        .collect();
    let (stop, stopping) = mpsc::channel();
    thread::spawn(move || {
        // The app never writes: any return is the pipe closing.
        let _ = stdin.read(&mut [0; 1]);
        let _ = stop.send(());
    });
    // `killed`: by us, so a SIGKILL in its status is not the memory killer's.
    let (status, stopped, killed) = loop {
        match server.try_wait() {
            Ok(Some(status)) => break (Some(status), false, false),
            Ok(None) => {}
            // It can no longer be watched: ended like our own stop, not left running unseen and not
            // blamed on the GPU.
            Err(_) => {
                let _ = server.kill();
                break (server.wait().ok(), true, true);
            }
        }
        if !matches!(stopping.recv_timeout(POLL), Err(RecvTimeoutError::Timeout)) {
            let _ = server.kill();
            break (server.wait().ok(), true, true);
        }
    };
    for relay in relays {
        let _ = relay.join();
    }
    let signal = status.and_then(signal_of);
    finish(Ended {
        exit: status.and_then(|s| s.code()),
        signal,
        oom: !killed && signal == Some(9),
        spawn_error: None,
        early_exit: !killed && since.elapsed() < EARLY_EXIT && !said.load(Ordering::SeqCst),
        stopped,
    })
}

/// One of llama-server's pipes into our stderr (`llama-server.log`), `LOG_CAP` in all; the
/// listening line also goes to the app.
fn relay(pipe: impl Read, pid: u32, said: &AtomicBool, written: &AtomicU64) {
    let mut reader = BufReader::new(pipe);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
        said.store(true, Ordering::SeqCst);
        if written.fetch_add(line.len() as u64, Ordering::SeqCst) < LOG_CAP {
            let _ = std::io::stderr().write_all(&line);
        }
        if let Some(port) = listening(&String::from_utf8_lossy(&line)) {
            engine::print(&Report::Started {
                pid,
                port: Some(port),
            });
        }
        line.clear();
    }
}

fn finish(ended: Ended) -> ! {
    engine::print(&Report::Ended(ended));
    std::process::exit(0)
}

/// A summary's hold on the engine: where it serves, with which key, and what has died so far.
pub struct Endpoint {
    /// `http://127.0.0.1:{port}/v1`: a new port after every restart.
    pub base_url: String,
    /// `LLAMA_API_KEY`, the same for every server of the summary.
    pub key: String,
    generation: u64,
    run: Run,
}

impl std::fmt::Debug for Endpoint {
    /// Never the key, in a log line or a panic message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("base_url", &self.base_url)
            .field("key", &"***")
            .field("generation", &self.generation)
            .field("run", &self.run)
            .finish()
    }
}

/// What the supervisor has said so far.
#[derive(Default)]
struct Seen {
    pid: Option<u32>,
    port: Option<u16>,
    end: Option<Ended>,
}

fn read_reports(stdout: ChildStdout, seen: &Mutex<Seen>) {
    engine::read_lines(stdout, |report| match report {
        Some(Report::Started { pid, port }) => {
            let mut seen = lock(seen);
            seen.pid = Some(pid);
            seen.port = seen.port.or(port);
        }
        Some(Report::Ended(end)) => lock(seen).end = Some(end),
        None => {}
    });
    // A supervisor gone without its last line (crashed, or killed by the pid fallback): a death.
    lock(seen).end.get_or_insert_with(Ended::default);
}

/// One supervisor and its llama-server.
struct Server {
    supervisor: Child,
    /// Closing it stops the server (ENG4).
    stdin: Option<ChildStdin>,
    seen: Arc<Mutex<Seen>>,
    device: Device,
    since: Instant,
}

#[derive(Default)]
struct State {
    /// Bumped by every `stop`: a start or restart begun before comes back `Stopped` (plan 5.3).
    generation: u64,
    server: Option<Server>,
}

enum Up {
    Ready(u16),
    Died(Ended),
    Stopped,
}

/// The engine, one per app (owned by the queue). `start` and `recover` block for as long as a model
/// loads, so stage 4 calls them from threads that may block: `start` on the queue's process thread
/// in `summarize()` before `request()` (a plain std thread), `recover` from the async `on_failure`
/// hook only through `tauri::async_runtime::spawn_blocking`. Never directly in the async task: it
/// would hold a runtime worker for minutes. `stop` never blocks and is safe under the queue's lock.
/// One `start` or `recover` at a time: stage 4 holds the queue's builtin lease (plan 5.5) around them.
pub struct Sidecar {
    /// This executable (tests: with arguments before `--llm-supervisor`).
    supervisor: Vec<OsString>,
    /// llama-server (tests: a fake, with arguments of its own).
    server: Vec<OsString>,
    /// llama-server runs here with `-m <file name>`: arguments go through the ANSI code page on
    /// Windows, so a Korean user folder in the path would not open.
    models: PathBuf,
    /// `app_log_dir()`: `llama-server.log` and `.prev.log`. `None` (no home folder): no log.
    logs: Option<PathBuf>,
    limits: Limits,
    /// The model would not load: hashes it, and a damaged file goes (`Models::check`).
    damaged: Box<dyn Fn() -> bool + Send + Sync>,
    /// Records `summary_gpu_failed`.
    gpu_failed: Box<dyn Fn() + Send + Sync>,
    state: Mutex<State>,
    /// Supervisors told to stop and not yet gone: the next server waits for them (ENG4).
    stopping: Arc<AtomicUsize>,
}

impl Sidecar {
    pub fn new(
        exe: &Path,
        models: PathBuf,
        logs: Option<PathBuf>,
        damaged: impl Fn() -> bool + Send + Sync + 'static,
        gpu_failed: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let server = server_path(
            exe,
            cfg!(debug_assertions),
            std::env::var_os("MS_LLAMA_SERVER"),
        );
        Self {
            supervisor: vec![exe.into()],
            server: vec![server.into()],
            models,
            logs,
            limits: LIMITS,
            damaged: Box::new(damaged),
            gpu_failed: Box::new(gpu_failed),
            state: Mutex::default(),
            stopping: Arc::default(),
        }
    }

    /// Stops the engine (plan 5.3): the generation goes up, so a start or restart in flight comes
    /// back `Stopped`, and the supervisor's stdin closes, so it kills llama-server. Never waits:
    /// winding down happens on a thread of its own and the next start waits for it, so this is safe
    /// under the queue's lock.
    pub fn stop(&self, why: &str) {
        let mut state = lock(&self.state);
        state.generation += 1;
        self.retire(&mut state, why);
    }

    /// A server runs or winds down, holding the summary model open (ENG18).
    pub fn running(&self) -> bool {
        lock(&self.state).server.is_some() || self.stopping.load(Ordering::SeqCst) > 0
    }

    /// Closes the server's stdin and leaves its end to a thread: the supervisor gets `reap` to exit,
    /// then its llama-server is killed by pid first, so it is not orphaned, and then it.
    fn retire(&self, state: &mut State, why: &str) {
        let Some(mut server) = state.server.take() else {
            return;
        };
        drop(server.stdin.take());
        if lock(&server.seen).end.is_none() {
            log::info!(
                "요약 엔진 끄기: {why} ({}, {}초 실행)",
                server.device.name(),
                server.since.elapsed().as_secs()
            );
        }
        self.stopping.fetch_add(1, Ordering::SeqCst);
        let (stopping, reap) = (self.stopping.clone(), self.limits.reap);
        thread::spawn(move || {
            // ponytail: the pid is safe to kill only because a hung supervisor still holds
            // llama-server unreaped (its zombie on unix, its handle on Windows). A supervisor that
            // died on its own is reaped here with no fallback, and on Windows its llama-server keeps
            // the supervisor's inherited pipe handles, so the app reads no end of its stdout until
            // both are gone; the Job Object still ends them with the app. Hold a process handle
            // (pidfd, OpenProcess) for llama-server if that ever bites.
            if wait_for(reap, || server.supervisor.try_wait().ok().flatten()).is_none() {
                let pid = lock(&server.seen).pid;
                log::warn!(
                    "요약 엔진 감독이 끝나지 않아 강제로 정리합니다 (llama-server pid {pid:?})"
                );
                if let Some(pid) = pid {
                    kill_pid(pid);
                }
                let _ = server.supervisor.kill();
                let _ = server.supervisor.wait();
            }
            stopping.fetch_sub(1, Ordering::SeqCst);
        });
    }

    /// `llama-server.log` for a new run, the last one moved to `.prev.log` (ENG19), with the
    /// arguments as its first line like `worker.log`. Without it the server still runs.
    fn open_log(&self, args: &[String]) -> Stdio {
        let Some(logs) = &self.logs else {
            return Stdio::null();
        };
        let path = logs.join(LOG);
        let _ = fs::create_dir_all(logs);
        let _ = fs::rename(&path, logs.join(PREV_LOG));
        match File::create(&path) {
            Ok(mut file) => {
                let _ = writeln!(file, "--- {} ---", args.join(" "));
                file.into()
            }
            Err(e) => {
                log::warn!("{LOG}를 열 수 없습니다: {e}");
                Stdio::null()
            }
        }
    }

    fn command(&self, device: Device, key: &str) -> Command {
        let args = server_args(device);
        let mut command = Command::new(&self.supervisor[0]);
        command
            .args(&self.supervisor[1..])
            .arg("--llm-supervisor")
            .args(&self.server)
            .args(&args)
            .current_dir(&self.models)
            .env("LLAMA_API_KEY", key)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(self.open_log(&args));
        hidden(&mut command);
        command
    }

    /// One server on `device`, up to its listening line and one `/health`.
    fn launch(&self, generation: u64, device: Device, key: &str, why: &str) -> Up {
        // ENG4: never two models in memory at once.
        while self.stopping.load(Ordering::SeqCst) > 0 {
            thread::sleep(POLL);
        }
        if lock(&self.state).generation != generation {
            return Up::Stopped;
        }
        // The log file and the spawn happen outside the lock: a recording's `stop` takes it while
        // holding the queue's.
        let seen = Arc::new(Mutex::new(Seen::default()));
        let since = Instant::now();
        let mut supervisor = match self.command(device, key).spawn() {
            Ok(child) => child,
            Err(e) => {
                return Up::Died(Ended {
                    spawn_error: Some(e.to_string()),
                    ..Ended::default()
                });
            }
        };
        #[cfg(windows)]
        contain(&supervisor);
        if let Some(stdout) = supervisor.stdout.take() {
            let seen = seen.clone();
            thread::spawn(move || read_reports(stdout, &seen));
        }
        let server = Server {
            stdin: supervisor.stdin.take(),
            supervisor,
            seen: seen.clone(),
            device,
            since,
        };
        {
            // A stop never misses this server: one before this check bumped the generation, and
            // the server goes here; one after it finds the server in `state` and retires it.
            let mut state = lock(&self.state);
            let stopped = state.generation != generation;
            // One start or restart at a time (the queue's builtin lease): a second one is a bug, and
            // the server it would orphan is retired, not dropped unreaped.
            debug_assert!(state.server.is_none(), "two engine starts at once");
            self.retire(&mut state, "겹친 시작");
            state.server = Some(server);
            if stopped {
                self.retire(&mut state, "띄우는 사이에 끄기");
                return Up::Stopped;
            }
        }
        log::info!("요약 엔진 시작: {why} ({})", device.name());
        wait_for(self.limits.ready, || {
            let stopped = lock(&self.state).generation != generation;
            let seen = lock(&seen);
            (stopped || seen.port.is_some() || seen.end.is_some()).then_some(())
        });
        let port = {
            let seen = lock(&seen);
            seen.port.filter(|_| seen.end.is_none())
        };
        // The stop that bumped the generation closed this server's stdin already.
        if lock(&self.state).generation != generation {
            return Up::Stopped;
        }
        let serving = port.filter(|&port| healthy(port, key));
        let mut state = lock(&self.state);
        if state.generation != generation {
            return Up::Stopped;
        }
        if let Some(port) = serving {
            drop(state);
            log::info!(
                "요약 엔진 준비: {}, {}초",
                device.name(),
                since.elapsed().as_secs()
            );
            return Up::Ready(port);
        }
        // Ended, silent past the limit, or not answering: gone either way.
        self.retire(&mut state, "준비되지 않음");
        drop(state);
        Up::Died(wait_for(self.limits.reap * 2, || lock(&seen).end.clone()).unwrap_or_default())
    }

    /// Servers until one is ready or `Run` gives up.
    fn serve(&self, endpoint: &mut Endpoint, mut why: &str) -> Result<(), Failure> {
        loop {
            let device = endpoint.run.device;
            match self.launch(endpoint.generation, device, &endpoint.key, why) {
                Up::Ready(port) => {
                    endpoint.base_url = format!("http://127.0.0.1:{port}/v1");
                    // The GPU's own death, and now the CPU works (plan 5.3 (4)).
                    if endpoint.run.ready() {
                        (self.gpu_failed)();
                    }
                    return Ok(());
                }
                Up::Stopped => return Err(Failure::Stopped),
                Up::Died(end) => self.next(&mut endpoint.run, &end, true)?,
            }
            why = "앞 엔진이 시작하다 끝남";
        }
    }

    fn next(&self, run: &mut Run, end: &Ended, starting: bool) -> Result<(), Failure> {
        log::warn!(
            "요약 엔진이 끝났습니다 ({}, {}): {end:?}",
            run.device.name(),
            if starting { "시작 중" } else { "요약 중" }
        );
        if end.spawn_error.is_some() || end.early_exit {
            return Err(Failure::CannotStart(self.server[0].clone().into()));
        }
        let log = self.logs.as_ref().map(|logs| logs.join(LOG));
        let fault = gpu_fault(end, || {
            log.and_then(|log| fs::read(log).ok()).unwrap_or_default()
        });
        run.after(end, starting, fault, || (self.damaged)())
    }
}

// ponytail: stage 4's built-in route (summarize) calls these; drop this allow then.
#[allow(dead_code)]
impl Sidecar {
    /// Read under the queue's lock together with the job's stop, before `start`: a `stop` after
    /// that is never missed.
    pub fn generation(&self) -> u64 {
        lock(&self.state).generation
    }

    /// A summary's first server (plan 5.3), on `device` with a new key; a GPU that dies loading
    /// gets one CPU try. `Stopped` once a `stop` came after `generation`, so no server comes up in a
    /// recording that began meanwhile. Blocking, up to `Limits::ready` per server plus winding down
    /// the one before: stage 4 runs it off the async runtime's workers.
    pub fn start(&self, generation: u64, device: Device) -> Result<Endpoint, Failure> {
        {
            let mut state = lock(&self.state);
            if state.generation != generation {
                return Err(Failure::Stopped);
            }
            // One summary at a time (the queue's lease): a server left from the last one goes.
            self.retire(&mut state, "새 요약");
        }
        let key = new_key().ok_or_else(|| Failure::CannotStart(self.server[0].clone().into()))?;
        let mut endpoint = Endpoint {
            base_url: String::new(),
            key,
            generation,
            run: Run::new(device),
        };
        self.serve(&mut endpoint, "요약 시작")?;
        Ok(endpoint)
    }

    /// A request to the server failed (ENG3, ENG6). Stage 4 calls it only for transport errors
    /// (refused, reset, early EOF) and timeouts, never for an HTTP status: a server that answers
    /// with an error is alive and its answer is the failure. A timeout stops the server and nothing
    /// is asked again; anything else is a death, and the next server comes up on the device `Run`
    /// picks, with the same key. Returns its address, for the request to be sent again. `Stopped`
    /// once a `stop` came after the summary's start. Blocking, like `start`.
    pub fn recover(&self, endpoint: &mut Endpoint, timed_out: bool) -> Result<String, Failure> {
        let seen = {
            let mut state = lock(&self.state);
            if state.generation != endpoint.generation {
                return Err(Failure::Stopped);
            }
            if timed_out {
                self.retire(&mut state, "요청 시간 제한");
                return Err(Failure::Timeout);
            }
            state.server.as_ref().map(|s| s.seen.clone())
        };
        let end = match seen {
            Some(seen) => {
                wait_for(DYING, || lock(&seen).end.clone());
                {
                    let mut state = lock(&self.state);
                    if state.generation != endpoint.generation {
                        return Err(Failure::Stopped);
                    }
                    // Dead already, or alive and not serving: stopped either way.
                    self.retire(&mut state, "요청 실패");
                }
                wait_for(self.limits.reap * 2, || lock(&seen).end.clone()).unwrap_or_default()
            }
            None => Ended::default(),
        };
        self.next(&mut endpoint.run, &end, false)?;
        self.serve(endpoint, "요약 중 끝난 엔진 다시")?;
        Ok(endpoint.base_url.clone())
    }
}

/// One `GET /health`: the server answers. llama-server leaves that endpoint open without a key
/// (server-http.cpp), so the key sent along proves nothing yet; the first request does.
fn healthy(port: u16, key: &str) -> bool {
    let address = (Ipv4Addr::LOCALHOST, port).into();
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_secs(2)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let request = format!(
        "GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {key}\r\nConnection: close\r\n\r\n"
    );
    let mut head = [0; 12];
    stream.write_all(request.as_bytes()).is_ok()
        && stream.read_exact(&mut head).is_ok()
        && head == *b"HTTP/1.1 200"
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;

    /// This test binary running `entry`, one of the ignored tests below (ENG8). One Rust fake for
    /// both OSes, and nothing extra built or bundled. libtest needs three things here: it takes
    /// every argument after `--` as one more filter and runs any test one of them names (the
    /// supervisor's arguments name `fake_server`), so the other entries are skipped, and each
    /// checks `role` and returns in any other process as well; and with one test thread its
    /// default formatter prints "test <name> ... " with no newline before the test runs, which
    /// would join the first JSON line, so `-q`, whose formatter prints nothing until a test ends.
    fn this(entry: &str, rest: &[&str]) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![std::env::current_exe().unwrap().into()];
        let name = |e: &str| format!("sidecar::tests::{e}");
        args.extend([name(entry), "--exact".into(), "--ignored".into()].map(OsString::from));
        args.extend(["--nocapture", "-q"].map(OsString::from));
        for other in ["supervisor", "deaf_supervisor", "fake_server"] {
            if other != entry {
                args.extend(["--skip".into(), name(other)].map(OsString::from));
            }
        }
        args.push("--".into());
        args.extend(rest.iter().map(OsString::from));
        args
    }

    /// This process was started as `entry`: its first argument names it.
    fn role(entry: &str) -> bool {
        std::env::args().nth(1) == Some(format!("sidecar::tests::{entry}"))
    }

    /// The fake llama-server doing `behave` (`fake_server`).
    pub(crate) fn fake_llama(behave: &str) -> Vec<OsString> {
        this("fake_server", &[behave])
    }

    pub(crate) struct Fake {
        pub(crate) sidecar: Sidecar,
        /// Calls of `damaged` and of `gpu_failed`.
        pub(crate) hashed: Arc<AtomicUsize>,
        pub(crate) marked: Arc<AtomicUsize>,
        logs: PathBuf,
    }

    /// A sidecar in `dir` whose supervisor is the `supervisor` (or `deaf_supervisor`) entry and whose
    /// llama-server is `server`. The model is never found damaged.
    pub(crate) fn fake(
        dir: &Path,
        supervisor: &str,
        server: Vec<OsString>,
        reap: Duration,
    ) -> Fake {
        let (hashed, marked) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (hashes, marks) = (hashed.clone(), marked.clone());
        let logs = dir.join("logs");
        fs::create_dir_all(dir.join("models")).unwrap();
        let sidecar = Sidecar {
            supervisor: this(supervisor, &[]),
            server,
            models: dir.join("models"),
            logs: Some(logs.clone()),
            limits: Limits {
                // The test binary starts twice per server: generous for a busy machine.
                ready: Duration::from_secs(20),
                reap,
            },
            damaged: Box::new(move || {
                hashes.fetch_add(1, Ordering::SeqCst);
                false
            }),
            gpu_failed: Box::new(move || {
                marks.fetch_add(1, Ordering::SeqCst);
            }),
            state: Mutex::default(),
            stopping: Arc::default(),
        };
        Fake {
            sidecar,
            hashed,
            marked,
            logs,
        }
    }

    impl Fake {
        fn log(&self, name: &str) -> String {
            fs::read_to_string(self.logs.join(name)).unwrap_or_default()
        }

        fn pid(&self) -> u32 {
            let state = lock(&self.sidecar.state);
            let seen = state.server.as_ref().unwrap().seen.clone();
            drop(state);
            lock(&seen).pid.unwrap()
        }

        fn seen(&self) -> Arc<Mutex<Seen>> {
            lock(&self.sidecar.state)
                .server
                .as_ref()
                .unwrap()
                .seen
                .clone()
        }
    }

    pub(crate) fn port(base_url: &str) -> u16 {
        let url = base_url.strip_prefix("http://127.0.0.1:").unwrap();
        url.strip_suffix("/v1").unwrap().parse().unwrap()
    }

    fn get(port: u16, path: &str, key: &str) -> Option<String> {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nAuthorization: Bearer {key}\r\nConnection: close\r\n\r\n"
        )
        .ok()?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply).ok()?;
        Some(reply)
    }

    /// A llama-server's port refuses connections: it is gone.
    pub(crate) fn gone(port: u16) -> bool {
        let address = (Ipv4Addr::LOCALHOST, port).into();
        wait_for(Duration::from_secs(5), || {
            TcpStream::connect_timeout(&address, Duration::from_millis(500))
                .is_err()
                .then_some(())
        })
        .is_some()
    }

    /// Kills the fake mid-generation: it aborts, which is neither our stop nor the memory killer.
    fn crash(port: u16, key: &str) {
        let _ = get(port, "/die", key);
    }

    /// The supervisor (`--llm-supervisor` and what follows, as the app passes it).
    #[test]
    #[ignore = "run by the sidecar tests as the supervisor"]
    fn supervisor() {
        if let Some(command) = after_flag().filter(|_| role("supervisor")) {
            supervise(command, std::io::stdin());
        }
    }

    /// A broken supervisor that never notices its stdin close (ENG4's pid fallback).
    #[test]
    #[ignore = "run by the sidecar tests as a supervisor that ignores its stdin"]
    fn deaf_supervisor() {
        struct Deaf;
        impl Read for Deaf {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                loop {
                    thread::park();
                }
            }
        }
        if let Some(command) = after_flag().filter(|_| role("deaf_supervisor")) {
            supervise(command, Deaf);
        }
    }

    fn after_flag() -> Option<Command> {
        let args: Vec<OsString> = std::env::args_os()
            .skip_while(|a| a != "--llm-supervisor")
            .skip(1)
            .collect();
        let (server, rest) = args.split_first()?;
        let mut command = Command::new(server);
        command.args(rest);
        Some(command)
    }

    /// A fake llama-server, `behave` first among its arguments: `ready` listens and serves
    /// `/health` with the key from `LLAMA_API_KEY` (never one found in its arguments) and `/die`,
    /// which aborts; `slow` says it listens after 3 s; `gpu-dies` dies loading unless on the CPU;
    /// `silent` never says it listens. It names its arguments and its port on stderr.
    #[test]
    #[ignore = "run by the sidecar tests as a fake llama-server"]
    fn fake_server() {
        if !role("fake_server") {
            return;
        }
        let args: Vec<String> = std::env::args().skip_while(|a| a != "--").skip(1).collect();
        let Some((behave, args)) = args.split_first() else {
            return;
        };
        let key = std::env::var("LLAMA_API_KEY").unwrap_or_default();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        eprintln!("fake: {behave} on {port}, args {}", args.join(" "));
        let cpu = args.iter().any(|a| a == "none");
        match behave.as_str() {
            "gpu-dies" if !cpu => std::process::abort(),
            "slow" => thread::sleep(Duration::from_secs(3)),
            "silent" => loop {
                thread::park();
            },
            _ => {}
        }
        // The pinned llama-server's own words.
        eprintln!("srv          main: listening on http://127.0.0.1:{port}");
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut buf = [0; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(n) if n > 0 => head.extend_from_slice(&buf[..n]),
                    _ => break,
                }
            }
            let head = String::from_utf8_lossy(&head);
            if head.starts_with("GET /die") {
                std::process::abort();
            }
            let ours = !key.is_empty()
                && head.contains(&format!("Authorization: Bearer {key}"))
                && !args.iter().any(|a| a.contains(&key));
            let status = if head.starts_with("GET /health") && ours {
                "200 OK"
            } else {
                "401 Unauthorized"
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    }

    fn ended(f: impl FnOnce(&mut Ended)) -> Ended {
        let mut end = Ended::default();
        f(&mut end);
        end
    }

    #[test]
    fn a_death_serving_restarts_on_the_same_device_once_then_the_cpu() {
        let crash = Ended::default();
        let mut run = Run::new(Device::Gpu);
        assert_eq!(run.after(&crash, false, true, || panic!("hashed")), Ok(()));
        assert_eq!(run.device, Device::Gpu);
        assert_eq!(run.after(&crash, false, true, || panic!("hashed")), Ok(()));
        assert_eq!((run.device, run.blamed), (Device::Cpu, true));
        assert_eq!(
            run.after(&crash, false, false, || false),
            Err(Failure::Crashed { gpu: true })
        );

        // Started on the CPU: once more there, then it fails, and never says GPU.
        let mut run = Run::new(Device::Cpu);
        assert_eq!(run.after(&crash, false, true, || false), Ok(()));
        assert_eq!(
            run.after(&crash, false, true, || false),
            Err(Failure::Crashed { gpu: false })
        );
        assert!(!run.blamed);
    }

    #[test]
    fn a_gpu_that_serves_again_is_not_blamed_for_a_later_memory_death() {
        let crash = Ended::default();
        let oom = ended(|e| e.oom = true);
        let mut run = Run::new(Device::Gpu);
        assert_eq!(run.after(&crash, false, true, || false), Ok(()));
        // The GPU came back and served: that death was not the GPU's.
        assert!(!run.ready());
        assert_eq!(run.after(&oom, false, false, || false), Ok(()));
        assert_eq!(run.device, Device::Cpu);
        assert!(!run.ready());

        // Without the GPU coming back, the CPU coming up records it, once.
        let mut run = Run::new(Device::Gpu);
        run.after(&crash, false, true, || false).unwrap();
        run.after(&crash, false, true, || false).unwrap();
        assert!(run.ready());
        assert!(!run.ready());
    }

    #[test]
    fn a_gpu_that_dies_loading_gets_one_cpu_try_after_the_model_is_hashed() {
        let crash = Ended::default();
        let hashes = AtomicUsize::new(0);
        let hash = |damaged| {
            hashes.fetch_add(1, Ordering::SeqCst);
            damaged
        };
        let mut run = Run::new(Device::Gpu);
        assert_eq!(run.after(&crash, true, true, || hash(false)), Ok(()));
        assert_eq!((run.device, run.blamed), (Device::Cpu, true));
        // Hashed once per summary.
        assert_eq!(
            run.after(&crash, true, false, || hash(false)),
            Err(Failure::Crashed { gpu: true })
        );
        assert_eq!(hashes.load(Ordering::SeqCst), 1);

        let mut run = Run::new(Device::Gpu);
        assert_eq!(
            run.after(&crash, true, true, || true),
            Err(Failure::ModelDamaged)
        );
    }

    #[test]
    fn the_memory_killer_and_our_own_stops_never_blame_the_gpu() {
        let oom = ended(|e| {
            e.oom = true;
            e.signal = Some(9);
        });
        let stopped = ended(|e| e.stopped = true);
        assert!(!gpu_fault(&oom, || panic!("no log needed")));
        assert!(!gpu_fault(&stopped, || panic!("no log needed")));
        // A death of its own: on Windows only with a Vulkan error in the log.
        let crash = Ended::default();
        assert_eq!(gpu_fault(&crash, Vec::new), cfg!(not(windows)));
        assert!(gpu_fault(&crash, || {
            b"ggml_vulkan: device lost on Intel(R) UHD Graphics\n".to_vec()
        }));

        // The memory killer is not hashed for, and ends the summary with its own words.
        let mut run = Run::new(Device::Gpu);
        assert_eq!(run.after(&oom, true, false, || panic!("hashed")), Ok(()));
        assert_eq!(run.after(&oom, false, false, || false), Ok(()));
        assert_eq!(
            run.after(&oom, false, false, || false),
            Err(Failure::OutOfMemory)
        );
        assert!(!run.blamed);
    }

    #[test]
    fn vulkan_errors_are_read_from_the_log() {
        for line in [
            "ggml_vulkan: device lost on AMD Radeon(TM) Graphics, likely caused by previous submission",
            "ggml_backend_alloc_ctx_tensors_from_buft: failed to allocate Vulkan0 buffer of size 2147483648",
            "terminate called after throwing an instance of 'vk::DeviceLostError' what(): vk::Queue::submit: ErrorDeviceLost",
            "vk::Device::allocateMemory: ErrorOutOfDeviceMemory",
        ] {
            assert!(vulkan_error(line.as_bytes()), "{line}");
        }
        assert!(!vulkan_error(
            b"llama_model_load: error loading model: out of memory"
        ));
        assert!(!vulkan_error(b""));
    }

    #[test]
    fn the_listening_line_names_the_port() {
        assert_eq!(
            listening("srv          main: listening on http://127.0.0.1:52341\n"),
            Some(52341)
        );
        assert_eq!(
            listening("main: listening on http://127.0.0.1:8080\r\n"),
            Some(8080)
        );
        assert_eq!(
            listening("srv  load_model: loading model 'Qwen3.5-4B-Q4_K_M.gguf'"),
            None
        );
        assert_eq!(listening("listening on unix:///tmp/x.sock"), None);
    }

    #[test]
    fn supervisor_lines_read_as_documented() {
        let started: Report = serde_json::from_str(r#"{"pid":7,"port":null}"#).unwrap();
        assert_eq!(started, Report::Started { pid: 7, port: None });
        let ready: Report = serde_json::from_str(r#"{"pid":7,"port":52341}"#).unwrap();
        assert_eq!(
            ready,
            Report::Started {
                pid: 7,
                port: Some(52341)
            }
        );
        let end = Report::Ended(ended(|e| {
            e.signal = Some(9);
            e.oom = true;
        }));
        let line = r#"{"exit":null,"signal":9,"oom":true,"spawn_error":null,"early_exit":false,"stopped":false}"#;
        assert_eq!(serde_json::to_string(&end).unwrap(), line);
        assert_eq!(serde_json::from_str::<Report>(line).unwrap(), end);
    }

    #[test]
    fn the_server_is_the_bundled_one_and_the_override_is_for_debug_builds_only() {
        let exe = Path::new("/Applications/회의 요약.app/Contents/MacOS/meeting-summarizer");
        let bundled = exe.with_file_name(format!("llama-server{}", std::env::consts::EXE_SUFFIX));
        let other = Some(OsString::from("/opt/homebrew/bin/llama-server"));
        assert_eq!(server_path(exe, true, None), bundled);
        assert_eq!(server_path(exe, false, other.clone()), bundled);
        assert_eq!(
            server_path(exe, true, other),
            PathBuf::from("/opt/homebrew/bin/llama-server")
        );
        // Relative to where the app runs, not to the models folder the supervisor runs in.
        let relative = server_path(exe, true, Some("fake/llama-server".into()));
        assert!(relative.is_absolute() && relative.ends_with("fake/llama-server"));
    }

    #[test]
    fn the_key_goes_in_the_environment_never_in_arguments() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fake(tmp.path(), "supervisor", fake_llama("ready"), LIMITS.reap);
        let command = f.sidecar.command(Device::Cpu, "sk-secret");
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
        assert!(!args.iter().any(|a| a.contains("sk-secret")), "{args:?}");
        for flag in [
            "--port",
            "--reasoning",
            "--no-slots",
            "--no-op-offload",
            "-m",
        ] {
            assert!(args.iter().any(|a| a == flag), "{flag}: {args:?}");
        }
        assert!(
            command
                .get_envs()
                .any(|(k, v)| k == "LLAMA_API_KEY" && v == Some(OsStr::new("sk-secret")))
        );
        assert_eq!(
            command.get_current_dir(),
            Some(tmp.path().join("models").as_path())
        );
    }

    #[test]
    fn every_builtin_failure_says_what_to_do_in_words_of_its_own() {
        use crate::summarize::Failure as External;
        let exe = PathBuf::from(r"C:\Program Files\회의 요약\llama-server.exe");
        let all = [
            Failure::Stopped,
            Failure::CannotStart(exe.clone()),
            Failure::Crashed { gpu: true },
            Failure::Crashed { gpu: false },
            Failure::OutOfMemory,
            Failure::Timeout,
            Failure::ModelDamaged,
            Failure::Length,
            Failure::Unreadable,
            Failure::GlossaryTooLong,
            Failure::Truncated,
        ];
        // DX5: Ollama, LM Studio and context-token advice does not fit the built-in engine.
        let external = [
            External::LocalDown,
            External::TooSmall,
            External::Truncated,
            External::Network("connection reset".into()),
            External::Timeout,
        ]
        .map(|f| f.message());
        let messages: Vec<String> = all.iter().map(Failure::message).collect();
        for m in &messages {
            assert!(!m.is_empty() && !m.contains('\u{2014}'), "{m}");
            assert!(!external.contains(m), "{m}");
            for word in ["Ollama", "LM Studio", "컨텍스트 토큰", "인터넷"] {
                assert!(!m.contains(word), "{m}");
            }
        }
        let distinct: std::collections::HashSet<_> = messages.iter().collect();
        assert_eq!(distinct.len(), all.len());

        let cannot = Failure::CannotStart(exe.clone()).message();
        assert!(
            cannot.contains(&exe.display().to_string()) && cannot.contains("IT에 이 경로의 예외")
        );
        assert!(cannot.ends_with("로그: 설정 → 일반 → 로그 폴더 열기"));
        let crashed = Failure::Crashed { gpu: true }.message();
        assert!(
            crashed.contains("GPU와 CPU에서 모두") && crashed.contains("다른 연결로 다시 실행")
        );
        assert!(!Failure::Crashed { gpu: false }.message().contains("GPU"));
        assert!(
            Failure::Timeout
                .message()
                .contains("요청 하나가 30분을 넘었습니다")
        );
        assert!(
            Failure::Length
                .message()
                .starts_with("요약이 길이 제한에서 끊겼습니다")
        );
        assert!(Failure::GlossaryTooLong.message().contains("용어집"));
    }

    // The lifecycle with processes: this test binary as the supervisor and as llama-server.

    #[test]
    fn a_server_reports_its_port_once_loaded_and_our_stop_kills_it_without_blame() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fake(tmp.path(), "supervisor", fake_llama("ready"), LIMITS.reap);
        let endpoint = f
            .sidecar
            .start(f.sidecar.generation(), Device::Gpu)
            .unwrap();
        let port = port(&endpoint.base_url);
        assert_eq!(endpoint.key.len(), 64);
        assert!(!format!("{endpoint:?}").contains(&endpoint.key));
        // The fake answers only with the key from its environment, and only if no argument has it.
        let health = get(port, "/health", &endpoint.key).unwrap();
        assert!(health.starts_with("HTTP/1.1 200"), "{health}");
        let log = f.log(LOG);
        assert!(
            log.starts_with("--- -m Qwen3.5-4B-Q4_K_M.gguf -c 16384 --host 127.0.0.1 --port 0")
        );
        assert!(
            log.contains("--reasoning off") && log.contains("-ngl 99"),
            "{log}"
        );
        assert!(!log.contains(&endpoint.key));
        assert!(f.sidecar.running());

        let seen = f.seen();
        f.sidecar.stop("테스트");
        assert!(gone(port));
        let end = wait_for(Duration::from_secs(5), || lock(&seen).end.clone()).unwrap();
        assert!(end.stopped && !end.oom, "{end:?}");
        assert!(
            wait_for(Duration::from_secs(5), || (!f.sidecar.running())
                .then_some(()))
            .is_some()
        );
        // A request that failed because of the stop does not bring a server back.
        let mut endpoint = endpoint;
        assert_eq!(
            f.sidecar.recover(&mut endpoint, false),
            Err(Failure::Stopped)
        );
        assert!(!f.sidecar.running());
        assert_eq!(f.marked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_server_that_cannot_run_takes_no_gpu_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("no-llama-server");
        let f = fake(
            tmp.path(),
            "supervisor",
            vec![missing.clone().into()],
            LIMITS.reap,
        );
        let started = f.sidecar.start(f.sidecar.generation(), Device::Gpu);
        assert_eq!(started.unwrap_err(), Failure::CannotStart(missing));

        // Ends at once without a word, like one antivirus stopped on launch.
        let silent: Vec<OsString> = if cfg!(windows) {
            ["cmd", "/d", "/c", "rem"].map(OsString::from).into()
        } else {
            vec!["true".into()]
        };
        let f = fake(tmp.path(), "supervisor", silent, LIMITS.reap);
        let started = f.sidecar.start(f.sidecar.generation(), Device::Gpu);
        assert!(
            matches!(started, Err(Failure::CannotStart(_))),
            "{started:?}"
        );
        // One server only: no hash, no CPU try, no blame.
        assert_eq!(f.hashed.load(Ordering::SeqCst), 0);
        assert!(!f.log(LOG).contains("--device none"));
        assert_eq!(f.marked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_death_serving_brings_the_server_back_on_the_gpu_then_the_cpu() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fake(tmp.path(), "supervisor", fake_llama("ready"), LIMITS.reap);
        let mut endpoint = f
            .sidecar
            .start(f.sidecar.generation(), Device::Gpu)
            .unwrap();
        let key = endpoint.key.clone();

        let first = port(&endpoint.base_url);
        crash(first, &key);
        let second = port(&f.sidecar.recover(&mut endpoint, false).unwrap());
        assert!(gone(first));
        assert!(f.log(LOG).contains("-ngl 99"));
        assert_eq!(endpoint.key, key);

        crash(second, &key);
        let third = port(&f.sidecar.recover(&mut endpoint, false).unwrap());
        assert!(f.log(LOG).contains("-ngl 0 --device none --no-op-offload"));
        // ENG19: the GPU run's log is kept beside the CPU's.
        assert!(f.log(PREV_LOG).contains("-ngl 99"));
        let health = get(third, "/health", &key).unwrap();
        assert!(health.starts_with("HTTP/1.1 200"), "{health}");
        // The GPU died of its own fault and the CPU works. Windows also needs a Vulkan error.
        assert_eq!(
            f.marked.load(Ordering::SeqCst),
            usize::from(cfg!(not(windows)))
        );
        assert_eq!(f.hashed.load(Ordering::SeqCst), 0);

        crash(third, &key);
        assert_eq!(
            f.sidecar.recover(&mut endpoint, false),
            Err(Failure::Crashed { gpu: true })
        );
        assert!(gone(third));
        assert!(
            wait_for(Duration::from_secs(5), || (!f.sidecar.running())
                .then_some(()))
            .is_some()
        );
    }

    #[test]
    fn a_gpu_that_dies_loading_is_hashed_for_then_the_cpu_serves() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fake(
            tmp.path(),
            "supervisor",
            fake_llama("gpu-dies"),
            LIMITS.reap,
        );
        let endpoint = f
            .sidecar
            .start(f.sidecar.generation(), Device::Gpu)
            .unwrap();
        assert!(f.log(LOG).contains("--device none"));
        assert_eq!(f.hashed.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.marked.load(Ordering::SeqCst),
            usize::from(cfg!(not(windows)))
        );
        f.sidecar.stop("테스트");
        assert!(gone(port(&endpoint.base_url)));
    }

    #[cfg(unix)]
    #[test]
    fn the_memory_killer_restarts_the_server_but_never_blames_the_gpu() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fake(tmp.path(), "supervisor", fake_llama("ready"), LIMITS.reap);
        let mut endpoint = f
            .sidecar
            .start(f.sidecar.generation(), Device::Gpu)
            .unwrap();
        let oom = |f: &Fake| {
            let killed = Command::new("kill")
                .args(["-KILL", &f.pid().to_string()])
                .status();
            assert!(killed.unwrap().success());
        };
        oom(&f);
        f.sidecar.recover(&mut endpoint, false).unwrap();
        oom(&f);
        f.sidecar.recover(&mut endpoint, false).unwrap();
        assert!(f.log(LOG).contains("--device none"));
        oom(&f);
        assert_eq!(
            f.sidecar.recover(&mut endpoint, false),
            Err(Failure::OutOfMemory)
        );
        assert_eq!(f.marked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_timeout_stops_the_server_and_starts_none() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fake(tmp.path(), "supervisor", fake_llama("ready"), LIMITS.reap);
        let mut endpoint = f
            .sidecar
            .start(f.sidecar.generation(), Device::Gpu)
            .unwrap();
        let port = port(&endpoint.base_url);
        assert_eq!(
            f.sidecar.recover(&mut endpoint, true),
            Err(Failure::Timeout)
        );
        assert!(gone(port));
        assert!(lock(&f.sidecar.state).server.is_none());
        assert_eq!(f.marked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_stop_while_the_model_loads_discards_that_start() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fake(tmp.path(), "supervisor", fake_llama("slow"), LIMITS.reap);
        let generation = f.sidecar.generation();
        let started = thread::scope(|s| {
            let start = s.spawn(|| f.sidecar.start(generation, Device::Gpu));
            // The fake has its port and is "loading".
            let port = wait_for(Duration::from_secs(20), || {
                let log = f.log(LOG);
                let (_, rest) = log.split_once("fake: slow on ")?;
                rest.split(',').next()?.parse::<u16>().ok()
            })
            .unwrap();
            f.sidecar.stop("녹음 시작");
            let started = start.join().unwrap();
            assert!(gone(port));
            started
        });
        assert_eq!(started.unwrap_err(), Failure::Stopped);
        assert!(lock(&f.sidecar.state).server.is_none());
        // No CPU try for a server we stopped.
        assert!(!f.log(LOG).contains("--device none"));
        assert_eq!(
            (
                f.hashed.load(Ordering::SeqCst),
                f.marked.load(Ordering::SeqCst)
            ),
            (0, 0)
        );
    }

    #[test]
    fn a_server_that_never_gets_ready_is_stopped_and_not_blamed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = fake(tmp.path(), "supervisor", fake_llama("silent"), LIMITS.reap);
        f.sidecar.limits.ready = Duration::from_secs(1);
        let started = f.sidecar.start(f.sidecar.generation(), Device::Gpu);
        assert_eq!(started.unwrap_err(), Failure::Crashed { gpu: true });
        assert!(f.log(LOG).contains("--device none"));
        assert_eq!(f.marked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_supervisor_that_ignores_its_stdin_is_ended_by_pid_before_the_next_server_starts() {
        let tmp = tempfile::tempdir().unwrap();
        let reap = Duration::from_millis(800);
        let f = fake(tmp.path(), "deaf_supervisor", fake_llama("ready"), reap);
        let first = f
            .sidecar
            .start(f.sidecar.generation(), Device::Gpu)
            .unwrap();
        let first = port(&first.base_url);

        let stopped = Instant::now();
        f.sidecar.stop("테스트");
        assert!(stopped.elapsed() < Duration::from_millis(200));
        assert!(f.sidecar.running());
        let second = f
            .sidecar
            .start(f.sidecar.generation(), Device::Gpu)
            .unwrap();
        // Not before the first was cleaned up: never two models in memory.
        assert!(stopped.elapsed() >= reap, "{:?}", stopped.elapsed());
        assert!(gone(first));
        f.sidecar.stop("테스트");
        assert!(gone(port(&second.base_url)));
        assert!(
            wait_for(Duration::from_secs(5), || (!f.sidecar.running())
                .then_some(()))
            .is_some()
        );
    }
}
