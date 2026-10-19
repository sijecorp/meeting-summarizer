//! Processing queue (architecture 5.3, 8.4, TRN-01). Up to two meetings are processed at once,
//! one once the summary model is installed (ENG1); the rest wait first in, first out. Each running
//! meeting has a thread that transcribes and then summarizes it in the same slot. Transcribing
//! starts one worker process per chunk and saves every finished chunk, so a stopped meeting
//! resumes from the chunk after its last finished one (STO-03). A meeting with `transcript.json`
//! is only summarized (architecture 6): a paused summary, a `summarize_failed` retry. While its
//! route does not resolve it waits in `transcribed` (or `paused`) with the reason (plan 5.4).
//!
//! The decisions are pure and tested without the app: `Sched` (who runs, who waits, places in
//! line, the built-in engine's lease), `ChunkRun` (GPU retry, CPU fallback, TRN-07),
//! `read_partial` (where to resume), `percent` and `eta_secs`.

use crate::grounding;
use crate::settings::{self, BUILTIN, Backend, LlmConnection, SecretKind, Settings, SettingsStore};
use crate::sidecar::{self, Sidecar};
use crate::store::{Meta, SCHEMA_VERSION, Status, Store, can_transition, write_atomic};
use crate::summarize;
use crate::transcribe::engine::{self, Chunk, Device, Line, Segment};
use crate::transcribe::filter;
use crate::transcribe::model::{self, Check, Models};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime, Wry};

/// Meetings processed at once: two (TRN-01, user decision 2026-10-14), one once the summary model
/// is installed (ENG1), so whisper and the 4B model never share the memory and the GPU. By the
/// model, not the default connection: any meeting can ask for "로컬로 요약".
fn slots(builtin_ready: bool) -> usize {
    if builtin_ready { 1 } else { 2 }
}
/// A worker that prints nothing for this long is killed and counted as a crash. It prints a
/// progress line at least once per 30 s Whisper window (once per 10 min window when planning),
/// and loading even the 874 MB model takes seconds.
const STALL: Duration = Duration::from_secs(5 * 60);
const PARTIAL: &str = "transcript.partial.jsonl";
const GPU_FAILED: &str = "GPU 처리가 불안정해 CPU로 전환했습니다. 처리 시간이 길어집니다";

/// `meeting://progress`, and one entry of `queue_state`. Sent when a meeting joins the line, its
/// place changes, it starts, a chunk or LLM request moves, and when it ends (then with its final
/// status).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Progress {
    pub id: String,
    pub status: Status,
    /// Chunks finished, of `chunks`. `chunks` is 0 until the chunk plan exists. While
    /// `summarizing`: LLM requests done, of the ones known so far (a long meeting adds some).
    pub chunk: u32,
    pub chunks: u32,
    /// While `chunks` is 0, the chunk planning's progress; then 0..100 over the whole recording.
    /// While `summarizing`, from `chunk` / `chunks`.
    pub percent: u8,
    /// Remaining time, once this run has done enough to tell. Never while `summarizing`.
    pub eta_secs: Option<u64>,
    /// 1-based place in line while waiting; `None` while running.
    pub position: Option<u32>,
}

/// `transcribe://gpu-failed`: the GPU crashed twice on one chunk, so automatic mode now uses the
/// CPU (architecture 5.4). An in-app banner only: there is no meeting for an OS notification to
/// open (APP-03).
#[derive(Serialize, Clone, Debug)]
pub struct Notice {
    pub message: &'static str,
}

#[derive(Default)]
struct Sched {
    waiting: VecDeque<String>,
    /// Meetings with a thread. A run stopped for a recording stays here until its thread ends,
    /// so a meeting never runs twice at once.
    running: Vec<String>,
    /// TRN-09: nothing starts while recording.
    recording: bool,
    /// ENG1: the meeting whose summary holds the built-in engine, one at a time. Taken before
    /// `summarizing` is written, given back in `end`.
    lease: Option<String>,
    /// Meetings that found the lease taken: back in line, passed over until it is given back.
    blocked: Vec<String>,
}

impl Sched {
    /// FIFO. A meeting already in line or running keeps its place.
    fn enqueue(&mut self, id: &str) -> bool {
        if self.waiting.iter().chain(&self.running).any(|w| w == id) {
            return false;
        }
        self.waiting.push_back(id.to_string());
        true
    }

    /// Meetings to start now, in line order, up to `slots` running, none while recording. One
    /// `can_start` turns down (its model is missing, ENG2) or that waits for the lease keeps its
    /// place and the next one may go.
    fn start_next(&mut self, slots: usize, can_start: impl Fn(&str) -> bool) -> Vec<String> {
        let mut started = Vec::new();
        let mut at = 0;
        while !self.recording && self.running.len() < slots && at < self.waiting.len() {
            let id = &self.waiting[at];
            // Still winding down from a stop: it and the line behind it wait for its thread.
            if self.running.contains(id) {
                break;
            }
            if self.blocked.contains(id) || !can_start(id) {
                at += 1;
                continue;
            }
            let id = self.waiting.remove(at).expect("in line");
            self.running.push(id.clone());
            started.push(id);
        }
        started
    }

    /// ENG1: the built-in engine for `id`. False while another summary holds it: `id` then waits
    /// in line, passed over, until it is given back.
    fn take_lease(&mut self, id: &str) -> bool {
        match &self.lease {
            Some(holder) if holder != id => {
                if !self.blocked.iter().any(|b| b == id) {
                    self.blocked.push(id.to_string());
                }
                false
            }
            _ => {
                self.lease = Some(id.to_string());
                true
            }
        }
    }

    /// True when `id` held the lease, which is free now.
    fn release_lease(&mut self, id: &str) -> bool {
        if self.lease.as_deref() != Some(id) {
            return false;
        }
        self.lease = None;
        self.blocked.clear();
        true
    }

    /// TRN-09: a recording started. Running meetings go back to the front of the line, in the
    /// order they started, except those `keep` names (cloud summaries, SUM-09). Returns the ones
    /// to be stopped.
    fn suspend(&mut self, keep: impl Fn(&str) -> bool) -> Vec<String> {
        self.recording = true;
        let stopped: Vec<String> = self
            .running
            .iter()
            .filter(|id| !keep(id))
            .cloned()
            .collect();
        for id in stopped.iter().rev() {
            if !self.waiting.contains(id) {
                self.waiting.push_front(id.clone());
            }
        }
        stopped
    }

    fn finished(&mut self, id: &str) {
        self.running.retain(|r| r != id);
    }

    /// Out of the line (delete). Returns whether a thread still runs it.
    fn remove(&mut self, id: &str) -> bool {
        self.waiting.retain(|w| w != id);
        self.running.iter().any(|r| r == id)
    }

    fn position(&self, id: &str) -> Option<u32> {
        let at = self.waiting.iter().position(|w| w == id)?;
        Some(at as u32 + 1)
    }
}

/// Why the queue stopped a run. Set before its worker is killed, so the kill never counts as a
/// crash.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stop {
    /// TRN-09: back in line; the chunk runs again from its start once the recording ends.
    Recording,
    Delete,
    /// App exit: `Store::pause_busy` writes `paused`.
    Quit,
}

#[derive(Default)]
struct Job {
    stop: Option<Stop>,
    child: Option<Child>,
    /// Aborts the summary task in flight.
    abort: Option<Box<dyn Fn() + Send>>,
    /// A cloud summary is in flight: a recording lets it run on (SUM-09).
    cloud: bool,
}

impl Job {
    fn halt(&mut self, why: Stop) {
        // A delete wins: its run must not write `paused` over `deleting` afterwards.
        if self.stop != Some(Stop::Delete) {
            self.stop = Some(why);
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
        if let Some(abort) = &self.abort {
            abort();
        }
    }
}

#[derive(Default)]
struct Inner {
    sched: Sched,
    jobs: HashMap<String, Job>,
    progress: HashMap<String, Progress>,
    /// The app is exiting: nothing starts any more.
    closed: bool,
}

/// Poisoning is ignored: a panicked run must not keep a recording from stopping the others.
fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    /// TRN-09, SUM-09: running meetings back in line, their workers killed and local summaries
    /// aborted. A cloud summary runs on through the recording.
    fn suspend(&mut self) {
        let Inner { sched, jobs, .. } = self;
        for id in sched.suspend(|id| jobs.get(id).is_some_and(|j| j.cloud)) {
            if let Some(job) = jobs.get_mut(&id) {
                job.halt(Stop::Recording);
            }
        }
    }
}

/// How a worker process ended, before its result line is looked at.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Ended {
    code: Option<i32>,
    /// SIGKILL that the queue did not send: on macOS, the memory killer.
    killed: bool,
    /// Silent for `STALL`, so the queue killed it.
    stalled: bool,
}

/// How a worker ended (architecture 5.3 exit-code contract).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Exit {
    /// Exit 0 with a result; the device that actually ran.
    Done(Device),
    Input,
    Model,
    /// Not the GPU's fault: never counts toward `gpu_failed`.
    OutOfMemory,
    /// Counts like a crash.
    Stalled,
    /// Any other code or signal, or exit 0 without a result.
    Crash,
}

fn exit_of(ended: Ended, result: Option<Device>) -> Exit {
    if ended.stalled {
        return Exit::Stalled;
    }
    if ended.killed {
        return Exit::OutOfMemory;
    }
    match (ended.code, result) {
        (Some(0), Some(device)) => Exit::Done(device),
        (Some(engine::EXIT_INPUT), _) => Exit::Input,
        (Some(engine::EXIT_MODEL), _) => Exit::Model,
        _ => Exit::Crash,
    }
}

fn killed_by_os(status: Option<ExitStatus>) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.and_then(|s| s.signal()) == Some(9)
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        false
    }
}

/// TRN-05 / architecture 5.4: the device a chunk starts on, and plan 5.3's for the built-in
/// engine, from the one device setting. `failed`: `gpu_failed`, or `summary_gpu_failed`.
fn first_device(backend: Backend, failed: bool) -> Device {
    match backend {
        Backend::Cpu => Device::Cpu,
        Backend::Gpu => Device::Gpu,
        Backend::Auto if failed => Device::Cpu,
        Backend::Auto => Device::Gpu,
    }
}

/// Only the summary is left (architecture 8.4): from its status, and for `paused` from
/// `transcript.json`. Not from the file alone: a `transcribe_failed` meeting whose
/// `transcript.json` was written before its status could be transcribes again.
fn summary_only(store: &Store, meta: &Meta) -> bool {
    match meta.status {
        Status::Transcribed | Status::Summarizing | Status::SummarizeFailed => true,
        Status::Paused => store
            .dir_for(&meta.id)
            .is_ok_and(|dir| dir.join("transcript.json").exists()),
        _ => false,
    }
}

#[derive(Debug, PartialEq)]
enum Next {
    Accept,
    Run(Device),
    /// The second GPU crash on this chunk: record `gpu_failed`, tell the recorder, then run the
    /// chunk on the CPU.
    GpuFailed,
    Fail,
}

/// One chunk's attempts (TRN-06, TRN-07). Crash counts live only here, in memory.
struct ChunkRun {
    device: Device,
    gpu_crashes: u32,
    rechecked: bool,
}

impl ChunkRun {
    fn new(device: Device) -> Self {
        Self {
            device,
            gpu_crashes: 0,
            rechecked: false,
        }
    }

    /// `abnormal`: the result failed the TRN-07 check.
    fn after(&mut self, exit: Exit, abnormal: bool) -> Next {
        match exit {
            Exit::Done(device) => {
                self.device = device;
                if abnormal && device == Device::Gpu && !self.rechecked {
                    self.rechecked = true;
                    self.device = Device::Cpu;
                    Next::Run(Device::Cpu)
                } else {
                    // A CPU result is kept even if it looks odd: running it again gives the same.
                    Next::Accept
                }
            }
            Exit::Crash | Exit::Stalled if self.device == Device::Gpu => {
                self.gpu_crashes += 1;
                if self.gpu_crashes < 2 {
                    Next::Run(Device::Gpu)
                } else {
                    self.device = Device::Cpu;
                    Next::GpuFailed
                }
            }
            // Input, model and memory errors have nothing to do with the GPU (architecture 5.3).
            Exit::Input | Exit::Model | Exit::OutOfMemory | Exit::Crash | Exit::Stalled => {
                Next::Fail
            }
        }
    }
}

/// `chunks.json`: made once per meeting, so a resumed meeting cuts the same way.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Plan {
    len_ms: u64,
    chunks: Vec<Chunk>,
}

/// One line of `transcript.partial.jsonl` (architecture 5.2). Segments are already filtered.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct ChunkResult {
    chunk: u32,
    start_ms: u64,
    end_ms: u64,
    segments: Vec<Segment>,
}

/// `transcript.json`: every kept segment of the meeting, in order.
#[derive(Serialize)]
struct Transcript<'a> {
    segments: Vec<&'a Segment>,
}

/// STO-03: the finished chunks, in order. Reading stops at the first line that is not the next
/// chunk of `plan`; that chunk runs again.
fn read_partial(path: &Path, plan: &[Chunk]) -> Vec<ChunkResult> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut done: Vec<ChunkResult> = Vec::new();
    for line in text.lines() {
        let Ok(result) = serde_json::from_str::<ChunkResult>(line) else {
            break;
        };
        let expected = plan.get(done.len());
        if result.chunk as usize != done.len()
            || expected.is_none_or(|c| (c.start_ms, c.end_ms) != (result.start_ms, result.end_ms))
        {
            break;
        }
        done.push(result);
    }
    done
}

/// ponytail: the whole file is rewritten atomically per chunk, never appended, so a crash can't
/// leave a torn line. A 4 h meeting has 24 chunks of a few KB.
fn write_partial(path: &Path, done: &[ChunkResult]) -> std::io::Result<()> {
    let mut text = String::new();
    for result in done {
        text += &serde_json::to_string(result)?;
        text.push('\n');
    }
    write_atomic(path, text.as_bytes())
}

/// Over the whole recording: finished chunks plus the running chunk's share.
fn percent(done_ms: u64, chunk_ms: u64, chunk_pct: u8, total_ms: u64) -> u8 {
    if total_ms == 0 {
        return 0;
    }
    ((done_ms + chunk_ms * u64::from(chunk_pct) / 100) * 100 / total_ms).min(100) as u8
}

/// The pace of this run so far, applied to what is left. `None` until both a minute of audio and
/// 10 s have passed: model loading would make earlier guesses wild.
fn eta_secs(processed_ms: u64, elapsed: Duration, remaining_ms: u64) -> Option<u64> {
    if processed_ms < 60_000 || elapsed < Duration::from_secs(10) {
        return None;
    }
    Some((remaining_ms as u128 * elapsed.as_millis() / processed_ms as u128 / 1000) as u64)
}

/// Why a model would not load, from hashing its files (P2-1 review).
fn model_failure(check: Check, why: &str) -> String {
    match check {
        Check::Intact => format!(
            "모델을 불러오지 못했습니다. 메모리가 부족할 수 있으니 다른 앱을 닫고 다시 시도해 주세요: {why}"
        ),
        Check::Damaged => "모델 파일이 손상됐습니다. 설정에서 모델을 다시 받아 주세요".into(),
        Check::Missing => {
            format!("모델 파일이 없거나 읽을 수 없습니다. 설정에서 모델을 다시 받아 주세요: {why}")
        }
    }
}

/// H4: ggml is built for AVX2, FMA, F16C and BMI2 (NFR-02, `.cargo/config.toml`). Without one of
/// them the worker dies with an illegal instruction, which would read as a GPU crash.
#[cfg(any(target_arch = "x86_64", test))]
fn cpu_failure(features: &[bool]) -> Option<&'static str> {
    features.contains(&false).then_some(
        "이 PC의 CPU는 전사에 필요한 AVX2 등의 명령어를 지원하지 않아 전사할 수 없습니다. AVX2를 지원하는 PC(대부분의 Intel Core·AMD Ryzen)에서 실행해 주세요",
    )
}

/// The last thing the worker said on stderr: the reason shown for a failure.
fn last_log_line(log: &Path) -> String {
    let text = fs::read(log).unwrap_or_default();
    String::from_utf8_lossy(&text)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty() && !l.starts_with("--- "))
        .map(|l| l.trim().to_string())
        .unwrap_or_default()
}

/// Runs a worker to its end, handing each stdout line to `on_line`. `Stopped` when the queue
/// stopped this run (recording, delete, quit): then how the worker ended does not count. A worker
/// silent for `stall` is killed here and comes back `stalled`.
fn run_worker(
    inner: &Mutex<Inner>,
    id: &str,
    mut command: Command,
    stall: Duration,
    mut on_line: impl FnMut(Line),
) -> Result<Ended, End> {
    // Stdin stays open while we run: the worker exits when it closes (engine::worker_main).
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console window for the worker.
        command.creation_flags(0x0800_0000);
    }
    let stdout = {
        // Spawned under the lock, so a stop can never miss a worker that is just starting.
        let mut inner = lock(inner);
        let Some(job) = inner.jobs.get_mut(id) else {
            return Err(End::Stopped(Stop::Delete));
        };
        if let Some(stop) = job.stop {
            return Err(End::Stopped(stop));
        }
        let mut child = command
            .spawn()
            .map_err(|e| End::Failed(format!("전사 워커를 시작할 수 없습니다: {e}")))?;
        let stdout = child.stdout.take();
        job.child = Some(child);
        stdout
    };
    let stalled = AtomicBool::new(false);
    let status = thread::scope(|s| {
        let (beat, beats) = mpsc::channel::<()>();
        let stalled = &stalled;
        s.spawn(move || {
            loop {
                match beats.recv_timeout(stall) {
                    Ok(()) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {
                        stalled.store(true, Ordering::SeqCst);
                        let mut inner = lock(inner);
                        if let Some(child) = inner.jobs.get_mut(id).and_then(|j| j.child.as_mut()) {
                            let _ = child.kill();
                        }
                        return;
                    }
                }
            }
        });
        if let Some(stdout) = stdout {
            engine::read_lines(stdout, |line| {
                let _ = beat.send(());
                if let Some(line) = line {
                    on_line(line);
                }
            });
        }
        // Stdout closed. The child stays in its job, killable, until it has really exited.
        let status = loop {
            {
                let mut inner = lock(inner);
                let Some(job) = inner.jobs.get_mut(id) else {
                    break None;
                };
                let Some(child) = job.child.as_mut() else {
                    break None;
                };
                match child.try_wait() {
                    Ok(None) => {}
                    done => {
                        job.child = None;
                        break done.ok().flatten();
                    }
                }
            }
            thread::sleep(Duration::from_millis(20));
        };
        drop(beat);
        status
    });
    let inner = lock(inner);
    if let Some(stop) = inner.jobs.get(id).map_or(Some(Stop::Delete), |j| j.stop) {
        return Err(End::Stopped(stop));
    }
    Ok(Ended {
        code: status.and_then(|s| s.code()),
        killed: killed_by_os(status),
        stalled: stalled.load(Ordering::SeqCst),
    })
}

enum End {
    /// Finished in this status: `review` (`transcribe` hands `transcribed` on to the summary).
    Done(Status),
    Stopped(Stop),
    /// `transcribe_failed`, or `summarize_failed` once summarizing started, with the reason.
    Failed(String),
    /// Not in a state to process (finished meanwhile, being deleted).
    Skipped(String),
    /// Back in line as it is: a model went missing after `pump` checked, or another summary holds
    /// the built-in engine (ENG1). `start_next` passes it over until that changes.
    Requeue,
}

/// An LLM connection's API key by connection name.
type Secret = Box<dyn Fn(&str) -> Result<Option<String>, String> + Send + Sync>;

/// Generic over the runtime only so tests can run it on `tauri::test`'s mock runtime.
pub struct Queue<R: Runtime = Wry> {
    app: AppHandle<R>,
    /// This executable, started with `--plan-chunks` / `--transcribe-worker`.
    exe: PathBuf,
    /// The keychain; tests put a stand-in here.
    secret: Secret,
    inner: Mutex<Inner>,
    /// The built-in summary engine, one for the app (plan 5.3). Its lock comes after `inner`'s.
    /// Shared with its summary's `on_failure` hook.
    sidecar: Arc<Sidecar>,
}

impl<R: Runtime> Queue<R> {
    /// After `Models` is managed: the sidecar runs llama-server in its folder.
    pub fn new(app: AppHandle<R>, exe: PathBuf) -> Self {
        let models = app.state::<Models>().dir.clone();
        // No log folder (no home folder): llama-server runs without its log, never into the cwd.
        let logs = app.path().app_log_dir().ok();
        let (hashing, marking) = (app.clone(), app.clone());
        let sidecar = Sidecar::new(
            &exe,
            models,
            logs,
            move || hashing.state::<Models>().check(&[&model::SUMMARY]) != Check::Intact,
            move || {
                if let Err(e) = marking.state::<SettingsStore>().mark_summary_gpu_failed() {
                    log::error!("summary_gpu_failed 저장 실패: {e}");
                }
            },
        );
        Self {
            app,
            exe,
            secret: Box::new(|name| settings::get_secret(SecretKind::Llm, name)),
            inner: Mutex::new(Inner::default()),
            sidecar: Arc::new(sidecar),
        }
    }

    fn emit(&self, progress: &Progress) {
        if let Err(e) = self.app.emit("meeting://progress", progress) {
            log::warn!("진행률 이벤트 전송 실패: {e}");
        }
    }

    /// Places in line changed: tell the UI about every meeting whose place moved.
    fn publish_positions(&self, inner: &mut Inner) {
        let Inner {
            sched, progress, ..
        } = inner;
        for p in progress.values_mut() {
            let position = sched.position(&p.id);
            if p.position != position {
                p.position = position;
                self.emit(p);
            }
        }
    }

    fn set_progress(&self, id: &str, f: impl FnOnce(&mut Progress)) {
        let mut inner = lock(&self.inner);
        if let Some(p) = inner.progress.get_mut(id) {
            f(p);
            self.emit(p);
        }
    }

    /// For the first render, before events arrive. Running meetings first, then the line.
    pub fn snapshot(&self) -> Vec<Progress> {
        let mut all: Vec<Progress> = lock(&self.inner).progress.values().cloned().collect();
        all.sort_by_key(|p| p.position.unwrap_or(0));
        all
    }

    fn add(&self, id: &str, status: Status) {
        let mut inner = lock(&self.inner);
        if inner.closed || !inner.sched.enqueue(id) {
            return;
        }
        inner.progress.insert(
            id.to_string(),
            Progress {
                id: id.to_string(),
                status,
                chunk: 0,
                chunks: 0,
                percent: 0,
                eta_secs: None,
                position: None,
            },
        );
        self.publish_positions(&mut inner);
    }

    /// TRN-01: every `recorded` and `paused` meeting, oldest first, but one with only its summary
    /// left (`transcribed`, `paused` after transcribing) once its route resolves (plan 5.4); until
    /// then it says why it waits. At startup (after recovery paused what was running), when a
    /// recording ends, when settings are saved and when the summary model arrives.
    pub fn enqueue_pending(&self) {
        let (settings, ready) = (self.settings(), self.builtin_ready());
        let store = self.app.state::<Store>();
        let mut metas = store.list();
        metas.reverse();
        for meta in metas {
            if meta.schema_version > SCHEMA_VERSION {
                continue;
            }
            let pending = match meta.status {
                Status::Paused | Status::Transcribed if summary_only(&store, &meta) => {
                    self.routes(&store, &settings, &meta, ready)
                }
                Status::Recorded | Status::Paused => true,
                _ => false,
            };
            if pending {
                self.add(&meta.id, meta.status);
            }
        }
        self.pump();
        crate::refresh_tray(&self.app);
    }

    /// Plan 5.4: the meeting's route resolves. `meta.json` says why not, or no longer says it.
    fn routes(&self, store: &Store, settings: &Settings, meta: &Meta, ready: bool) -> bool {
        let wait = summarize::route(settings, meta, ready).err();
        if meta.wait != wait {
            let waits = store.update(&meta.id, |m| {
                // Not one that started summarizing meanwhile.
                if matches!(m.status, Status::Transcribed | Status::Paused) {
                    m.wait = wait;
                }
                Ok(())
            });
            if let Err(e) = waits {
                log::warn!("요약 대기 이유 기록 실패 ({}): {e}", meta.id);
            }
        }
        wait.is_none()
    }

    /// `resume_meeting`: a continued interrupted recording, or a `transcribe_failed` retry.
    /// `retry_summary`: a `summarize_failed` one, or a waiting one given another connection.
    pub fn enqueue(&self, id: &str, status: Status) {
        self.add(id, status);
        self.pump();
        crate::refresh_tray(&self.app);
    }

    /// Plan 5.4: the summary model is installed, so the built-in route can summarize.
    pub fn builtin_ready(&self) -> bool {
        self.app.state::<Models>().installed(&model::SUMMARY)
    }

    /// Starts whatever may start (`slots`). ENG2: a meeting to transcribe waits for the Whisper
    /// and VAD models, until a download or import finishes (ONB-01) and calls this again; one with
    /// only its summary left starts without them.
    pub fn pump(&self) {
        let settings = self.settings();
        let store = self.app.state::<Store>();
        let transcribable = self.app.state::<Models>().ready(&settings.model).is_some();
        let slots = slots(self.builtin_ready());
        let can_start =
            |id: &str| transcribable || store.get(id).is_ok_and(|m| summary_only(&store, &m));
        let mut inner = lock(&self.inner);
        if inner.closed {
            return;
        }
        for id in inner.sched.start_next(slots, can_start) {
            let app = self.app.clone();
            let run_id = id.clone();
            let spawned = thread::Builder::new()
                .name("process".into())
                .spawn(move || run(&app, &run_id));
            match spawned {
                // Detached: `cancel` waits on `running` instead, which `end` clears.
                Ok(_) => {
                    inner.jobs.insert(id, Job::default());
                }
                Err(e) => {
                    log::error!("처리 스레드를 시작할 수 없습니다 ({id}): {e}");
                    inner.sched.finished(&id);
                    inner.progress.remove(&id);
                }
            }
        }
        self.publish_positions(&mut inner);
    }

    /// TRN-09, SUM-09: kills every worker and aborts local summaries at once. Their meetings go
    /// back to the front of the line. Cloud summaries go on. The built-in engine stops too, under
    /// the same lock and without waiting for it (plan 5.3).
    pub fn recording_started(&self) {
        let mut inner = lock(&self.inner);
        inner.suspend();
        self.sidecar.stop("녹음 시작");
        self.publish_positions(&mut inner);
    }

    /// The recording is `recorded` (or failed to start): queue it and carry on.
    pub fn recording_finished(&self) {
        lock(&self.inner).sched.recording = false;
        self.enqueue_pending();
    }

    /// Delete steps 2–3 (architecture 8.3): out of the line, the worker killed or the summary
    /// aborted, and returns once the meeting's run has ended. A second delete of the same meeting
    /// waits here too.
    pub fn cancel(&self, id: &str) {
        {
            let mut inner = lock(&self.inner);
            inner.progress.remove(id);
            if inner.sched.remove(id)
                && let Some(job) = inner.jobs.get_mut(id)
            {
                job.halt(Stop::Delete);
                if inner.sched.lease.as_deref() == Some(id) {
                    self.sidecar.stop("요약 중인 회의 삭제");
                }
            }
            self.publish_positions(&mut inner);
        }
        while lock(&self.inner).sched.running.iter().any(|r| r == id) {
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// App exit (architecture 8.5): nothing new starts, every worker is killed and every summary
    /// aborted. Child processes outlive their parent otherwise. `Store::pause_busy` then writes
    /// `paused`. The engine's supervisor only has its stdin closed: it ends llama-server itself.
    pub fn shutdown(&self) {
        let mut inner = lock(&self.inner);
        inner.closed = true;
        for job in inner.jobs.values_mut() {
            job.halt(Stop::Quit);
        }
        self.sidecar.stop("앱 종료");
    }

    /// ENG18: llama-server holds the summary model open.
    pub fn engine_running(&self) -> bool {
        self.sidecar.running()
    }

    /// ENG1, ENG3: a built-in summary of `id` is about to start the engine. Under the lock, with
    /// the job's stop: `Stopped` if the run was stopped or deleted already, `Requeue` while another
    /// summary holds the lease, else `id` holds it (a delete or its end stops the engine) and the
    /// generation for `Sidecar::start` comes back, so a stop after this is never missed.
    fn engine_generation(&self, id: &str) -> Result<u64, End> {
        let mut inner = lock(&self.inner);
        let Some(job) = inner.jobs.get(id) else {
            return Err(End::Stopped(Stop::Delete));
        };
        if let Some(stop) = job.stop {
            return Err(End::Stopped(stop));
        }
        if !inner.sched.take_lease(id) {
            return Err(End::Requeue);
        }
        Ok(self.sidecar.generation())
    }

    /// A built-in summary whose engine did not come up: a stop's own end, else the engine's words.
    fn engine_end(&self, id: &str, failure: sidecar::Failure) -> End {
        match lock(&self.inner)
            .jobs
            .get(id)
            .map_or(Some(Stop::Delete), |j| j.stop)
        {
            Some(stop) => End::Stopped(stop),
            None => End::Failed(failure.message()),
        }
    }

    fn settings(&self) -> Settings {
        self.app
            .state::<SettingsStore>()
            .load()
            .map(|l| l.settings)
            .unwrap_or_default()
    }

    fn worker(&self, dir: &Path, args: &[&OsStr]) -> Result<Command, End> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("worker.log"))
            .map_err(|e| End::Failed(format!("worker.log를 열 수 없습니다: {e}")))?;
        let header: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
        let _ = writeln!(&log, "--- {} ---", header.join(" "));
        let mut command = Command::new(&self.exe);
        command.args(args).stderr(log);
        Ok(command)
    }

    /// Why a run failed, for the recorder. A model that would not load is hashed first: a damaged
    /// file is deleted so "다시 받기" fetches it, an intact one points elsewhere (`model_failure`).
    fn failure(&self, exit: Exit, dir: &Path) -> String {
        let why = last_log_line(&dir.join("worker.log"));
        match exit {
            Exit::Input => format!("녹음 파일을 읽을 수 없습니다: {why}"),
            Exit::Model => model_failure(
                self.app.state::<Models>().verify(&self.settings().model),
                &why,
            ),
            Exit::OutOfMemory => {
                "메모리가 부족해 전사가 중단되었습니다. 다른 앱을 닫고 다시 시도해 주세요".into()
            }
            Exit::Stalled => format!(
                "전사가 {}분 동안 응답하지 않아 중단했습니다: {why}",
                STALL.as_secs() / 60
            ),
            Exit::Crash | Exit::Done(_) => format!("전사 중 오류가 났습니다: {why}"),
        }
    }

    /// The chunk plan: from `chunks.json`, or made by a `--plan-chunks` worker and saved. Its
    /// progress shows as `percent` while `chunks` is still 0.
    fn plan(&self, store: &Store, id: &str, dir: &Path, vad: &Path) -> Result<Plan, End> {
        let path = dir.join("chunks.json");
        if let Some(plan) = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
        {
            return Ok(plan);
        }
        let args = [
            OsStr::new("--plan-chunks"),
            dir.as_os_str(),
            vad.as_os_str(),
        ];
        let command = self.worker(dir, &args)?;
        let mut plan = None;
        let ended = run_worker(&self.inner, id, command, STALL, |line| match line {
            Line::Progress { pct } => self.set_progress(id, |p| p.percent = pct),
            Line::Plan { len_ms, chunks } => plan = Some(Plan { len_ms, chunks }),
            Line::Result { .. } => {}
        })?;
        let plan = match (ended.code, plan) {
            (Some(0), Some(plan)) if !ended.stalled => plan,
            (_, _) => return Err(End::Failed(self.failure(exit_of(ended, None), dir))),
        };
        let bytes = serde_json::to_vec(&plan).map_err(|e| End::Failed(e.to_string()))?;
        store
            .update(id, |_| {
                write_atomic(&path, &bytes).map_err(|e| format!("chunks.json 저장 실패: {e}"))
            })
            .map_err(End::Failed)?;
        Ok(plan)
    }

    /// One chunk until it is accepted or fails, following `ChunkRun`. Returns the filtered
    /// segments and the device that produced them.
    fn chunk(
        &self,
        id: &str,
        dir: &Path,
        chunk: &Chunk,
        (model, vad): (&Path, &Path),
        mut on_pct: impl FnMut(u8),
    ) -> Result<(Vec<Segment>, Device), End> {
        let settings = self.settings();
        let mut run = ChunkRun::new(first_device(settings.backend, settings.gpu_failed));
        loop {
            let (start, end) = (chunk.start_ms.to_string(), chunk.end_ms.to_string());
            let args = [
                OsStr::new("--transcribe-worker"),
                dir.as_os_str(),
                OsStr::new(&start),
                OsStr::new(&end),
                OsStr::new(run.device.name()),
                model.as_os_str(),
                vad.as_os_str(),
            ];
            let command = self.worker(dir, &args)?;
            let mut result = None;
            let ended = run_worker(&self.inner, id, command, STALL, |line| match line {
                Line::Progress { pct } => on_pct(pct),
                Line::Result { device, segments } => result = Some((device, segments)),
                Line::Plan { .. } => {}
            })?;
            let exit = exit_of(ended, result.as_ref().map(|(device, _)| *device));
            let segments = result.map(|(_, s)| filter::filter(s)).unwrap_or_default();
            let abnormal = matches!(exit, Exit::Done(_))
                .then(|| filter::abnormal(&segments, chunk.speech_ms))
                .flatten();
            if let Some(why) = abnormal {
                log::warn!("비정상 전사 결과 ({id}, {}ms~): {why}", chunk.start_ms);
            }
            match run.after(exit, abnormal.is_some()) {
                Next::Accept => return Ok((segments, run.device)),
                Next::Run(_) => {}
                Next::GpuFailed => {
                    if let Err(e) = self.app.state::<SettingsStore>().mark_gpu_failed() {
                        log::error!("gpu_failed 저장 실패: {e}");
                    }
                    let _ = self.app.emit(
                        "transcribe://gpu-failed",
                        Notice {
                            message: GPU_FAILED,
                        },
                    );
                }
                Next::Fail => return Err(End::Failed(self.failure(exit, dir))),
            }
        }
    }

    fn transcribe(&self, store: &Store, id: &str) -> End {
        let settings = self.settings();
        let Some((model, vad)) = self.app.state::<Models>().ready(&settings.model) else {
            return End::Requeue;
        };
        let started = store.update(id, |m| {
            // `transcribing` itself too: the queue runs a meeting once at a time, so that is a
            // stop whose `paused` could not be written. Recovery would pause it at the next start.
            if m.status != Status::Transcribing && !can_transition(m.status, Status::Transcribing) {
                return Err(format!("전사할 수 없는 상태입니다: {:?}", m.status));
            }
            m.status = Status::Transcribing;
            m.error = None;
            Ok(())
        });
        if let Err(e) = started {
            return End::Skipped(e);
        }
        // Before any worker: a plain failure, not a crash that counts toward `gpu_failed`.
        #[cfg(target_arch = "x86_64")]
        if let Some(why) = cpu_failure(&[
            std::arch::is_x86_feature_detected!("avx2"),
            std::arch::is_x86_feature_detected!("fma"),
            std::arch::is_x86_feature_detected!("f16c"),
            std::arch::is_x86_feature_detected!("bmi2"),
        ]) {
            return End::Failed(why.into());
        }
        self.set_progress(id, |p| {
            p.status = Status::Transcribing;
            p.position = None;
        });
        let dir = match store.dir_for(id) {
            Ok(dir) => dir,
            Err(e) => return End::Failed(e),
        };

        let plan = match self.plan(store, id, &dir, &vad) {
            Ok(plan) => plan,
            Err(end) => return end,
        };
        let partial = dir.join(PARTIAL);
        let mut done = read_partial(&partial, &plan.chunks);
        let chunks = plan.chunks.len() as u32;
        let done_ms =
            |done: &[ChunkResult]| -> u64 { done.iter().map(|r| r.end_ms - r.start_ms).sum() };
        let resumed_ms = done_ms(&done);
        let since = Instant::now();
        self.set_progress(id, |p| {
            p.chunk = done.len() as u32;
            p.chunks = chunks;
            p.percent = percent(resumed_ms, 0, 0, plan.len_ms);
        });

        for (index, chunk) in plan.chunks.iter().enumerate().skip(done.len()) {
            let before_ms = done_ms(&done);
            let chunk_ms = chunk.end_ms - chunk.start_ms;
            let report = |pct: u8| {
                let at_ms = before_ms + chunk_ms * u64::from(pct) / 100;
                self.set_progress(id, |p| {
                    p.percent = percent(before_ms, chunk_ms, pct, plan.len_ms);
                    p.eta_secs = eta_secs(
                        at_ms - resumed_ms,
                        since.elapsed(),
                        plan.len_ms.saturating_sub(at_ms),
                    );
                });
            };
            // No speech by VAD: nothing to transcribe.
            let (segments, device) = if chunk.speech_ms == 0 {
                (Vec::new(), None)
            } else {
                match self.chunk(id, &dir, chunk, (&model, &vad), report) {
                    Ok((segments, device)) => (segments, Some(device)),
                    Err(end) => return end,
                }
            };
            done.push(ChunkResult {
                chunk: index as u32,
                start_ms: chunk.start_ms,
                end_ms: chunk.end_ms,
                segments,
            });
            let saved = store.update(id, |m| {
                write_partial(&partial, &done).map_err(|e| format!("{PARTIAL} 저장 실패: {e}"))?;
                m.chunks_done = done.len() as u32;
                if let Some(device) = device {
                    m.backend = Some(device.name().into());
                }
                Ok(())
            });
            if let Err(e) = saved {
                return End::Failed(e);
            }
            self.set_progress(id, |p| p.chunk = done.len() as u32);
        }

        let segments = done.iter().flat_map(|r| &r.segments).collect();
        let finished = store.update(id, |m| {
            if !can_transition(m.status, Status::Transcribed) {
                return Err(format!("전사를 마칠 수 없는 상태입니다: {:?}", m.status));
            }
            let bytes =
                serde_json::to_vec_pretty(&Transcript { segments }).map_err(|e| e.to_string())?;
            write_atomic(&dir.join("transcript.json"), &bytes)
                .map_err(|e| format!("transcript.json 저장 실패: {e}"))?;
            m.status = Status::Transcribed;
            Ok(())
        });
        match finished {
            Ok(_) => End::Done(Status::Transcribed),
            Err(e) => End::Failed(e),
        }
    }

    /// Transcribes, then summarizes in the same slot (architecture 8.4), or only summarizes
    /// (`summary_only`).
    fn process(&self, store: &Store, id: &str) -> End {
        let meta = match store.get(id) {
            Ok(meta) => meta,
            Err(e) => return End::Skipped(e),
        };
        if !summary_only(store, &meta) {
            match self.transcribe(store, id) {
                End::Done(_) => {}
                end => return end,
            }
        }
        self.summarize(store, id)
    }

    /// `summarizing`, written in `m`. With `send` false (SUM-10, or a transcript that cannot be
    /// read) nothing goes anywhere, so no route is needed and none is pinned. Otherwise only once
    /// the meeting's route resolves (plan 5.4) and, for the built-in route, once it holds the lease
    /// (ENG1), with the connection pinned; else the meeting stays as it is, with the reason when
    /// the route does not resolve, back in line when the lease is taken. Under the meeting's lock,
    /// so a change of connection cannot slip in between.
    fn begin(
        &self,
        id: &str,
        m: &mut Meta,
        (settings, ready): (&Settings, bool),
        send: bool,
    ) -> Result<Option<(LlmConnection, Option<u64>)>, End> {
        let sending = if send {
            let connection = summarize::route(settings, m, ready).map_err(|wait| {
                // Only where it waits: a `summarize_failed` one keeps its error, and a retry
                // offers only a connection that resolves (`commands::retry_summary`).
                if matches!(m.status, Status::Transcribed | Status::Paused) {
                    m.wait = Some(wait);
                }
                // Not a failure: no notification, and it stays as it is.
                End::Skipped(format!("요약 경로를 기다립니다: {wait:?}"))
            })?;
            let generation = if connection.name == BUILTIN {
                Some(self.engine_generation(id)?)
            } else {
                None
            };
            m.llm_connection = Some(connection.name.clone());
            m.summarize_local = connection.is_local;
            Some((connection, generation))
        } else {
            None
        };
        m.wait = None;
        m.status = Status::Summarizing;
        m.error = None;
        Ok(sending)
    }

    fn summarizing(&self, id: &str) {
        self.set_progress(id, |p| {
            *p = Progress {
                id: p.id.clone(),
                status: Status::Summarizing,
                chunk: 0,
                chunks: 0,
                percent: 0,
                eta_secs: None,
                position: None,
            };
        });
    }

    /// Architecture 6. A transcript with almost no speech is not sent anywhere (SUM-10), and
    /// neither it nor one that cannot be read needs a connection. Otherwise the meeting's
    /// connection (`begin`) gets it as a task this thread waits on, with its key from the keychain;
    /// the built-in route's key and address come from its engine, which starts here first.
    fn summarize(&self, store: &Store, id: &str) -> End {
        let dir = match store.dir_for(id) {
            Ok(dir) => dir,
            Err(e) => return End::Failed(e),
        };
        let lines = store.transcript(id).map(|segments| {
            segments
                .into_iter()
                .map(|s| s.text.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect::<Vec<String>>()
        });
        // `summary`: the LLM's text and what the grounding check made of it.
        let review = |summary: Option<(&str, &grounding::Grounded)>| {
            store.update(id, |m| {
                if m.status != Status::Summarizing {
                    return Err(format!("요약을 마칠 수 없는 상태입니다: {:?}", m.status));
                }
                // The checked text that review edits and the edit count compares against, then
                // the LLM's own (SUM-05, SND-03). A failure in between fails the summary, so a new
                // raw text never sits next to an old checked one.
                if let Some((raw, checked)) = summary {
                    for (name, text) in [
                        ("summary.generated.md", checked.text.as_str()),
                        ("summary.raw.md", raw),
                    ] {
                        write_atomic(&dir.join(name), text.as_bytes())
                            .map_err(|e| format!("{name} 저장 실패: {e}"))?;
                    }
                    if let (Some(title), _) = summarize::split_title(&checked.text) {
                        m.title = title;
                    }
                }
                m.grounding_replaced = summary.map_or(0, |(_, g)| g.replaced);
                m.grounding_checked = summary.map(|(_, g)| g.checked);
                m.no_speech = summary.is_none();
                m.status = Status::Review;
                Ok(())
            })
        };
        // SUM-10, and a transcript that cannot be read: nothing is sent.
        let send = lines.as_ref().is_ok_and(|l| !summarize::too_short(l));
        let (settings, ready) = (self.settings(), self.builtin_ready());
        let mut begun = Err(End::Skipped(String::new()));
        let started = store.update(id, |m| {
            // `summarizing` itself too, like `transcribing` above.
            if m.status != Status::Summarizing && !can_transition(m.status, Status::Summarizing) {
                return Err(format!("요약할 수 없는 상태입니다: {:?}", m.status));
            }
            begun = self.begin(id, m, (&settings, ready), send);
            Ok(())
        });
        if let Err(e) = started {
            return End::Skipped(e);
        }
        let begun = match begun {
            Ok(begun) => begun,
            Err(end) => return end,
        };
        self.summarizing(id);
        let (mut connection, generation, lines) = match (begun, lines) {
            (Some((connection, generation)), Ok(lines)) => (connection, generation, lines),
            (_, lines) => {
                return match lines.and_then(|_| review(None)) {
                    Ok(_) => End::Done(Status::Review),
                    Err(e) => End::Failed(e),
                };
            }
        };
        let (key, recover) = match generation {
            Some(generation) => {
                let device = first_device(settings.backend, settings.summary_gpu_failed);
                let endpoint = match self.sidecar.start(generation, device) {
                    Ok(endpoint) => endpoint,
                    Err(failure) => return self.engine_end(id, failure),
                };
                connection.base_url = endpoint.base_url.clone();
                let key = Some(endpoint.key.clone());
                (key, Some(self.sidecar.on_failure(endpoint)))
            }
            None => match (self.secret)(&connection.name) {
                Ok(key) => (key, None),
                Err(e) => return End::Failed(e),
            },
        };
        let (cloud, builtin) = (!connection.is_local, recover.is_some());
        let (transcript, glossary) = (lines.join("\n"), settings.glossary);
        let (app, run_id, asked) = (self.app.clone(), id.to_string(), glossary.clone());
        let task = async move {
            let on_step = |done: u32, known: u32| {
                app.state::<Queue<R>>().set_progress(&run_id, |p| {
                    p.chunk = done;
                    p.chunks = known;
                    p.percent = (done * 100 / known.max(1)) as u8;
                });
            };
            summarize::summarize(&connection, key, &asked, &lines, on_step, recover).await
        };
        let (summary, timings) = match self.request(id, cloud, task) {
            Ok(Ok(done)) => done,
            Ok(Err(failure)) => return End::Failed(failure.message()),
            Err(end) => return end,
        };
        if builtin {
            log::info!(
                "이 PC 요약 끝 ({id}): 입력 {}토큰 {:.1}초, 출력 {}토큰 {:.1}초",
                timings.prompt_n,
                timings.prompt_ms / 1000.0,
                timings.predicted_n,
                timings.predicted_ms / 1000.0
            );
        }
        let grounded = grounding::check(&summary, &transcript, &glossary);
        if !grounded.checked {
            log::warn!("요약의 액션 아이템을 읽지 못해 근거 검사를 하지 못했습니다 ({id})");
        }
        match review(Some((&summary, &grounded))) {
            Ok(_) => End::Done(Status::Review),
            Err(e) => End::Failed(e),
        }
    }

    /// Runs a summary as its own task and waits for it. The task starts under the lock, so a stop
    /// never misses it: a delete or quit aborts it, and so does a recording unless it is a cloud
    /// summary (SUM-09).
    fn request<T: Send + 'static>(
        &self,
        id: &str,
        cloud: bool,
        task: impl Future<Output = T> + Send + 'static,
    ) -> Result<T, End> {
        let task = {
            let mut inner = lock(&self.inner);
            let Some(job) = inner.jobs.get_mut(id) else {
                return Err(End::Stopped(Stop::Delete));
            };
            // A recording that started just before has stopped this run already.
            if let Some(stop) = job.stop {
                return Err(End::Stopped(stop));
            }
            let task = tauri::async_runtime::spawn(task);
            let abort = task.inner().abort_handle();
            job.abort = Some(Box::new(move || abort.abort()));
            job.cloud = cloud;
            task
        };
        let result = tauri::async_runtime::block_on(task);
        let inner = lock(&self.inner);
        if let Some(stop) = inner.jobs.get(id).map_or(Some(Stop::Delete), |j| j.stop) {
            return Err(End::Stopped(stop));
        }
        result.map_err(|e| End::Failed(format!("요약 작업이 비정상 종료했습니다: {e}")))
    }

    /// Writes how the run ended, frees its slot and the lease and starts the next meeting. Only a
    /// recording stop and `Requeue` keep the meeting in line.
    fn end(&self, store: &Store, id: &str, end: End) {
        let requeue = matches!(end, End::Stopped(Stop::Recording) | End::Requeue);
        let finished = matches!(end, End::Done(_) | End::Failed(_));
        let status = match end {
            End::Done(status) => Some(status),
            End::Stopped(Stop::Recording) => {
                // Nothing on disk claims work that is not running (same as recovery, 8.4).
                if let Err(e) = store.set_status(id, Status::Paused) {
                    log::error!("일시정지 기록 실패 ({id}): {e}");
                }
                Some(Status::Paused)
            }
            End::Stopped(Stop::Delete | Stop::Quit) => None,
            End::Failed(message) => {
                log::error!("처리 실패 ({id}): {message}");
                let failed = store.update(id, |m| {
                    let to = if m.status == Status::Summarizing {
                        Status::SummarizeFailed
                    } else {
                        Status::TranscribeFailed
                    };
                    if can_transition(m.status, to) {
                        m.status = to;
                        m.error = Some(message);
                    }
                    Ok(())
                });
                match failed {
                    Ok(m) => Some(m.status),
                    Err(e) => {
                        log::error!("실패 상태 기록 실패 ({id}): {e}");
                        None
                    }
                }
            }
            End::Skipped(reason) => {
                log::info!("처리 건너뜀 ({id}): {reason}");
                store.get(id).ok().map(|m| m.status)
            }
            // Its place in line shows the status it waits in, maybe `transcribed` by now.
            End::Requeue => store.get(id).ok().map(|m| m.status),
        };
        {
            let mut inner = lock(&self.inner);
            // A delete or quit that landed after the run stopped wins: no ghost back in line.
            let halted = matches!(
                inner.jobs.get(id).and_then(|j| j.stop),
                Some(Stop::Delete | Stop::Quit)
            );
            let requeue = requeue && !halted;
            // Plan 5.3: no idle engine. It would hold 4 GB while the next transcription runs. The
            // meetings passed over for the lease may start now.
            if inner.sched.release_lease(id) {
                self.sidecar.stop("요약 끝");
            }
            inner.sched.finished(id);
            inner.jobs.remove(id);
            if !requeue {
                // Finished while a recording put it back in line: nothing left to run.
                inner.sched.waiting.retain(|w| w != id);
            } else if !inner.sched.waiting.contains(&id.to_string()) {
                inner.sched.waiting.push_front(id.to_string());
            }
            let waiting = requeue;
            let entry = if waiting {
                inner.progress.get_mut(id).cloned()
            } else {
                inner.progress.remove(id)
            };
            if let (Some(mut p), Some(status)) = (entry, status) {
                p.status = status;
                p.eta_secs = None;
                if !waiting {
                    // Out of line, even if a recording put it back before this run could stop.
                    p.position = None;
                }
                if matches!(status, Status::Transcribed | Status::Review) {
                    p.percent = 100;
                }
                if waiting {
                    inner.progress.insert(id.to_string(), p.clone());
                }
                self.emit(&p);
            }
            self.publish_positions(&mut inner);
        }
        self.pump();
        // APP-03: done or failed, said with its title; a click opens it.
        if finished && let Ok(meta) = store.get(id) {
            let event = match meta.status {
                Status::Review if meta.no_speech => Some("음성이 거의 감지되지 않았습니다"),
                Status::Review => Some("회의 요약이 준비됐습니다"),
                Status::SummarizeFailed => Some("요약하지 못했습니다"),
                Status::TranscribeFailed => Some("전사하지 못했습니다"),
                _ => None,
            };
            if let Some(event) = event {
                let title = if meta.title.is_empty() {
                    "제목 없음"
                } else {
                    &meta.title
                };
                crate::notify(&self.app, format!("{event}: {title}"), id);
            }
        }
        crate::refresh_tray(&self.app);
    }
}

/// A running meeting's thread.
fn run<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let queue = app.state::<Queue<R>>();
    let store = app.state::<Store>();
    // A panic still frees the slot and records the failure. (Release builds abort on panic.)
    let end = panic::catch_unwind(AssertUnwindSafe(|| queue.process(&store, id)))
        .unwrap_or_else(|_| End::Failed("처리 중 내부 오류가 났습니다".into()));
    queue.end(&store, id, end);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn two_run_at_once_and_the_rest_wait_in_order() {
        let mut s = Sched::default();
        for id in ["a", "b", "c", "d"] {
            assert!(s.enqueue(id));
        }
        assert!(!s.enqueue("c"));
        assert_eq!(s.start_next(2, |_| true), ids(&["a", "b"]));
        assert_eq!(s.start_next(2, |_| true), Vec::<String>::new());
        assert_eq!(
            (s.position("a"), s.position("c"), s.position("d")),
            (None, Some(1), Some(2))
        );
        // A running meeting is not queued twice.
        assert!(!s.enqueue("a"));

        s.finished("b");
        assert_eq!(s.start_next(2, |_| true), ids(&["c"]));
        assert_eq!(s.position("d"), Some(1));
        assert!(s.remove("c"));
        assert!(!s.remove("d"));
        assert_eq!(s.position("d"), None);
    }

    #[test]
    fn recording_puts_running_meetings_back_first_and_starts_nothing() {
        let mut s = Sched::default();
        for id in ["a", "b", "c"] {
            s.enqueue(id);
        }
        s.start_next(2, |_| true);
        assert_eq!(s.suspend(|_| false), ids(&["a", "b"]));
        assert_eq!(
            (s.position("a"), s.position("b"), s.position("c")),
            (Some(1), Some(2), Some(3))
        );
        assert!(s.start_next(2, |_| true).is_empty());

        // Recording over, but a's stopped run has not wound down: nothing jumps ahead of it.
        s.recording = false;
        assert!(s.start_next(2, |_| true).is_empty());
        s.finished("b");
        assert!(s.start_next(2, |_| true).is_empty());
        s.finished("a");
        assert_eq!(s.start_next(2, |_| true), ids(&["a", "b"]));
        assert_eq!(s.position("c"), Some(1));
    }

    /// ENG1: one at a time once the summary model is in. ENG2: a meeting that cannot start yet
    /// keeps its place, and the ones behind it go.
    #[test]
    fn slots_follow_the_summary_model_and_a_meeting_that_cannot_start_is_passed_over() {
        assert_eq!((slots(true), slots(false)), (1, 2));
        let mut s = Sched::default();
        for id in ["a", "b", "c"] {
            s.enqueue(id);
        }
        assert_eq!(s.start_next(slots(true), |_| true), ids(&["a"]));
        assert!(s.start_next(slots(true), |_| true).is_empty());
        assert_eq!(s.start_next(slots(false), |id| id != "b"), ids(&["c"]));
        assert_eq!(s.position("b"), Some(1));
    }

    /// ENG1: the built-in engine serves one summary; a meeting that finds it taken goes back
    /// first in line and is passed over, never started again, until the lease is given back.
    #[test]
    fn a_meeting_blocked_by_the_lease_waits_without_respinning() {
        let mut s = Sched::default();
        for id in ["a", "b", "c"] {
            s.enqueue(id);
        }
        assert_eq!(s.start_next(2, |_| true), ids(&["a", "b"]));
        assert!(s.take_lease("a") && s.take_lease("a"));
        assert!(!s.take_lease("b"));
        // b's run ends as `Requeue` does.
        s.finished("b");
        s.waiting.push_front("b".into());
        assert_eq!(s.start_next(2, |_| true), ids(&["c"]));
        s.finished("c");
        for _ in 0..3 {
            assert!(s.start_next(2, |_| true).is_empty());
        }
        assert_eq!(s.position("b"), Some(1));
        assert!(!s.release_lease("b"));
        assert!(s.release_lease("a"));
        s.finished("a");
        assert_eq!(s.start_next(1, |_| true), ids(&["b"]));
        assert!(s.take_lease("b"));
    }

    /// SUM-09: a cloud summary keeps its slot through a recording; the freed slot stays empty.
    #[test]
    fn a_cloud_summary_keeps_its_slot_through_a_recording() {
        let mut s = Sched::default();
        for id in ["a", "b", "c"] {
            s.enqueue(id);
        }
        s.start_next(2, |_| true);
        assert_eq!(s.suspend(|id| id == "b"), ids(&["a"]));
        assert_eq!(
            (s.position("a"), s.position("b"), s.position("c")),
            (Some(1), None, Some(2))
        );
        s.finished("b");
        assert!(s.start_next(2, |_| true).is_empty());
    }

    #[test]
    fn gpu_crash_retries_once_then_marks_gpu_failed_and_uses_the_cpu() {
        let mut run = ChunkRun::new(Device::Gpu);
        assert_eq!(run.after(Exit::Crash, false), Next::Run(Device::Gpu));
        assert_eq!(run.after(Exit::Crash, false), Next::GpuFailed);
        assert_eq!(run.device, Device::Cpu);
        assert_eq!(run.after(Exit::Crash, false), Next::Fail);

        let mut run = ChunkRun::new(Device::Gpu);
        assert_eq!(run.after(Exit::Crash, false), Next::Run(Device::Gpu));
        assert_eq!(run.after(Exit::Done(Device::Gpu), false), Next::Accept);
        assert_eq!(run.device, Device::Gpu);
    }

    #[test]
    fn input_and_model_errors_fail_without_blaming_the_gpu() {
        for exit in [Exit::Input, Exit::Model] {
            let mut run = ChunkRun::new(Device::Gpu);
            assert_eq!(run.after(exit, false), Next::Fail);
            assert_eq!((run.gpu_crashes, run.device), (0, Device::Gpu));
        }
        let code = |code| Ended {
            code,
            ..Ended::default()
        };
        assert_eq!(exit_of(code(Some(3)), None), Exit::Model);
        assert_eq!(exit_of(code(Some(2)), None), Exit::Input);
        assert_eq!(
            exit_of(code(Some(0)), Some(Device::Cpu)),
            Exit::Done(Device::Cpu)
        );
        // Exit 0 without a result, a signal, any other code: a crash.
        assert_eq!(exit_of(code(Some(0)), None), Exit::Crash);
        assert_eq!(exit_of(code(None), None), Exit::Crash);
        assert_eq!(exit_of(code(Some(139)), None), Exit::Crash);
    }

    #[test]
    fn a_model_failure_says_which_it_is() {
        let intact = model_failure(Check::Intact, "ggml error");
        assert!(intact.contains("메모리") && intact.ends_with("ggml error"));
        let damaged = model_failure(Check::Damaged, "ggml error");
        assert!(damaged.contains("손상") && damaged.contains("다시 받아"));
        // Not "deleted": a damaged file is left alone while a download runs.
        assert!(!damaged.contains("지웠"));
        let missing = model_failure(Check::Missing, "ggml error");
        assert!(missing.contains("없거나 읽을 수 없습니다") && missing.contains("다시 받아"));
    }

    #[test]
    fn a_cpu_without_the_build_baseline_cannot_transcribe() {
        assert_eq!(cpu_failure(&[true; 4]), None);
        let missing = cpu_failure(&[false, true, true, true]).unwrap();
        assert!(missing.contains("AVX2 등의 명령어를 지원하지 않아"));
        assert_eq!(cpu_failure(&[true, true, true, false]), Some(missing));
    }

    #[test]
    fn the_memory_killer_is_not_a_gpu_crash_but_a_stall_is() {
        let killed = Ended {
            killed: true,
            ..Ended::default()
        };
        assert_eq!(exit_of(killed, None), Exit::OutOfMemory);
        let mut run = ChunkRun::new(Device::Gpu);
        assert_eq!(run.after(Exit::OutOfMemory, false), Next::Fail);
        assert_eq!(run.gpu_crashes, 0);

        let stalled = Ended {
            stalled: true,
            killed: true,
            ..Ended::default()
        };
        assert_eq!(exit_of(stalled, None), Exit::Stalled);
        let mut run = ChunkRun::new(Device::Gpu);
        assert_eq!(run.after(Exit::Stalled, false), Next::Run(Device::Gpu));
        assert_eq!(run.after(Exit::Stalled, false), Next::GpuFailed);
    }

    #[test]
    fn abnormal_gpu_result_is_redone_once_on_the_cpu() {
        let mut run = ChunkRun::new(Device::Gpu);
        assert_eq!(
            run.after(Exit::Done(Device::Gpu), true),
            Next::Run(Device::Cpu)
        );
        assert_eq!(run.after(Exit::Done(Device::Cpu), true), Next::Accept);
        // Already on the CPU (forced, or the GPU fell back): kept.
        let mut run = ChunkRun::new(Device::Gpu);
        assert_eq!(run.after(Exit::Done(Device::Cpu), true), Next::Accept);
        let mut run = ChunkRun::new(Device::Cpu);
        assert_eq!(run.after(Exit::Done(Device::Cpu), true), Next::Accept);
    }

    #[test]
    fn backend_setting_picks_the_first_device() {
        assert_eq!(first_device(Backend::Auto, false), Device::Gpu);
        assert_eq!(first_device(Backend::Auto, true), Device::Cpu);
        assert_eq!(first_device(Backend::Gpu, true), Device::Gpu);
        assert_eq!(first_device(Backend::Cpu, false), Device::Cpu);
    }

    fn chunk(start_ms: u64, end_ms: u64) -> Chunk {
        Chunk {
            start_ms,
            end_ms,
            speech_ms: 1,
        }
    }

    fn result(index: u32, c: &Chunk) -> ChunkResult {
        ChunkResult {
            chunk: index,
            start_ms: c.start_ms,
            end_ms: c.end_ms,
            segments: vec![Segment {
                start_ms: c.start_ms,
                end_ms: c.end_ms,
                text: format!("구간 {index}"),
            }],
        }
    }

    #[test]
    fn resumes_after_the_last_finished_chunk() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PARTIAL);
        let plan = [chunk(0, 10), chunk(10, 20), chunk(20, 30)];
        assert!(read_partial(&path, &plan).is_empty());

        write_partial(&path, &[result(0, &plan[0]), result(1, &plan[1])]).unwrap();
        assert_eq!(read_partial(&path, &plan).len(), 2);

        // A torn last line (an older appending writer, a disk error) is redone, not trusted.
        let mut text = fs::read_to_string(&path).unwrap();
        text.truncate(text.len() - 10);
        fs::write(&path, text).unwrap();
        assert_eq!(read_partial(&path, &plan), [result(0, &plan[0])]);

        // Lines that do not match the plan stop the resume there.
        write_partial(&path, &[result(0, &plan[0]), result(1, &chunk(10, 25))]).unwrap();
        assert_eq!(read_partial(&path, &plan).len(), 1);
        write_partial(&path, &[result(1, &plan[1])]).unwrap();
        assert!(read_partial(&path, &plan).is_empty());
    }

    #[test]
    fn percent_and_remaining_time() {
        assert_eq!(percent(0, 0, 0, 0), 0);
        assert_eq!(percent(600_000, 600_000, 50, 1_800_000), 50);
        assert_eq!(percent(1_800_000, 0, 0, 1_800_000), 100);
        assert_eq!(eta_secs(30_000, Duration::from_secs(60), 600_000), None);
        assert_eq!(eta_secs(600_000, Duration::from_secs(5), 600_000), None);
        // 10 min of audio in 60 s: 20 min left takes 120 s.
        assert_eq!(
            eta_secs(600_000, Duration::from_secs(60), 1_200_000),
            Some(120)
        );
    }

    #[test]
    fn a_delete_is_never_turned_back_into_a_pause() {
        let mut job = Job::default();
        job.halt(Stop::Recording);
        job.halt(Stop::Delete);
        job.halt(Stop::Recording);
        assert_eq!(job.stop, Some(Stop::Delete));
    }

    fn job(inner: &Mutex<Inner>, id: &str) {
        let mut inner = lock(inner);
        inner.sched.enqueue(id);
        inner.sched.start_next(2, |_| true);
        inner.jobs.insert(id.into(), Job::default());
    }

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    /// TRN-09 with a real process: a recording starting kills a running worker at once, the
    /// kill does not count as a crash, and the meeting is first in line again.
    #[cfg(unix)]
    #[test]
    fn recording_start_kills_a_running_worker_within_a_second() {
        let inner = Mutex::new(Inner::default());
        job(&inner, "a");
        let command = sh(r#"echo '{"type":"progress","pct":5}'; exec sleep 30"#);
        let started = Instant::now();
        let mut lines = Vec::new();
        let ended = thread::scope(|s| {
            s.spawn(|| {
                thread::sleep(Duration::from_millis(300));
                lock(&inner).suspend();
            });
            run_worker(&inner, "a", command, STALL, |line| lines.push(line))
        });
        assert!(matches!(ended, Err(End::Stopped(Stop::Recording))));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(lines, [Line::Progress { pct: 5 }]);
        assert_eq!(lock(&inner).sched.position("a"), Some(1));

        // Once stopped, the run does not start another worker.
        let again = run_worker(&inner, "a", Command::new("true"), STALL, |_| {});
        assert!(matches!(again, Err(End::Stopped(Stop::Recording))));
    }

    #[cfg(unix)]
    #[test]
    fn worker_lines_and_exit_codes_come_through() {
        let inner = Mutex::new(Inner::default());
        job(&inner, "a");
        let mut lines = Vec::new();
        // Noise and a line that is not UTF-8 are skipped; the result still arrives.
        let ended = run_worker(
            &inner,
            "a",
            sh(
                r#"echo 'ggml_metal_init: noise'; printf '\377\376\n'; echo '{"type":"result","device":"cpu","segments":[]}'"#,
            ),
            STALL,
            |line| lines.push(line),
        );
        assert_eq!(ended.ok().map(|e| e.code), Some(Some(0)));
        assert_eq!(
            lines,
            [Line::Result {
                device: Device::Cpu,
                segments: vec![]
            }]
        );
        let ended = |script| run_worker(&inner, "a", sh(script), STALL, |_| {}).ok();
        assert_eq!(ended("exit 3").map(|e| e.code), Some(Some(3)));
        let segv = ended("kill -SEGV $$").unwrap();
        assert_eq!((segv.code, segv.killed), (None, false));
        // SIGKILL nobody here sent: the memory killer.
        assert!(ended("kill -KILL $$").unwrap().killed);
    }

    #[cfg(unix)]
    #[test]
    fn a_silent_worker_is_killed_and_reported_as_stalled() {
        let inner = Mutex::new(Inner::default());
        job(&inner, "a");
        let started = Instant::now();
        let ended = run_worker(
            &inner,
            "a",
            sh(r#"echo '{"type":"progress","pct":1}'; exec sleep 30"#),
            Duration::from_millis(300),
            |_| {},
        )
        .ok()
        .unwrap();
        assert!(ended.stalled);
        assert_eq!(exit_of(ended, None), Exit::Stalled);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// A queue on the mock runtime whose engine is the sidecar tests' fake (no meeting runs).
    fn engine_queue(
        dir: &Path,
        supervisor: &str,
        reap: Duration,
    ) -> (
        tauri::App<tauri::test::MockRuntime>,
        Queue<tauri::test::MockRuntime>,
    ) {
        use crate::sidecar::tests::{fake, fake_llama};
        let app = tauri::test::mock_app();
        app.manage(Models::new(dir.join("models")));
        // For `end`, which pumps the line: no model, so nothing starts.
        app.manage(Store::new(dir.join("meetings")).unwrap());
        app.manage(SettingsStore::new(dir.join("settings.json")));
        let fake = fake(dir, supervisor, fake_llama("ready"), reap);
        let queue = Queue {
            sidecar: Arc::new(fake.sidecar),
            ..Queue::new(app.handle().clone(), PathBuf::new())
        };
        (app, queue)
    }

    /// SUM-09, plan 5.3: a recording stops the engine under the queue lock without waiting for it
    /// (the supervisor here takes 2 s to go), and a summary begun before cannot start another.
    #[test]
    fn a_recording_stops_the_engine_at_once_and_none_starts_in_it() {
        use crate::sidecar::tests::{gone, port};
        let tmp = tempfile::tempdir().unwrap();
        let (_app, queue) = engine_queue(tmp.path(), "deaf_supervisor", Duration::from_secs(2));
        let generation = queue.sidecar.generation();
        let endpoint = queue.sidecar.start(generation, Device::Gpu).unwrap();

        let started = Instant::now();
        queue.recording_started();
        assert!(started.elapsed() < Duration::from_millis(500));
        let again = queue.sidecar.start(generation, Device::Gpu);
        assert!(matches!(again, Err(crate::sidecar::Failure::Stopped)));
        // At most the one winding down, and then none.
        assert!(gone(port(&endpoint.base_url)));
    }

    /// A run of `id` without a thread: the test plays its end.
    fn running(queue: &Queue<tauri::test::MockRuntime>, id: &str) {
        let mut inner = lock(&queue.inner);
        inner.sched.enqueue(id);
        inner.sched.start_next(2, |_| true);
        inner.jobs.insert(id.into(), Job::default());
    }

    /// `cancel` waits for the run to end, which the test plays once the stop has landed.
    fn delete(queue: &Queue<tauri::test::MockRuntime>, id: &str) {
        thread::scope(|s| {
            let deleting = s.spawn(|| queue.cancel(id));
            while lock(&queue.inner).jobs[id].stop.is_none() {
                thread::sleep(Duration::from_millis(10));
            }
            lock(&queue.inner).sched.finished(id);
            deleting.join().unwrap();
        });
    }

    /// ENG3: the generation comes only with the job's stop checked, and marks the job, so a delete
    /// after it stops the engine and a delete before it leaves nothing to start.
    #[test]
    fn deleting_the_meeting_whose_builtin_summary_runs_stops_the_engine() {
        use crate::sidecar::tests::{gone, port};
        let tmp = tempfile::tempdir().unwrap();
        let (_app, queue) = engine_queue(tmp.path(), "supervisor", Duration::from_secs(10));
        assert!(matches!(
            queue.engine_generation("nothing"),
            Err(End::Stopped(Stop::Delete))
        ));
        running(&queue, "deleted");
        delete(&queue, "deleted");
        assert!(matches!(
            queue.engine_generation("deleted"),
            Err(End::Stopped(Stop::Delete))
        ));

        running(&queue, "builtin");
        let generation = queue.engine_generation("builtin").ok().unwrap();
        let endpoint = queue.sidecar.start(generation, Device::Gpu).unwrap();
        // A meeting that is not using the engine leaves it alone.
        running(&queue, "cloud");
        delete(&queue, "cloud");
        assert_eq!(queue.sidecar.generation(), generation);
        delete(&queue, "builtin");
        assert!(gone(port(&endpoint.base_url)));
        let again = queue.sidecar.start(generation, Device::Gpu);
        assert!(matches!(again, Err(crate::sidecar::Failure::Stopped)));
    }

    /// Plan 5.3 "즉시 종료": a built-in summary that ends takes the engine with it, and the summary
    /// model may change again (ENG18).
    #[test]
    fn a_builtin_summary_ending_stops_the_engine() {
        use crate::sidecar::tests::{gone, port};
        let tmp = tempfile::tempdir().unwrap();
        let (app, queue) = engine_queue(tmp.path(), "supervisor", Duration::from_secs(10));
        running(&queue, "a");
        let generation = queue.engine_generation("a").ok().unwrap();
        let endpoint = queue.sidecar.start(generation, Device::Gpu).unwrap();
        let refused = crate::commands::not_in_use(&[&model::SUMMARY], queue.engine_running());
        assert!(refused.is_err());

        queue.end(&app.state::<Store>(), "a", End::Done(Status::Review));
        assert!(gone(port(&endpoint.base_url)));
        let until = Instant::now() + Duration::from_secs(5);
        while queue.engine_running() {
            assert!(Instant::now() < until, "still winding down");
            thread::sleep(Duration::from_millis(10));
        }
        let allowed = crate::commands::not_in_use(&[&model::SUMMARY], queue.engine_running());
        assert!(allowed.is_ok());
    }

    /// ENG1: two meetings running when the summary model arrives both reach the built-in route;
    /// the second leaves the engine alone, waits first in line without starting again, and goes
    /// once the first gives the lease back.
    #[test]
    fn the_model_arriving_mid_run_never_lets_two_builtin_summaries_run() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, queue) = engine_queue(tmp.path(), "supervisor", Duration::from_secs(10));
        running(&queue, "a");
        running(&queue, "b");
        let generation = queue.engine_generation("a").ok().unwrap();
        assert!(matches!(queue.engine_generation("b"), Err(End::Requeue)));
        assert_eq!(queue.sidecar.generation(), generation);

        let store = app.state::<Store>();
        queue.end(&store, "b", End::Requeue);
        {
            let mut inner = lock(&queue.inner);
            assert_eq!(inner.sched.position("b"), Some(1));
            assert!(inner.sched.start_next(2, |_| true).is_empty());
        }
        queue.end(&store, "a", End::Done(Status::Review));
        let mut inner = lock(&queue.inner);
        assert_eq!(inner.sched.start_next(1, |_| true), ids(&["b"]));
        assert!(inner.sched.take_lease("b"));
    }

    /// The queue end to end on the mock runtime, with a shell script as the worker, the `tiny`
    /// model "installed" as sparse files of the right size, stub LLM servers, the sidecar tests'
    /// fake llama-server and a stand-in keychain. Tests that run the worker are unix-only; the
    /// rest (meetings that start from a transcript) run on Windows too.
    #[cfg_attr(not(unix), allow(dead_code, unused_imports))]
    mod app {
        use super::*;
        use crate::settings::LlmConnection;
        use crate::summarize::split_title;
        use crate::summarize::tests::{SUMMARY, completion, connection, reply, serve};
        use crate::transcribe::model;
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::MockRuntime;

        type Mock = Queue<MockRuntime>;

        /// Two chunks; the second has no speech, so no worker runs for it.
        const PLAN: &str = r#"{"type":"plan","len_ms":20000,"chunks":[{"start_ms":0,"end_ms":10000,"speech_ms":5000},{"start_ms":10000,"end_ms":20000,"speech_ms":0}]}"#;
        /// A result echoing the chunk and device, plus a listed hallucination.
        const RESULT: &str = r#"echo '{"type":"progress","pct":40}'; echo "{\"type\":\"result\",\"device\":\"$5\",\"segments\":[{\"start_ms\":$3,\"end_ms\":$4,\"text\":\"구간 $3\"},{\"start_ms\":$3,\"end_ms\":$4,\"text\":\"시청해주셔서 감사합니다.\"}]}""#;
        /// A result with enough speech to summarize (SUM-10 wants 30 characters).
        const SPEECH: &str = r#"echo "{\"type\":\"result\",\"device\":\"$5\",\"segments\":[{\"start_ms\":$3,\"end_ms\":$4,\"text\":\"이번 주 배포는 목요일 오후에 하고 릴리스 노트는 민수 씨가 금요일까지 씁니다\"}]}""#;

        struct Fixture {
            app: tauri::App<MockRuntime>,
            events: Arc<Mutex<Vec<serde_json::Value>>>,
            gpu_notices: Arc<Mutex<u32>>,
            tmp: tempfile::TempDir,
        }

        /// One test with a summary model on disk at a time (`Fixture::install_summary`): taken
        /// before the fixture, so it is dropped after the fixture's temp folder, model and all.
        fn summary_model() -> MutexGuard<'static, ()> {
            static ONE: Mutex<()> = Mutex::new(());
            ONE.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// `transcribe`: the shell for `--transcribe-worker <dir> <start> <end> <device> ...`.
        fn fixture(transcribe: &str, backend: Backend) -> Fixture {
            fixture_with(transcribe, backend, "ready")
        }

        /// The built-in engine's llama-server is the sidecar tests' fake doing `behave`, in
        /// `engine/`. It only runs once the summary model is "installed" (`install_summary`).
        fn fixture_with(transcribe: &str, backend: Backend, behave: &str) -> Fixture {
            use crate::sidecar::tests::{fake, fake_llama};
            let tmp = tempfile::tempdir().unwrap();
            let app = tauri::test::mock_app();
            app.manage(Store::new(tmp.path().join("meetings")).unwrap());
            let settings = SettingsStore::new(tmp.path().join("settings.json"));
            let chosen = Settings {
                model: "tiny".into(),
                backend,
                ..Settings::default()
            };
            settings.save(chosen).unwrap();
            app.manage(settings);
            let models = tmp.path().join("models");
            fs::create_dir(&models).unwrap();
            for f in model::needed("tiny").unwrap() {
                let file = fs::File::create(models.join(f.file)).unwrap();
                file.set_len(f.size).unwrap();
            }
            app.manage(Models::new(models));
            let exe = tmp.path().join("worker.sh");
            let script = format!(
                "#!/bin/sh\ncase \"$1\" in\n--plan-chunks)\n echo '{{\"type\":\"progress\",\"pct\":50}}'; echo '{PLAN}' ;;\n--transcribe-worker)\n {transcribe} ;;\nesac\n"
            );
            fs::write(&exe, script).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let engine = fake(
                &tmp.path().join("engine"),
                "supervisor",
                fake_llama(behave),
                crate::sidecar::LIMITS.reap,
            );
            // Never the real keychain.
            app.manage(Queue {
                secret: Box::new(|_| Ok(Some("sk-test".into()))),
                sidecar: Arc::new(engine.sidecar),
                ..Queue::new(app.handle().clone(), exe)
            });

            let events = Arc::new(Mutex::new(Vec::new()));
            let seen = events.clone();
            app.listen_any("meeting://progress", move |e| {
                seen.lock()
                    .unwrap()
                    .push(serde_json::from_str(e.payload()).unwrap())
            });
            let gpu_notices = Arc::new(Mutex::new(0));
            let seen = gpu_notices.clone();
            app.listen_any("transcribe://gpu-failed", move |_| {
                *seen.lock().unwrap() += 1;
            });
            Fixture {
                app,
                events,
                gpu_notices,
                tmp,
            }
        }

        impl Fixture {
            fn queue(&self) -> tauri::State<'_, Mock> {
                self.app.state::<Mock>()
            }

            fn store(&self) -> tauri::State<'_, Store> {
                self.app.state::<Store>()
            }

            fn recorded(&self) -> String {
                let id = self.store().create("", false).unwrap().id;
                self.store().finish_recording(&id, 0, vec![]).unwrap();
                id
            }

            fn status(&self, id: &str) -> Status {
                self.store().get(id).unwrap().status
            }

            fn wait(&self, what: &str, done: impl Fn() -> bool) {
                let until = Instant::now() + Duration::from_secs(10);
                while !done() {
                    assert!(
                        Instant::now() < until,
                        "timed out: {what}; queue {:?}",
                        self.queue().snapshot()
                    );
                    thread::sleep(Duration::from_millis(10));
                }
            }

            /// A transcribe worker of `id` is alive.
            fn transcribing(&self, id: &str) -> bool {
                let dir = self.store().dir_for(id).unwrap();
                dir.join("chunks.json").exists()
                    && lock(&self.queue().inner)
                        .jobs
                        .get(id)
                        .is_some_and(|j| j.child.is_some())
            }

            /// The `--transcribe-worker` runs in `worker.log`, by device.
            fn runs(&self, id: &str) -> Vec<String> {
                let log = self.store().dir_for(id).unwrap().join("worker.log");
                fs::read_to_string(log)
                    .unwrap()
                    .lines()
                    .filter(|l| l.starts_with("--- --transcribe-worker"))
                    .map(|l| l.split(' ').nth(5).unwrap().to_string())
                    .collect()
            }

            fn transcript(&self, id: &str) -> serde_json::Value {
                let path = self.store().dir_for(id).unwrap().join("transcript.json");
                serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
            }

            /// What "저장" in the settings does: these connections, the first as the default.
            fn connect(&self, connections: Vec<LlmConnection>) {
                let settings = self.app.state::<SettingsStore>();
                let current = settings.load().unwrap().settings;
                let default_llm = connections.first().map(|c| c.name.clone());
                settings
                    .save(Settings {
                        llm_connections: connections,
                        default_llm,
                        ..current
                    })
                    .unwrap();
            }

            fn generated(&self, id: &str) -> Option<String> {
                let dir = self.store().dir_for(id).unwrap();
                fs::read_to_string(dir.join("summary.generated.md")).ok()
            }

            /// The summary model, as a file of its size: the built-in route is ready. Only under
            /// `summary_model`'s lock.
            fn install_summary(&self) {
                let path = self.app.state::<Models>().dir.join(model::SUMMARY.file);
                let file = fs::File::create(&path).unwrap();
                // NTFS allocates a file's whole length unless it is sparse. Best effort: the lock
                // still keeps it to one at a time.
                #[cfg(windows)]
                let _ = Command::new("fsutil")
                    .args(["sparse", "setflag"])
                    .arg(&path)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                file.set_len(model::SUMMARY.size).unwrap();
            }

            /// A meeting that waits with its transcript, as one transcribed before would.
            fn transcribed(&self, summarize_local: bool, pinned: Option<&str>) -> String {
                let id = self.store().create("", summarize_local).unwrap().id;
                self.store().finish_recording(&id, 0, vec![]).unwrap();
                let segments = serde_json::json!({"segments": [
                    {"start_ms": 0, "end_ms": 10000, "text": "이번 주 배포는 목요일 오후에 하고 릴리스 노트는 민수 씨가 금요일까지 씁니다"}
                ]});
                let dir = self.store().dir_for(&id).unwrap();
                fs::write(dir.join("transcript.json"), segments.to_string()).unwrap();
                self.store()
                    .update(&id, |m| {
                        m.status = Status::Transcribed;
                        m.llm_connection = pinned.map(String::from);
                        Ok(())
                    })
                    .unwrap();
                id
            }

            /// Chat requests the fake llama-servers were sent.
            fn asked(&self) -> usize {
                crate::sidecar::tests::asked(&self.tmp.path().join("engine/models"))
            }

            fn engine_log(&self) -> String {
                let log = self.tmp.path().join("engine/logs/llama-server.log");
                fs::read_to_string(log).unwrap_or_default()
            }

            /// No engine runs, nor winds down.
            fn engine_gone(&self) {
                self.wait("engine gone", || !self.queue().engine_running());
            }

            fn gpu_failed(&self) -> bool {
                self.app
                    .state::<SettingsStore>()
                    .load()
                    .unwrap()
                    .settings
                    .gpu_failed
            }
        }

        /// Transcribed, then SUM-10 before any route: with no summary model and the built-in route
        /// the default, a silent meeting still reaches review, with no connection pinned.
        #[cfg(unix)]
        #[test]
        fn a_silent_meeting_runs_through_to_review_without_waiting_for_a_route() {
            let f = fixture(RESULT, Backend::Auto);
            let id = f.recorded();
            f.queue().enqueue_pending();
            // The status is on disk before the last event: wait for the queue to let go.
            f.wait("done", || f.queue().snapshot().is_empty());

            let meta = f.store().get(&id).unwrap();
            assert_eq!(
                (meta.status, meta.no_speech, meta.llm_connection, meta.wait),
                (Status::Review, true, None, None)
            );
            assert_eq!(
                (meta.chunks_done, meta.backend.as_deref()),
                (2, Some("gpu"))
            );
            // The hallucination is filtered; the silent chunk ran no worker.
            assert_eq!(
                f.transcript(&id),
                serde_json::json!({"segments": [{"start_ms": 0, "end_ms": 10000, "text": "구간 0"}]})
            );
            // What the processing screen reads back (`get_transcript`).
            assert_eq!(
                f.store().transcript(&id).unwrap(),
                [Segment {
                    start_ms: 0,
                    end_ms: 10000,
                    text: "구간 0".into()
                }]
            );
            assert_eq!(f.runs(&id), ["gpu"]);
            let events = f.events.lock().unwrap();
            assert!(
                events
                    .iter()
                    .any(|e| e["chunks"] == 0 && e["percent"] == 50)
            );
            let last = events.last().unwrap();
            assert_eq!(
                (&last["status"], &last["percent"]),
                (&"review".into(), &100.into())
            );
        }

        #[cfg(unix)]
        #[test]
        fn recording_pauses_a_run_and_it_resumes_first_from_the_same_chunk() {
            // Hangs the first time any chunk runs, answers after that.
            let f = fixture(
                &format!(
                    r#"if [ ! -e "$2/../slept" ]; then touch "$2/../slept"; echo '{{"type":"progress","pct":10}}'; exec sleep 30; fi; {RESULT}"#
                ),
                Backend::Auto,
            );
            let first = f.recorded();
            f.queue().enqueue_pending();
            let marker = f.tmp.path().join("meetings").join("slept");
            f.wait("first chunk hanging", || {
                marker.exists() && f.transcribing(&first)
            });

            let started = Instant::now();
            f.queue().recording_started();
            f.wait("paused", || f.status(&first) == Status::Paused);
            assert!(started.elapsed() < Duration::from_secs(1));
            // A meeting recorded meanwhile waits behind it; nothing starts while recording.
            let second = f.recorded();
            f.queue().enqueue_pending();
            thread::sleep(Duration::from_millis(200));
            {
                let queue = f.queue();
                let inner = lock(&queue.inner);
                assert!(inner.sched.running.is_empty());
                assert_eq!(
                    (inner.sched.position(&first), inner.sched.position(&second)),
                    (Some(1), Some(2))
                );
            }
            assert_eq!(f.status(&second), Status::Recorded);

            f.queue().recording_finished();
            // Too little speech to summarize (SUM-10): both done once transcribed.
            f.wait("both done", || {
                f.status(&first) == Status::Review && f.status(&second) == Status::Review
            });
            // The stopped chunk ran again from its start.
            assert_eq!(f.runs(&first), ["gpu", "gpu"]);
        }

        #[cfg(unix)]
        #[test]
        fn a_run_ending_after_a_recording_requeued_it_leaves_no_place_in_line() {
            let f = fixture(RESULT, Backend::Auto);
            let id = f.recorded();
            let queue = f.queue();
            queue.add(&id, Status::Recorded);
            {
                // Started (without a thread, so nothing races this test).
                let mut inner = lock(&queue.inner);
                inner.sched.start_next(2, |_| true);
                queue.publish_positions(&mut inner);
            }
            // A recording puts it back in line, but the run finishes before its kill lands.
            queue.recording_started();
            assert_eq!(f.events.lock().unwrap().last().unwrap()["position"], 1);
            queue.end(&f.store(), &id, End::Done(Status::Transcribed));

            let last = f.events.lock().unwrap().last().cloned().unwrap();
            assert_eq!(
                (&last["status"], &last["position"]),
                (&"transcribed".into(), &serde_json::Value::Null)
            );
            assert!(queue.snapshot().is_empty());
        }

        #[cfg(unix)]
        #[test]
        fn a_cpu_crash_fails_the_meeting_with_the_workers_last_words() {
            let f = fixture("echo 'whisper exploded' >&2; exit 1", Backend::Cpu);
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("done", || f.queue().snapshot().is_empty());
            let meta = f.store().get(&id).unwrap();
            assert_eq!(meta.status, Status::TranscribeFailed);
            let error = meta.error.unwrap();
            assert!(error.contains("whisper exploded"), "{error}");
            assert!(!f.gpu_failed());
        }

        #[cfg(unix)]
        #[test]
        fn two_gpu_crashes_mark_gpu_failed_and_the_cpu_finishes() {
            let f = fixture(
                &format!(r#"if [ "$5" = gpu ]; then echo 'gpu fault' >&2; exit 1; fi; {RESULT}"#),
                Backend::Auto,
            );
            let id = f.recorded();
            f.queue().enqueue_pending();
            // Too little speech to summarize (SUM-10): done once transcribed.
            f.wait("done", || f.status(&id) == Status::Review);
            assert_eq!(f.runs(&id), ["gpu", "gpu", "cpu"]);
            assert_eq!(f.store().get(&id).unwrap().backend.as_deref(), Some("cpu"));
            assert!(f.gpu_failed());
            assert_eq!(*f.gpu_notices.lock().unwrap(), 1);
        }

        /// Exit code 3 hashes the model: the fixture's sparse files have the right size but not
        /// the right bytes, so the message says damaged and the file is gone for "다시 받기".
        #[cfg(unix)]
        #[test]
        fn a_model_that_will_not_load_is_hashed_and_a_damaged_one_deleted() {
            let f = fixture("echo 'failed to load model' >&2; exit 3", Backend::Auto);
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("done", || f.queue().snapshot().is_empty());
            let meta = f.store().get(&id).unwrap();
            assert_eq!(meta.status, Status::TranscribeFailed);
            assert_eq!(
                meta.error.as_deref(),
                Some(model_failure(Check::Damaged, "").as_str())
            );
            assert!(f.app.state::<Models>().ready("tiny").is_none());
            assert!(!f.gpu_failed());
        }

        #[cfg(unix)]
        #[test]
        fn delete_waits_for_the_run_before_the_folder_goes() {
            let f = fixture(
                r#"echo '{"type":"progress","pct":10}'; exec sleep 30"#,
                Backend::Auto,
            );
            let id = f.recorded();
            let dir = f.store().dir_for(&id).unwrap();
            f.queue().enqueue_pending();
            f.wait("running", || f.transcribing(&id));

            let started = Instant::now();
            // Twice at once, like a double click: neither may race the winding-down run.
            let app = f.app.handle();
            let deleted: Vec<_> = thread::scope(|s| {
                let delete = || {
                    let cancel = || app.state::<Mock>().cancel(&id);
                    app.state::<Store>().delete_with(&id, cancel)
                };
                let one = s.spawn(delete);
                let two = s.spawn(delete);
                [one.join().unwrap(), two.join().unwrap()].into()
            });
            assert!(deleted.iter().all(Result::is_ok), "{deleted:?}");
            assert!(!dir.exists());
            assert!(started.elapsed() < Duration::from_secs(2));
            let queue = f.queue();
            let inner = lock(&queue.inner);
            assert!(inner.sched.running.is_empty() && inner.jobs.is_empty());
            assert!(f.tmp.path().join("meetings").exists());
        }

        #[cfg(unix)]
        #[test]
        fn a_meeting_is_transcribed_then_summarized_in_the_same_slot() {
            let f = fixture(SPEECH, Backend::Auto);
            // Nobody named 철수 or a 18th (plan 5.6).
            let item = "- [민수] 릴리스 노트 초안 (금요일)";
            let raw = SUMMARY.replace(item, "- [민수, 철수] 릴리스 노트 초안 (18일)");
            let checked = SUMMARY.replace(item, "- [민수, 미정] 릴리스 노트 초안 (미정)");
            let answer = raw.clone();
            let stub = serve(move |_| Some(completion(&answer)));
            f.connect(vec![connection(&stub.url, "내 Ollama", true)]);
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("done", || f.queue().snapshot().is_empty());

            let meta = f.store().get(&id).unwrap();
            assert_eq!(
                (meta.status, meta.title.as_str(), meta.no_speech),
                (Status::Review, "주간 회의", false)
            );
            assert_eq!(
                (meta.llm_connection.as_deref(), meta.summarize_local),
                (Some("내 Ollama"), true)
            );
            // The LLM's text as it came, and as checked for review (SUM-05); the body without its
            // title for the review.
            let dir = f.store().dir_for(&id).unwrap();
            assert_eq!(fs::read_to_string(dir.join("summary.raw.md")).unwrap(), raw);
            assert_eq!(f.generated(&id).unwrap(), checked);
            assert_eq!(f.store().summary(&id).unwrap(), split_title(&checked).1);
            assert_eq!(
                (meta.grounding_replaced, meta.grounding_checked),
                (2, Some(true))
            );
            let requests = stub.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let user = requests[0].body["messages"][1]["content"].as_str().unwrap();
            assert!(user.ends_with(
                "\n이번 주 배포는 목요일 오후에 하고 릴리스 노트는 민수 씨가 금요일까지 씁니다"
            ));

            let events = f.events.lock().unwrap();
            // Straight on from transcribing: never back in line in between.
            assert!(!events.iter().any(|e| e["status"] == "transcribed"));
            assert!(
                events
                    .iter()
                    .any(|e| e["status"] == "summarizing" && e["chunks"] == 1)
            );
            let last = events.last().unwrap();
            assert_eq!(
                (&last["status"], &last["chunk"], &last["percent"]),
                (&"review".into(), &1.into(), &100.into())
            );
        }

        #[cfg(unix)]
        #[test]
        fn without_a_connection_it_waits_transcribed_until_one_is_saved() {
            let f = fixture(SPEECH, Backend::Auto);
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("transcribed", || f.queue().snapshot().is_empty());
            assert_eq!(f.status(&id), Status::Transcribed);
            // A recording ending or the app starting changes nothing while there is no LLM.
            f.queue().enqueue_pending();
            assert!(f.queue().snapshot().is_empty());

            let stub = serve(|_| Some(completion(SUMMARY)));
            f.connect(vec![connection(&stub.url, "회사 클라우드", false)]);
            f.queue().enqueue_pending();
            f.wait("review", || f.status(&id) == Status::Review);
            // Summarized from the transcript, not transcribed again.
            assert_eq!(f.runs(&id), ["gpu"]);
            let requests = stub.requests.lock().unwrap();
            assert!(requests[0].head.contains("authorization: bearer sk-test"));
            assert!(!f.store().get(&id).unwrap().summarize_local);
        }

        /// E3: a transcription file in place only pumps; the summary model, downloaded or imported
        /// (`commands::arrived`), also queues the `transcribed` meetings.
        #[cfg(unix)]
        #[test]
        fn the_summary_model_arriving_queues_the_transcribed_meetings() {
            let f = fixture(SPEECH, Backend::Auto);
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("transcribed", || f.queue().snapshot().is_empty());
            let stub = serve(|_| Some(completion(SUMMARY)));
            // Saved past `save_settings`, which would queue it by itself.
            f.connect(vec![connection(&stub.url, "회사 클라우드", false)]);

            crate::commands::arrived(&f.queue(), &model::VAD);
            assert!(f.queue().snapshot().is_empty());
            assert_eq!(f.status(&id), Status::Transcribed);
            crate::commands::arrived(&f.queue(), &model::SUMMARY);
            f.wait("review", || f.status(&id) == Status::Review);
        }

        /// SUM-10: no LLM request at all.
        #[cfg(unix)]
        #[test]
        fn too_little_speech_goes_to_review_without_asking_the_llm() {
            let f = fixture(RESULT, Backend::Auto);
            let stub = serve(|_| Some(completion(SUMMARY)));
            f.connect(vec![connection(&stub.url, "내 Ollama", true)]);
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("done", || f.queue().snapshot().is_empty());

            let meta = f.store().get(&id).unwrap();
            assert_eq!(
                (meta.status, meta.no_speech, meta.llm_connection),
                (Status::Review, true, None)
            );
            assert!(f.generated(&id).is_none());
            assert_eq!(stub.connections.load(Ordering::SeqCst), 0);
        }

        #[cfg(unix)]
        #[test]
        fn an_llm_error_fails_the_summary_and_a_retry_takes_another_connection() {
            let f = fixture(SPEECH, Backend::Auto);
            let broken = serve(|_| Some(reply("500 Internal Server Error", "model overloaded")));
            let working = serve(|_| Some(completion(SUMMARY)));
            let chosen = connection(&working.url, "정상", true);
            f.connect(vec![connection(&broken.url, "고장", true), chosen.clone()]);
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("failed", || f.queue().snapshot().is_empty());

            let meta = f.store().get(&id).unwrap();
            assert_eq!(meta.status, Status::SummarizeFailed);
            let error = meta.error.unwrap();
            assert!(
                error.contains("HTTP 500") && error.contains("model overloaded"),
                "{error}"
            );
            // SUM-06: the transcript stays.
            assert!(f.store().transcript(&id).is_ok());
            let last = f.events.lock().unwrap().last().cloned().unwrap();
            assert_eq!(last["status"], "summarize_failed");

            let meta = f.store().retry_summary(&id, &chosen).unwrap();
            f.queue().enqueue(&id, meta.status);
            f.wait("review", || f.status(&id) == Status::Review);
            let meta = f.store().get(&id).unwrap();
            assert_eq!(
                (meta.llm_connection.as_deref(), meta.error),
                (Some("정상"), None)
            );
            assert_eq!(f.runs(&id), ["gpu"]);
        }

        /// SUM-09: a local summary stops for a recording and runs again from the transcript after.
        #[cfg(unix)]
        #[test]
        fn a_recording_aborts_a_local_summary_which_runs_again_after_it() {
            let f = fixture(SPEECH, Backend::Auto);
            // The first request is never answered.
            let calls = AtomicUsize::new(0);
            let stub = serve(move |_| {
                (calls.fetch_add(1, Ordering::SeqCst) > 0).then(|| completion(SUMMARY))
            });
            f.connect(vec![connection(&stub.url, "내 Ollama", true)]);
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("requested", || stub.requests.lock().unwrap().len() == 1);

            let started = Instant::now();
            f.queue().recording_started();
            f.wait("paused", || f.status(&id) == Status::Paused);
            assert!(started.elapsed() < Duration::from_secs(1));
            {
                let queue = f.queue();
                let inner = lock(&queue.inner);
                assert_eq!(inner.sched.position(&id), Some(1));
            }

            f.queue().recording_finished();
            f.wait("review", || f.status(&id) == Status::Review);
            assert_eq!(f.runs(&id), ["gpu"]);
            assert_eq!(stub.requests.lock().unwrap().len(), 2);
        }

        /// SUM-09: a cloud summary is not stopped by a recording.
        #[cfg(unix)]
        #[test]
        fn a_cloud_summary_runs_on_through_a_recording() {
            let f = fixture(SPEECH, Backend::Auto);
            let gate = Arc::new(AtomicBool::new(false));
            let open = gate.clone();
            let stub = serve(move |_| {
                while !open.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(10));
                }
                Some(completion(SUMMARY))
            });
            f.connect(vec![connection(&stub.url, "회사 클라우드", false)]);
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("requested", || stub.requests.lock().unwrap().len() == 1);

            f.queue().recording_started();
            thread::sleep(Duration::from_millis(200));
            {
                let queue = f.queue();
                let inner = lock(&queue.inner);
                assert!(inner.sched.running.contains(&id));
                assert_eq!(inner.sched.position(&id), None);
            }
            assert_eq!(f.status(&id), Status::Summarizing);

            gate.store(true, Ordering::SeqCst);
            f.wait("review", || f.status(&id) == Status::Review);
            assert!(lock(&f.queue().inner).sched.recording);
            assert_eq!(stub.requests.lock().unwrap().len(), 1);
            f.queue().recording_finished();
        }

        /// Quit stops even a cloud summary; `pause_busy` leaves it `paused` for the next start.
        #[cfg(unix)]
        #[test]
        fn quit_aborts_a_cloud_summary_and_leaves_it_paused() {
            let f = fixture(SPEECH, Backend::Auto);
            let stub = serve(|_| None);
            f.connect(vec![connection(&stub.url, "회사 클라우드", false)]);
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("requested", || stub.requests.lock().unwrap().len() == 1);

            let started = Instant::now();
            // What `confirm_quit` and the exit handler do.
            f.queue().shutdown();
            f.store().pause_busy();
            f.wait("run ended", || lock(&f.queue().inner).jobs.is_empty());
            assert!(started.elapsed() < Duration::from_secs(2));
            assert_eq!(f.status(&id), Status::Paused);
            assert!(lock(&f.queue().inner).sched.running.is_empty());
        }

        /// The status decides what runs: a `transcribe_failed` meeting is transcribed again even
        /// with a `transcript.json` there (written before its status could be, disk full).
        #[cfg(unix)]
        #[test]
        fn a_transcribe_failed_meeting_with_a_transcript_file_is_transcribed_again() {
            let f = fixture(SPEECH, Backend::Auto);
            let stub = serve(|_| Some(completion(SUMMARY)));
            f.connect(vec![connection(&stub.url, "내 Ollama", true)]);
            let id = f.recorded();
            let dir = f.store().dir_for(&id).unwrap();
            fs::write(dir.join("transcript.json"), r#"{"segments":[]}"#).unwrap();
            f.store()
                .update(&id, |m| {
                    m.status = Status::TranscribeFailed;
                    Ok(())
                })
                .unwrap();
            f.queue().enqueue(&id, Status::TranscribeFailed);
            f.wait("review", || f.status(&id) == Status::Review);
            assert_eq!(f.runs(&id), ["gpu"]);
            assert!(!f.store().get(&id).unwrap().no_speech);
        }

        /// A delete landing after a recording stopped the run, before its end is written: the
        /// meeting does not come back in line.
        #[cfg(unix)]
        #[test]
        fn a_delete_after_a_recording_stop_leaves_no_ghost_in_line() {
            let f = fixture(RESULT, Backend::Auto);
            let id = f.recorded();
            let queue = f.queue();
            queue.add(&id, Status::Recorded);
            {
                // Started without a thread, so the test drives its end.
                let mut inner = lock(&queue.inner);
                inner.sched.start_next(2, |_| true);
                inner.jobs.insert(id.clone(), Job::default());
            }
            queue.recording_started();
            thread::scope(|s| {
                let deleting = s.spawn(|| queue.cancel(&id));
                f.wait("delete landed", || {
                    lock(&queue.inner).jobs.get(&id).and_then(|j| j.stop) == Some(Stop::Delete)
                });
                queue.end(&f.store(), &id, End::Stopped(Stop::Recording));
                deleting.join().unwrap();
            });
            let inner = lock(&queue.inner);
            assert_eq!(inner.sched.position(&id), None);
            assert!(inner.sched.running.is_empty() && inner.jobs.is_empty());
        }

        /// Delete (and quit) stop even a cloud summary.
        #[cfg(unix)]
        #[test]
        fn delete_aborts_a_summary_in_flight() {
            let f = fixture(SPEECH, Backend::Auto);
            let stub = serve(|_| None);
            f.connect(vec![connection(&stub.url, "회사 클라우드", false)]);
            let id = f.recorded();
            let dir = f.store().dir_for(&id).unwrap();
            f.queue().enqueue_pending();
            f.wait("requested", || stub.requests.lock().unwrap().len() == 1);

            let started = Instant::now();
            f.store()
                .delete_with(&id, || f.queue().cancel(&id))
                .unwrap();
            assert!(!dir.exists());
            assert!(started.elapsed() < Duration::from_secs(2));
            let queue = f.queue();
            let inner = lock(&queue.inner);
            assert!(inner.sched.running.is_empty() && inner.jobs.is_empty());
        }

        // The built-in route (plan 5.3-5.5) through the fake llama-server.

        /// Plan 5.4: no route yet (the built-in one without its model) is a wait in `transcribed`
        /// with the reason, not a failure; the model arriving (E3) then summarizes it end to end on
        /// the built-in route, its engine on the device the settings give (stage 2's deferred
        /// check).
        #[cfg(unix)]
        #[test]
        fn the_summary_model_arriving_summarizes_a_waiting_meeting_on_this_pc() {
            let _model = summary_model();
            let f = fixture(SPEECH, Backend::Auto);
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("transcribed", || f.queue().snapshot().is_empty());
            let meta = f.store().get(&id).unwrap();
            assert_eq!(
                (meta.status, meta.wait, meta.error),
                (
                    Status::Transcribed,
                    Some(crate::store::Wait::SummaryModel),
                    None
                )
            );
            assert_eq!(meta.llm_connection, None);
            // Nothing changes while the model is missing.
            f.queue().enqueue_pending();
            assert!(f.queue().snapshot().is_empty());

            // The GPU failed the summary engine before: automatic mode starts it on the CPU.
            let settings = f.app.state::<SettingsStore>();
            settings.mark_summary_gpu_failed().unwrap();
            f.install_summary();
            crate::commands::arrived(&f.queue(), &model::SUMMARY);
            f.wait("review", || f.status(&id) == Status::Review);
            let meta = f.store().get(&id).unwrap();
            assert_eq!(
                (meta.llm_connection.as_deref(), meta.summarize_local),
                (Some(BUILTIN), true)
            );
            assert_eq!((meta.wait, meta.title.as_str()), (None, "주간 회의"));
            assert_eq!(f.generated(&id).unwrap(), SUMMARY);
            assert_eq!(f.asked(), 1);
            assert!(f.engine_log().contains("--device none"));
            // Plan 5.3: no idle engine once it is done.
            f.engine_gone();
        }

        /// ENG2: a meeting with only its summary left starts without the Whisper model, a cloud
        /// one too; one still to transcribe keeps its place meanwhile.
        #[test]
        fn a_summary_only_meeting_starts_without_the_whisper_model() {
            let _model = summary_model();
            let f = fixture(SPEECH, Backend::Auto);
            let stub = serve(|_| Some(completion(SUMMARY)));
            let settings = f.app.state::<SettingsStore>();
            let current = settings.load().unwrap().settings;
            let cloud = connection(&stub.url, "회사 클라우드", false);
            settings
                .save(Settings {
                    llm_connections: vec![cloud],
                    ..current
                })
                .unwrap();
            f.install_summary();
            let whisper = model::needed("tiny").unwrap()[0];
            fs::remove_file(f.app.state::<Models>().dir.join(whisper.file)).unwrap();

            let waiting = f.recorded();
            let on_this_pc = f.transcribed(false, None);
            let to_cloud = f.transcribed(false, Some("회사 클라우드"));
            f.queue().enqueue_pending();
            f.wait("both summarized", || {
                f.status(&on_this_pc) == Status::Review && f.status(&to_cloud) == Status::Review
            });
            assert_eq!(
                f.store()
                    .get(&on_this_pc)
                    .unwrap()
                    .llm_connection
                    .as_deref(),
                Some(BUILTIN)
            );
            assert_eq!(stub.requests.lock().unwrap().len(), 1);
            assert_eq!(f.asked(), 1);
            assert_eq!(f.status(&waiting), Status::Recorded);
            assert_eq!(lock(&f.queue().inner).sched.position(&waiting), Some(1));
        }

        /// ENG1: with the summary model in, two "로컬로 요약" meetings beside a cloud default run one
        /// after the other on this PC: no deadlock, never two at once, nothing to the cloud.
        #[test]
        fn two_local_summaries_with_a_cloud_default_run_one_after_the_other() {
            let _model = summary_model();
            let f = fixture(SPEECH, Backend::Auto);
            let stub = serve(|_| Some(completion(SUMMARY)));
            f.connect(vec![connection(&stub.url, "회사 클라우드", false)]);
            f.install_summary();
            let first = f.transcribed(true, None);
            let second = f.transcribed(true, None);
            f.queue().enqueue_pending();
            let most = AtomicUsize::new(0);
            f.wait("both summarized", || {
                most.fetch_max(lock(&f.queue().inner).sched.running.len(), Ordering::SeqCst);
                f.status(&first) == Status::Review && f.status(&second) == Status::Review
            });
            assert_eq!(most.load(Ordering::SeqCst), 1);
            assert_eq!(f.asked(), 2);
            assert_eq!(stub.connections.load(Ordering::SeqCst), 0);
            for id in [&first, &second] {
                let meta = f.store().get(id).unwrap();
                assert_eq!(meta.llm_connection.as_deref(), Some(BUILTIN));
            }
        }

        /// ENG15: a meeting pinned to a connection since deleted waits and says so, is never sent
        /// elsewhere unasked, and takes the connection picked for it; one waiting for the summary
        /// model can be given a cloud connection instead.
        #[test]
        fn a_waiting_meeting_takes_the_connection_picked_for_it() {
            let f = fixture(SPEECH, Backend::Auto);
            let stub = serve(|_| Some(completion(SUMMARY)));
            let settings = f.app.state::<SettingsStore>();
            let current = settings.load().unwrap().settings;
            let cloud = connection(&stub.url, "새 클라우드", false);
            settings
                .save(Settings {
                    llm_connections: vec![cloud.clone()],
                    ..current
                })
                .unwrap();
            let orphan = f.transcribed(false, Some("지운 연결"));
            let modelless = f.transcribed(false, None);
            f.queue().enqueue_pending();
            f.wait("reasons written", || {
                f.store().get(&orphan).unwrap().wait.is_some()
                    && f.store().get(&modelless).unwrap().wait.is_some()
            });
            assert_eq!(
                f.store().get(&orphan).unwrap().wait,
                Some(crate::store::Wait::ConnectionDeleted)
            );
            assert_eq!(
                f.store().get(&modelless).unwrap().wait,
                Some(crate::store::Wait::SummaryModel)
            );
            assert!(f.queue().snapshot().is_empty());
            assert_eq!(stub.connections.load(Ordering::SeqCst), 0);

            for id in [&orphan, &modelless] {
                let meta = f.store().retry_summary(id, &cloud).unwrap();
                f.queue().enqueue(id, meta.status);
            }
            f.wait("both summarized", || {
                f.status(&orphan) == Status::Review && f.status(&modelless) == Status::Review
            });
            let meta = f.store().get(&orphan).unwrap();
            assert_eq!(
                (meta.llm_connection.as_deref(), meta.wait),
                (Some("새 클라우드"), None)
            );
            assert_eq!(f.asked(), 0);
        }

        /// SUM-09, plan 5.3: a recording stops the built-in engine, the meeting goes back first in
        /// line, no server runs through the recording, and the summary runs again after it.
        #[test]
        fn a_recording_stops_a_builtin_summary_which_runs_again_after_it() {
            let _model = summary_model();
            let f = fixture_with(SPEECH, Backend::Auto, "stalls-once");
            f.install_summary();
            let id = f.transcribed(false, None);
            f.queue().enqueue_pending();
            f.wait("requested", || f.asked() == 1);

            let started = Instant::now();
            f.queue().recording_started();
            f.wait("paused", || f.status(&id) == Status::Paused);
            assert!(started.elapsed() < Duration::from_secs(1));
            assert_eq!(lock(&f.queue().inner).sched.position(&id), Some(1));
            f.engine_gone();
            thread::sleep(Duration::from_millis(300));
            assert!(!f.queue().engine_running());
            assert_eq!(f.asked(), 1);

            f.queue().recording_finished();
            f.wait("review", || f.status(&id) == Status::Review);
            let asked = fs::read_to_string(f.tmp.path().join("engine/models/asked.log"));
            assert_eq!(f.asked(), 2, "{asked:?}");
            let meta = f.store().get(&id).unwrap();
            assert_eq!(meta.llm_connection.as_deref(), Some(BUILTIN));
        }

        /// ENG1 through the processing thread: a meeting that finds the lease taken goes back
        /// first in line unchanged, never starts again while it is held, and runs once it is free.
        #[test]
        fn a_meeting_blocked_by_the_lease_runs_once_it_is_given_back() {
            let _model = summary_model();
            let f = fixture(SPEECH, Backend::Auto);
            f.install_summary();
            // Another summary holds the engine; it has no thread here.
            assert!(lock(&f.queue().inner).sched.take_lease("holder"));
            let id = f.transcribed(false, None);
            f.queue().enqueue_pending();
            f.wait("passed over", || {
                let queue = f.queue();
                let inner = lock(&queue.inner);
                inner.sched.blocked.contains(&id) && inner.sched.running.is_empty()
            });
            let events = || f.events.lock().unwrap().len();
            let before = events();
            thread::sleep(Duration::from_millis(300));
            assert_eq!(events(), before, "started again");
            assert_eq!(f.status(&id), Status::Transcribed);
            assert_eq!(lock(&f.queue().inner).sched.position(&id), Some(1));
            assert_eq!(f.asked(), 0);

            f.queue()
                .end(&f.store(), "holder", End::Skipped("테스트".into()));
            f.wait("review", || f.status(&id) == Status::Review);
            assert_eq!(f.asked(), 1);
            f.engine_gone();
        }

        /// A reason that no longer holds leaves `meta.json` as soon as the line is looked at again,
        /// before the meeting can start (here a recording holds it back).
        #[test]
        fn a_stale_wait_goes_once_the_route_resolves() {
            let _model = summary_model();
            let f = fixture(SPEECH, Backend::Auto);
            let id = f.transcribed(false, None);
            f.queue().enqueue_pending();
            let wait = || f.store().get(&id).unwrap().wait;
            assert_eq!(wait(), Some(crate::store::Wait::SummaryModel));
            f.queue().recording_started();
            f.install_summary();
            f.queue().enqueue_pending();
            assert_eq!((f.status(&id), wait()), (Status::Transcribed, None));
            assert_eq!(lock(&f.queue().inner).sched.position(&id), Some(1));
            f.queue().recording_finished();
            f.wait("review", || f.status(&id) == Status::Review);
        }

        /// "로컬로 요약" turned on for a meeting waiting for the summary model: a saved local
        /// connection takes it (`set_summarize_local` queues the waiting meetings again).
        #[test]
        fn turning_on_local_for_a_waiting_meeting_summarizes_it_on_a_saved_local_one() {
            let f = fixture(SPEECH, Backend::Auto);
            let stub = serve(|_| Some(completion(SUMMARY)));
            let settings = f.app.state::<SettingsStore>();
            let current = settings.load().unwrap().settings;
            let ollama = connection(&stub.url, "내 Ollama", true);
            settings
                .save(Settings {
                    llm_connections: vec![ollama],
                    ..current
                })
                .unwrap();
            let id = f.transcribed(false, None);
            f.queue().enqueue_pending();
            let waiting = f.store().get(&id).unwrap().wait;
            assert_eq!(waiting, Some(crate::store::Wait::SummaryModel));
            // The command's own path.
            let current = settings.load().unwrap().settings;
            let summarize_locally = crate::commands::summarize_locally;
            let route = summarize_locally(&f.store(), &current, &f.queue(), &id, true).unwrap();
            assert_eq!(route.connection.map(|c| c.name), Some("내 Ollama".into()));
            f.wait("review", || f.status(&id) == Status::Review);
            let meta = f.store().get(&id).unwrap();
            assert_eq!(
                (meta.llm_connection.as_deref(), meta.wait),
                (Some("내 Ollama"), None)
            );
            assert_eq!(stub.requests.lock().unwrap().len(), 1);
        }

        /// A failed summary whose route no longer resolves (its model went) stays failed with its
        /// error and no wait; a retry is then offered only a connection that resolves.
        #[test]
        fn a_failed_summary_without_a_route_keeps_its_error_and_no_wait() {
            let f = fixture(SPEECH, Backend::Auto);
            let id = f.transcribed(false, Some(BUILTIN));
            f.store()
                .update(&id, |m| {
                    m.status = Status::SummarizeFailed;
                    m.error = Some("앞의 실패".into());
                    Ok(())
                })
                .unwrap();
            f.queue().enqueue(&id, Status::SummarizeFailed);
            f.wait("run ended", || f.queue().snapshot().is_empty());
            let meta = f.store().get(&id).unwrap();
            assert_eq!(
                (meta.status, meta.error.as_deref(), meta.wait),
                (Status::SummarizeFailed, Some("앞의 실패"), None)
            );
            assert_eq!(f.asked(), 0);
        }
    }
}
