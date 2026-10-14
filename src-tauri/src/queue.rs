//! Processing queue (architecture 5.3, 8.4, TRN-01). Up to two meetings transcribe at once; the
//! rest wait first in, first out. Each running meeting has a thread that starts one worker process
//! per chunk and saves every finished chunk, so a stopped meeting resumes from the chunk after its
//! last finished one (STO-03). A fully transcribed meeting ends in `transcribed`; summarizing
//! comes with M4.
//!
//! The decisions are pure and tested without the app: `Sched` (who runs, who waits, places in
//! line), `ChunkRun` (GPU retry, CPU fallback, TRN-07), `read_partial` (where to resume),
//! `percent` and `eta_secs`.

use crate::settings::{Backend, Settings, SettingsStore};
use crate::store::{SCHEMA_VERSION, Status, Store, can_transition, write_atomic};
use crate::transcribe::engine::{self, Chunk, Device, Line, Segment};
use crate::transcribe::filter;
use crate::transcribe::model::{Check, Models};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime, Wry};

/// TRN-01 (user decision 2026-10-14).
const MAX_RUNNING: usize = 2;
/// A worker that prints nothing for this long is killed and counted as a crash. It prints a
/// progress line at least once per 30 s Whisper window (once per 10 min window when planning),
/// and loading even the 874 MB model takes seconds.
const STALL: Duration = Duration::from_secs(5 * 60);
const PARTIAL: &str = "transcript.partial.jsonl";
const GPU_FAILED: &str = "GPU 처리가 불안정해 CPU로 전환했습니다. 처리 시간이 길어집니다";

/// `meeting://progress`, and one entry of `queue_state`. Sent when a meeting joins the line, its
/// place changes, it starts, a chunk moves, and when it ends (then with its final status).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Progress {
    pub id: String,
    pub status: Status,
    /// Chunks finished, of `chunks`. `chunks` is 0 until the chunk plan exists.
    pub chunk: u32,
    pub chunks: u32,
    /// While `chunks` is 0, the chunk planning's progress; then 0..100 over the whole recording.
    pub percent: u8,
    /// Remaining time, once this run has done enough to tell.
    pub eta_secs: Option<u64>,
    /// 1-based place in line while waiting; `None` while running.
    pub position: Option<u32>,
}

/// `transcribe://gpu-failed`: the GPU crashed twice on one chunk, so automatic mode now uses the
/// CPU (architecture 5.4). M5 turns this into an OS notification.
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

    /// Meetings to start now: the front of the line, two at most, none while recording.
    fn start_next(&mut self) -> Vec<String> {
        let mut started = Vec::new();
        while !self.recording && self.running.len() < MAX_RUNNING {
            // The front may still be winding down from a stop: it waits for its thread to end.
            match self.waiting.front() {
                Some(id) if !self.running.contains(id) => {
                    let id = self.waiting.pop_front().expect("front exists");
                    self.running.push(id.clone());
                    started.push(id);
                }
                _ => break,
            }
        }
        started
    }

    /// TRN-09: a recording started. Running meetings go back to the front of the line, in the
    /// order they started. Returns them, to be stopped.
    fn suspend(&mut self) -> Vec<String> {
        self.recording = true;
        for id in self.running.iter().rev() {
            if !self.waiting.contains(id) {
                self.waiting.push_front(id.clone());
            }
        }
        self.running.clone()
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
    /// TRN-09: running meetings back in line, their workers killed.
    fn suspend(&mut self) {
        for id in self.sched.suspend() {
            if let Some(job) = self.jobs.get_mut(&id) {
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

/// TRN-05 / architecture 5.4: the device a chunk starts on.
fn first_device(settings: &Settings) -> Device {
    match settings.backend {
        Backend::Cpu => Device::Cpu,
        Backend::Gpu => Device::Gpu,
        Backend::Auto if settings.gpu_failed => Device::Cpu,
        Backend::Auto => Device::Gpu,
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
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            // Bytes, not text: a line that is not ours (a native library printing, bad UTF-8)
            // is skipped instead of ending the read.
            while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
                let _ = beat.send(());
                if let Ok(line) = serde_json::from_slice(&line) {
                    on_line(line);
                }
                line.clear();
            }
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
    Done,
    Stopped(Stop),
    Failed(String),
    /// Not in a state to transcribe (finished meanwhile, being deleted).
    Skipped(String),
    /// A model went missing after `pump` checked: back in line until it is there again.
    Requeue,
}

/// Generic over the runtime only so tests can run it on `tauri::test`'s mock runtime.
pub struct Queue<R: Runtime = Wry> {
    app: AppHandle<R>,
    /// This executable, started with `--plan-chunks` / `--transcribe-worker`.
    exe: PathBuf,
    inner: Mutex<Inner>,
}

impl<R: Runtime> Queue<R> {
    pub fn new(app: AppHandle<R>, exe: PathBuf) -> Self {
        Self {
            app,
            exe,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn emit(&self, progress: &Progress) {
        if let Err(e) = self.app.emit("meeting://progress", progress) {
            eprintln!("진행률 이벤트 전송 실패: {e}");
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

    /// TRN-01: every `recorded` and `paused` meeting, oldest first. At startup (after recovery
    /// paused what was transcribing) and when a recording ends.
    /// M4: a meeting paused while summarizing must go to the summarizer instead.
    pub fn enqueue_pending(&self) {
        let mut metas = self.app.state::<Store>().list();
        metas.reverse();
        for meta in metas {
            if meta.schema_version <= SCHEMA_VERSION
                && matches!(meta.status, Status::Recorded | Status::Paused)
            {
                self.add(&meta.id, meta.status);
            }
        }
        self.pump();
    }

    /// `resume_meeting`: a continued interrupted recording, or a `transcribe_failed` retry.
    pub fn enqueue(&self, id: &str, status: Status) {
        self.add(id, status);
        self.pump();
    }

    /// Starts whatever may start. Nothing does while a model is missing: meetings wait until a
    /// download or import finishes (ONB-01), which calls this again.
    pub fn pump(&self) {
        let model = self.settings().model;
        if self.app.state::<Models>().ready(&model).is_none() {
            return;
        }
        let mut inner = lock(&self.inner);
        if inner.closed {
            return;
        }
        for id in inner.sched.start_next() {
            let app = self.app.clone();
            let run_id = id.clone();
            let spawned = thread::Builder::new()
                .name("transcribe".into())
                .spawn(move || run(&app, &run_id));
            match spawned {
                // Detached: `cancel` waits on `running` instead, which `end` clears.
                Ok(_) => {
                    inner.jobs.insert(id, Job::default());
                }
                Err(e) => {
                    eprintln!("전사 스레드를 시작할 수 없습니다 ({id}): {e}");
                    inner.sched.finished(&id);
                    inner.progress.remove(&id);
                }
            }
        }
        self.publish_positions(&mut inner);
    }

    /// TRN-09: kills every worker at once. Their meetings go back to the front of the line.
    pub fn recording_started(&self) {
        let mut inner = lock(&self.inner);
        inner.suspend();
        self.publish_positions(&mut inner);
    }

    /// The recording is `recorded` (or failed to start): queue it and carry on.
    pub fn recording_finished(&self) {
        lock(&self.inner).sched.recording = false;
        self.enqueue_pending();
    }

    /// Delete steps 2–3 (architecture 8.3): out of the line, the worker killed, and returns once
    /// the meeting's run has ended. A second delete of the same meeting waits here too.
    pub fn cancel(&self, id: &str) {
        {
            let mut inner = lock(&self.inner);
            inner.progress.remove(id);
            if inner.sched.remove(id)
                && let Some(job) = inner.jobs.get_mut(id)
            {
                job.halt(Stop::Delete);
            }
            self.publish_positions(&mut inner);
        }
        while lock(&self.inner).sched.running.iter().any(|r| r == id) {
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// App exit (architecture 8.5): nothing new starts and every worker is killed. Child processes
    /// outlive their parent otherwise. `Store::pause_busy` then writes `paused`.
    pub fn shutdown(&self) {
        let mut inner = lock(&self.inner);
        inner.closed = true;
        for job in inner.jobs.values_mut() {
            job.halt(Stop::Quit);
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
        let mut run = ChunkRun::new(first_device(&self.settings()));
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
                eprintln!("비정상 전사 결과 ({id}, {}ms~): {why}", chunk.start_ms);
            }
            match run.after(exit, abnormal.is_some()) {
                Next::Accept => return Ok((segments, run.device)),
                Next::Run(_) => {}
                Next::GpuFailed => {
                    if let Err(e) = self.app.state::<SettingsStore>().mark_gpu_failed() {
                        eprintln!("gpu_failed 저장 실패: {e}");
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
            Ok(_) => End::Done,
            Err(e) => End::Failed(e),
        }
    }

    /// Writes how the run ended, frees its slot and starts the next meeting. Only a recording
    /// stop or a missing model keeps the meeting in line.
    fn end(&self, store: &Store, id: &str, end: End) {
        let requeue = matches!(end, End::Stopped(Stop::Recording) | End::Requeue);
        let status = match end {
            End::Done => Some(Status::Transcribed),
            End::Stopped(Stop::Recording) => {
                // Nothing on disk claims work that is not running (same as recovery, 8.4).
                if let Err(e) = store.set_status(id, Status::Paused) {
                    eprintln!("일시정지 기록 실패 ({id}): {e}");
                }
                Some(Status::Paused)
            }
            End::Stopped(Stop::Delete | Stop::Quit) => None,
            End::Failed(message) => {
                eprintln!("전사 실패 ({id}): {message}");
                let failed = store.update(id, |m| {
                    if can_transition(m.status, Status::TranscribeFailed) {
                        m.status = Status::TranscribeFailed;
                        m.error = Some(message);
                    }
                    Ok(())
                });
                if let Err(e) = failed {
                    eprintln!("실패 상태 기록 실패 ({id}): {e}");
                }
                Some(Status::TranscribeFailed)
            }
            End::Skipped(reason) => {
                eprintln!("전사 건너뜀 ({id}): {reason}");
                store.get(id).ok().map(|m| m.status)
            }
            End::Requeue => None,
        };
        {
            let mut inner = lock(&self.inner);
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
                if status == Status::Transcribed {
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
    }
}

/// A running meeting's thread.
fn run<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let queue = app.state::<Queue<R>>();
    let store = app.state::<Store>();
    // A panic still frees the slot and records the failure. (Release builds abort on panic.)
    let end = panic::catch_unwind(AssertUnwindSafe(|| queue.transcribe(&store, id)))
        .unwrap_or_else(|_| End::Failed("전사 처리 중 내부 오류가 났습니다".into()));
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
        assert_eq!(s.start_next(), ids(&["a", "b"]));
        assert_eq!(s.start_next(), Vec::<String>::new());
        assert_eq!(
            (s.position("a"), s.position("c"), s.position("d")),
            (None, Some(1), Some(2))
        );
        // A running meeting is not queued twice.
        assert!(!s.enqueue("a"));

        s.finished("b");
        assert_eq!(s.start_next(), ids(&["c"]));
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
        s.start_next();
        assert_eq!(s.suspend(), ids(&["a", "b"]));
        assert_eq!(
            (s.position("a"), s.position("b"), s.position("c")),
            (Some(1), Some(2), Some(3))
        );
        assert!(s.start_next().is_empty());

        // Recording over, but a's stopped run has not wound down: nothing jumps ahead of it.
        s.recording = false;
        assert!(s.start_next().is_empty());
        s.finished("b");
        assert!(s.start_next().is_empty());
        s.finished("a");
        assert_eq!(s.start_next(), ids(&["a", "b"]));
        assert_eq!(s.position("c"), Some(1));
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
        let with = |backend, gpu_failed| Settings {
            backend,
            gpu_failed,
            ..Settings::default()
        };
        assert_eq!(first_device(&with(Backend::Auto, false)), Device::Gpu);
        assert_eq!(first_device(&with(Backend::Auto, true)), Device::Cpu);
        assert_eq!(first_device(&with(Backend::Gpu, true)), Device::Gpu);
        assert_eq!(first_device(&with(Backend::Cpu, false)), Device::Cpu);
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
        inner.sched.start_next();
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

    /// The queue end to end on the mock runtime, with a shell script as the worker and the
    /// `tiny` model "installed" as sparse files of the right size.
    #[cfg(unix)]
    mod app {
        use super::*;
        use crate::transcribe::model;
        use std::sync::Arc;
        use tauri::Listener;
        use tauri::test::MockRuntime;

        type Mock = Queue<MockRuntime>;

        /// Two chunks; the second has no speech, so no worker runs for it.
        const PLAN: &str = r#"{"type":"plan","len_ms":20000,"chunks":[{"start_ms":0,"end_ms":10000,"speech_ms":5000},{"start_ms":10000,"end_ms":20000,"speech_ms":0}]}"#;
        /// A result echoing the chunk and device, plus a listed hallucination.
        const RESULT: &str = r#"echo '{"type":"progress","pct":40}'; echo "{\"type\":\"result\",\"device\":\"$5\",\"segments\":[{\"start_ms\":$3,\"end_ms\":$4,\"text\":\"구간 $3\"},{\"start_ms\":$3,\"end_ms\":$4,\"text\":\"시청해주셔서 감사합니다.\"}]}""#;

        struct Fixture {
            app: tauri::App<MockRuntime>,
            events: Arc<Mutex<Vec<serde_json::Value>>>,
            gpu_notices: Arc<Mutex<u32>>,
            tmp: tempfile::TempDir,
        }

        /// `transcribe`: the shell for `--transcribe-worker <dir> <start> <end> <device> ...`.
        fn fixture(transcribe: &str, backend: Backend) -> Fixture {
            use std::os::unix::fs::PermissionsExt;
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
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
            app.manage(Queue::new(app.handle().clone(), exe));

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
                let id = self.store().create("").unwrap().id;
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

            fn gpu_failed(&self) -> bool {
                self.app
                    .state::<SettingsStore>()
                    .load()
                    .unwrap()
                    .settings
                    .gpu_failed
            }
        }

        #[test]
        fn a_meeting_runs_through_to_transcribed() {
            let f = fixture(RESULT, Backend::Auto);
            let id = f.recorded();
            f.queue().enqueue_pending();
            // The status is on disk before the last event: wait for the queue to let go.
            f.wait("done", || f.queue().snapshot().is_empty());

            let meta = f.store().get(&id).unwrap();
            assert_eq!(meta.status, Status::Transcribed);
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
                (&"transcribed".into(), &100.into())
            );
        }

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
            f.wait("both transcribed", || {
                f.status(&first) == Status::Transcribed && f.status(&second) == Status::Transcribed
            });
            // The stopped chunk ran again from its start.
            assert_eq!(f.runs(&first), ["gpu", "gpu"]);
        }

        #[test]
        fn a_run_ending_after_a_recording_requeued_it_leaves_no_place_in_line() {
            let f = fixture(RESULT, Backend::Auto);
            let id = f.recorded();
            let queue = f.queue();
            queue.add(&id, Status::Recorded);
            {
                // Started (without a thread, so nothing races this test).
                let mut inner = lock(&queue.inner);
                inner.sched.start_next();
                queue.publish_positions(&mut inner);
            }
            // A recording puts it back in line, but the run finishes before its kill lands.
            queue.recording_started();
            assert_eq!(f.events.lock().unwrap().last().unwrap()["position"], 1);
            queue.end(&f.store(), &id, End::Done);

            let last = f.events.lock().unwrap().last().cloned().unwrap();
            assert_eq!(
                (&last["status"], &last["position"]),
                (&"transcribed".into(), &serde_json::Value::Null)
            );
            assert!(queue.snapshot().is_empty());
        }

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

        #[test]
        fn two_gpu_crashes_mark_gpu_failed_and_the_cpu_finishes() {
            let f = fixture(
                &format!(r#"if [ "$5" = gpu ]; then echo 'gpu fault' >&2; exit 1; fi; {RESULT}"#),
                Backend::Auto,
            );
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("transcribed", || f.status(&id) == Status::Transcribed);
            assert_eq!(f.runs(&id), ["gpu", "gpu", "cpu"]);
            assert_eq!(f.store().get(&id).unwrap().backend.as_deref(), Some("cpu"));
            assert!(f.gpu_failed());
            assert_eq!(*f.gpu_notices.lock().unwrap(), 1);
        }

        /// Exit code 3 hashes the model: the fixture's sparse files have the right size but not
        /// the right bytes, so the message says damaged and the file is gone for "다시 받기".
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
    }
}
