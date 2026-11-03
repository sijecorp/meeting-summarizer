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
//! `read_partial` (where to resume), `percent`, `eta_secs` and `Pace` (why it is slow, plan 5.1-11).

use crate::grounding;
use crate::settings::{self, BUILTIN, Backend, LlmConnection, SecretKind, Settings, SettingsStore};
use crate::sidecar::{self, Sidecar};
use crate::store::{Meta, SCHEMA_VERSION, Status, Store, can_transition, write_atomic};
use crate::summarize;
use crate::transcribe::engine::{self, Chunk, Device, Line, Segment};
use crate::transcribe::filter;
use crate::transcribe::lang;
use crate::transcribe::model::{self, Check, Models};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::{HashMap, HashSet, VecDeque};
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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
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
pub(crate) const STALL: Duration = Duration::from_secs(5 * 60);
const PARTIAL: &str = "transcript.partial.jsonl";
const GPU_FAILED: &str = "GPU 처리가 불안정해 CPU로 전환했습니다. 처리 시간이 길어집니다";
/// Plan 5.6-4: `set_meeting_language` after a chunk is done; the screen then offers E3.
pub const TRANSCRIBED: &str = "이미 전사한 구간이 있어 다시 전사해야 바꿀 수 있습니다";

/// `meeting://progress`, and one entry of `queue_state`. Sent when a meeting joins the line, its
/// place changes, it starts, a chunk or LLM request moves, its slow reason changes, and when it
/// ends (then with its final status).
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
    /// Unix seconds since the built-in engine began starting for this summary, until it is ready
    /// or fails (plan 5.3, up to 120 s): the screens count "요약 엔진 준비 중 · N초" from it.
    pub engine_since: Option<u64>,
    /// Plan 5.1-11: why it is slow, judged again every `TICK` and at every progress line. Only
    /// `saver` while `summarizing`.
    pub slow: Option<SlowReason>,
    /// Plan 5.6-27: a chunk's worker is telling the languages of a mixed meeting apart ("언어를
    /// 가리는 중").
    pub detecting: bool,
}

/// Plan 5.1-11: the one reason a running meeting is slow.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SlowReason {
    /// On battery in battery saver or the best-efficiency power mode (Windows).
    Saver,
    /// Automatic or GPU, yet the CPU transcribes.
    Cpu,
    /// As `Cpu`, but the worker found no GPU at all.
    CpuNogpu,
    /// Slower than the recording all the same.
    Slow,
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
/// crash. Plan 5.7-4: ordered, and `Job::halt` keeps the larger, so a delete always wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stop {
    /// TRN-09: back in line; the chunk runs again from its start once the recording ends.
    Recording,
    /// App exit: `Store::pause_busy` writes `paused`.
    Quit,
    /// Plan 5.3 "일시정지": `end` writes `paused`, and `held` keeps it out of line.
    User,
    Delete,
}

/// Plan 5.3-4: what `pause_meeting` did.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Pause {
    /// It was in line: `paused` now.
    Paused,
    /// It runs: stopped, and `end` writes `paused`.
    Pausing,
    /// It finished meanwhile: nothing was written.
    AlreadyDone,
}

/// Where a run that ended goes back in line.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Place {
    First,
    Last,
}

/// Plan 5.3-5: where a run that ended goes in line, from how it ended, its job's stop (the
/// largest it was given) and the meeting's `held` read as it ends.
fn back_in_line(end: &End, stop: Option<Stop>, held: bool) -> Option<Place> {
    let stop = match end {
        End::Stopped(s) => stop.max(Some(*s)),
        _ => stop,
    };
    match (end, stop) {
        (End::Done(_) | End::Failed(_), _) => None,
        // The recorder paused it: it waits, `paused`, for "이어서 처리".
        _ if held => None,
        // A delete or quit that landed after the run stopped wins: no ghost back in line.
        (_, Some(Stop::Delete | Stop::Quit)) => None,
        // Paused, then taken back while it was stopping.
        (End::Stopped(_), Some(Stop::User)) => Some(Place::Last),
        (End::Stopped(_) | End::Requeue, _) => Some(Place::First),
        (End::Skipped(_), _) => None,
    }
}

#[derive(Default)]
struct Job {
    stop: Option<Stop>,
    child: Option<Child>,
    /// Aborts the summary task in flight.
    abort: Option<Box<dyn Fn() + Send>>,
    /// A cloud summary is in flight: a recording lets it run on (SUM-09).
    cloud: bool,
    /// The last worker's `Line::Device`: where it runs, and the GPU it picked ("" for none).
    device: Option<(Device, String)>,
    /// Plan 5.1-11: the transcription's pace, once its chunks start.
    pace: Option<Pace>,
}

impl Job {
    /// The worker's `Line::Device`. Plan 5.1-11: from the GPU to the CPU, the pace is measured
    /// again.
    fn ran_on(&mut self, device: Device, name: String) {
        if device == Device::Cpu
            && matches!(self.device, Some((Device::Gpu, _)))
            && let Some(pace) = &mut self.pace
        {
            pace.restart();
        }
        self.device = Some((device, name));
    }

    fn halt(&mut self, why: Stop) {
        // A delete wins: its run must not write `paused` over `deleting` afterwards.
        self.stop = self.stop.max(Some(why));
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
    /// Plan 5.6-14: meetings lined up with two or more languages, so `pump` reads no `meta.json`
    /// under this lock to tell. Kept up by `line_up` and `set_language`.
    mixed: HashSet<String>,
    /// The app is exiting: nothing starts any more.
    closed: bool,
}

/// Poisoning is ignored: a panicked run must not keep a recording from stopping the others.
fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    /// TRN-09, SUM-09: running meetings back in line, their workers killed and local summaries
    /// aborted. A cloud summary runs on through the recording, and a paused one goes on stopping
    /// out of line, so it never holds up the line (plan 5.3-6).
    fn suspend(&mut self) {
        let Inner { sched, jobs, .. } = self;
        let keep = |id: &str| {
            jobs.get(id)
                .is_some_and(|j| j.cloud || j.stop == Some(Stop::User))
        };
        for id in sched.suspend(keep) {
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

/// One language, or a list that is not valid and so runs as Korean (`transcribe`): no detection
/// model needed (plan 5.6-14).
fn single(languages: &[String]) -> bool {
    lang::parse(languages).map_or(true, |l| l.len() < 2)
}

/// Only the summary is left (architecture 8.4): from its status, and for `paused` from
/// `transcript.json`. Not from the file alone: a `transcribe_failed` meeting whose
/// `transcript.json` was written before its status could be transcribes again. Nor a `paused` one
/// with no chunk done: E3 starts over from 0, and one it could not clear is a waiting `recorded`
/// meeting paused (plan 5.6-6).
fn summary_only(store: &Store, meta: &Meta) -> bool {
    match meta.status {
        Status::Transcribed | Status::Summarizing | Status::SummarizeFailed => true,
        Status::Paused => {
            meta.chunks_done > 0
                && store
                    .dir_for(&meta.id)
                    .is_ok_and(|dir| dir.join("transcript.json").exists())
        }
        _ => false,
    }
}

#[derive(Debug, PartialEq)]
enum Next {
    Accept,
    Run(Device),
    /// A GPU crash with flash attention on: turn it off for good and run the chunk on the GPU
    /// again without it. Not counted toward `gpu_failed` (plan 5.1-6).
    FlashAttnOff,
    /// The second GPU crash on this chunk: record `gpu_failed`, tell the recorder, then run the
    /// chunk on the CPU.
    GpuFailed,
    Fail,
}

/// One chunk's attempts (TRN-06, TRN-07). Crash counts live only here, in memory.
struct ChunkRun {
    device: Device,
    /// The worker's `--fa`.
    flash_attn: bool,
    gpu_crashes: u32,
    rechecked: bool,
}

impl ChunkRun {
    fn new(device: Device, flash_attn: bool) -> Self {
        Self {
            device,
            flash_attn,
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
            // Some integrated GPUs may crash only in Vulkan's flash attention: that is tried
            // before the GPU is given up on.
            Exit::Crash | Exit::Stalled if self.device == Device::Gpu && self.flash_attn => {
                self.flash_attn = false;
                Next::FlashAttnOff
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
    /// Plan 5.6-7: the languages it was transcribed in. A line from before 1.1.0 was Korean.
    #[serde(default = "lang::default_list")]
    languages: Vec<String>,
}

/// `transcript.json`: every kept segment of the meeting, in order.
#[derive(Serialize)]
struct Transcript<'a> {
    segments: Vec<&'a Segment>,
}

/// STO-03: the finished chunks, in order. Reading stops at the first line that is not the next
/// chunk of `plan` in `languages` (plan 5.6-7: never another language's leftover); that chunk
/// runs again.
fn read_partial(path: &Path, plan: &[Chunk], languages: &[String]) -> Vec<ChunkResult> {
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
            || result.languages != languages
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

/// Plan 5.1-11: running meetings' slow reasons are judged again this often without a progress line.
const TICK: Duration = Duration::from_secs(30);
/// Plan 5.1-11: a measurement keeps its last verdict this long after it starts.
const HOLD: Duration = Duration::from_secs(60);

/// Plan 5.1-11: this run's pace, for its real-time factor. Measured again from the first progress
/// line after the GPU gave way to the CPU.
struct Pace {
    /// When measuring began and the audio done then (ms of the recording). `None` from a restart
    /// until the next progress line.
    from: Option<(Instant, u64)>,
    /// The last progress line: when, and the audio done by it. Up to it, not up to now, so the
    /// factor does not creep up between lines.
    last: (Instant, u64),
    /// The last verdict, kept through a measurement's first `HOLD`.
    held: Option<SlowReason>,
}

impl Pace {
    fn new(now: Instant, done_ms: u64) -> Self {
        Self {
            from: Some((now, done_ms)),
            last: (now, done_ms),
            held: None,
        }
    }

    fn progress(&mut self, now: Instant, done_ms: u64) {
        self.from.get_or_insert((now, done_ms));
        self.last = (now, done_ms);
    }

    fn restart(&mut self) {
        self.from = None;
    }

    /// The verdict at `now`: the last one within `HOLD` of measuring's start or before any audio
    /// is done, then by the real-time factor and where the worker runs.
    fn judge(
        &mut self,
        now: Instant,
        backend: Backend,
        device: Option<&(Device, String)>,
    ) -> Option<SlowReason> {
        let Some((since, from_ms)) = self.from else {
            return self.held;
        };
        let (at, done_ms) = self.last;
        let audio_s = done_ms.saturating_sub(from_ms) as f64 / 1000.0;
        if now.duration_since(since) < HOLD || audio_s == 0.0 {
            return self.held;
        }
        let rtf = at.duration_since(since).as_secs_f64() / audio_s;
        self.held = (rtf > 1.0).then_some(match (backend, device) {
            (Backend::Cpu, _) | (_, None | Some((Device::Gpu, _))) => SlowReason::Slow,
            (_, Some((Device::Cpu, gpu))) if gpu.is_empty() => SlowReason::CpuNogpu,
            _ => SlowReason::Cpu,
        });
        self.held
    }
}

/// Plan 5.1-11: a running meeting's one reason at `now`, `saver` first. `pace` only while it
/// transcribes: a summary is only ever `saver`.
fn slow_reason(
    saver: bool,
    pace: Option<&mut Pace>,
    backend: Backend,
    device: Option<&(Device, String)>,
    now: Instant,
) -> Option<SlowReason> {
    let paced = pace.and_then(|p| p.judge(now, backend, device));
    if saver {
        Some(SlowReason::Saver)
    } else {
        paced
    }
}

/// Plan 5.1-11: Windows' effective power mode as its last notification said; -1 before the first.
#[cfg(windows)]
static POWER_MODE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// Plan 5.1-11: Windows tells this process its effective power mode now and at every change
/// (Windows 10 1809 on, hence V1). For the process's life: never unregistered.
#[cfg(windows)]
fn watch_power_mode() {
    use windows_sys::Win32::System::Power::{
        EFFECTIVE_POWER_MODE, EFFECTIVE_POWER_MODE_V1,
        PowerRegisterForEffectivePowerModeNotifications,
    };
    unsafe extern "system" fn changed(mode: EFFECTIVE_POWER_MODE, _: *const std::ffi::c_void) {
        POWER_MODE.store(mode, Ordering::Relaxed);
    }
    let mut registration = std::ptr::null_mut();
    // SAFETY: the callback only stores into a static; the registration handle is never used.
    let hr = unsafe {
        PowerRegisterForEffectivePowerModeNotifications(
            EFFECTIVE_POWER_MODE_V1,
            Some(changed),
            std::ptr::null(),
            &mut registration,
        )
    };
    if hr < 0 {
        log::warn!("전원 모드 알림을 받을 수 없습니다: {hr:#010x}");
    }
}

/// Plan 5.1-11 `saver`: on battery, in battery saver or the best-efficiency power mode.
#[cfg(windows)]
fn saver() -> bool {
    use windows_sys::Win32::System::Power::{
        EffectivePowerModeBatterySaver, EffectivePowerModeBetterBattery, GetSystemPowerStatus,
        SYSTEM_POWER_STATUS,
    };
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: fills `status` and nothing else.
    let battery = unsafe { GetSystemPowerStatus(&mut status) } != 0 && status.ACLineStatus == 0;
    battery
        && [
            EffectivePowerModeBatterySaver,
            EffectivePowerModeBetterBattery,
        ]
        .contains(&POWER_MODE.load(Ordering::Relaxed))
}

/// macOS never says `saver`.
#[cfg(not(windows))]
fn saver() -> bool {
    false
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
pub(crate) fn last_log_line(log: &Path) -> String {
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
/// Plan 5.1-9: holds the system awake until dropped, `None` when it could not.
type Awake = Box<dyn Fn() -> Option<Box<dyn Any>> + Send + Sync>;

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
    /// Wakes the power thread (`watch`): a run started or ended, a progress line came, the app
    /// exits.
    wake: mpsc::Sender<()>,
    /// Taken by the power thread; tests put a stand-in here.
    awake: Awake,
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
        let (wake, woken) = mpsc::channel();
        let watching = app.clone();
        let spawned = thread::Builder::new()
            .name("power".into())
            .spawn(move || watch(&watching, &woken));
        if let Err(e) = spawned {
            log::error!("전원 감시 스레드를 시작할 수 없습니다: {e}");
        }
        Self {
            app,
            exe,
            secret: Box::new(|name| settings::get_secret(SecretKind::Llm, name)),
            inner: Mutex::new(Inner::default()),
            sidecar: Arc::new(sidecar),
            wake,
            awake: Box::new(|| {
                keepawake::Builder::default()
                    .idle(true)
                    .reason("회의 처리 중")
                    .app_name("회의 요약")
                    .app_reverse_domain("com.sije.meeting-summarizer")
                    .create()
                    .inspect_err(|e| log::warn!("절전 방지 실패: {e}"))
                    .ok()
                    .map(|held| Box::new(held) as Box<dyn Any>)
            }),
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

    /// Plan 5.7-3: `held` is read again under the meeting's lock as it joins the line, never
    /// trusted from a list read before.
    fn add(&self, id: &str, status: Status) {
        let store = self.app.state::<Store>();
        let added = store.with_lock(id, |m| {
            if !m.held {
                self.line_up(&mut lock(&self.inner), id, status, &m.languages);
            }
        });
        if let Err(e) = added {
            log::warn!("대기열에 넣지 못했습니다 ({id}): {e}");
        }
    }

    fn line_up(&self, inner: &mut Inner, id: &str, status: Status, languages: &[String]) {
        if single(languages) {
            inner.mixed.remove(id);
        } else {
            inner.mixed.insert(id.to_string());
        }
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
                engine_since: None,
                slow: None,
                detecting: false,
            },
        );
        self.publish_positions(inner);
    }

    /// Plan 5.1-11: every running meeting's slow reason again, sent where it changed. Whether any
    /// meeting is processing, or `None` once the app is exiting.
    fn judge(&self) -> Option<bool> {
        // The OS and the settings file first: never under the lock.
        let (saver, backend, now) = (saver(), self.settings().backend, Instant::now());
        let mut inner = lock(&self.inner);
        if inner.closed {
            return None;
        }
        let Inner { jobs, progress, .. } = &mut *inner;
        for (id, job) in jobs.iter_mut() {
            let Some(p) = progress.get_mut(id) else {
                continue;
            };
            let pace = job
                .pace
                .as_mut()
                .filter(|_| p.status == Status::Transcribing);
            let slow = slow_reason(saver, pace, backend, job.device.as_ref(), now);
            if p.slow != slow {
                p.slow = slow;
                self.emit(p);
            }
        }
        Some(!jobs.is_empty())
    }

    /// TRN-01: every `recorded` and `paused` meeting, oldest first, but one with only its summary
    /// left (`transcribed`, `paused` after transcribing) once its route resolves (plan 5.4); until
    /// then it says why it waits. Never one the recorder paused (plan 5.3). At startup (after
    /// recovery paused what was running), when a recording ends, when settings are saved and when
    /// the summary model arrives.
    pub fn enqueue_pending(&self) {
        self.enqueue_listed(self.app.state::<Store>().list());
    }

    /// `enqueue_pending` from `metas`, newest first as `Store::list` reads them.
    fn enqueue_listed(&self, mut metas: Vec<Meta>) {
        let (settings, ready) = (self.settings(), self.builtin_ready());
        let store = self.app.state::<Store>();
        metas.reverse();
        for meta in metas {
            if meta.schema_version > SCHEMA_VERSION || meta.held {
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

    /// Plan 5.3-4, 5.7-1: "일시정지", decided and written at once under the meeting's lock, then
    /// the queue's. A running meeting is stopped, even a cloud summary, and its `end` writes
    /// `paused`; one in line leaves it, `paused` now. Either way `held` keeps it out of line until
    /// "이어서 처리". Nothing here calls `store.update` while holding `inner`.
    pub fn pause(&self, store: &Store, id: &str) -> Result<Pause, String> {
        let (mut paused, mut done) = (Pause::AlreadyDone, false);
        let written = store.update(id, |m| {
            let mut inner = lock(&self.inner);
            if inner.sched.running.iter().any(|r| r == id) {
                // Out of line too, where a recording starting put it back first.
                inner.sched.waiting.retain(|w| w != id);
                if let Some(job) = inner.jobs.get_mut(id) {
                    job.halt(Stop::User);
                }
                if inner.sched.lease.as_deref() == Some(id) {
                    self.sidecar.stop("일시정지");
                }
                paused = Pause::Pausing;
            } else if inner.sched.waiting.iter().any(|w| w == id) {
                // One `paused` already (a recording stopped it) keeps its status.
                if m.status != Status::Paused {
                    if !can_transition(m.status, Status::Paused) {
                        return Err(format!("일시정지할 수 없는 상태입니다: {:?}", m.status));
                    }
                    m.status = Status::Paused;
                }
                inner.sched.waiting.retain(|w| w != id);
                if let Some(mut p) = inner.progress.remove(id) {
                    (p.status, p.position) = (Status::Paused, None);
                    self.emit(&p);
                }
                paused = Pause::Paused;
            } else {
                // Finished meanwhile: nothing to write.
                done = true;
                return Err(String::new());
            }
            m.held = true;
            self.publish_positions(&mut inner);
            Ok(())
        });
        if done {
            return Ok(Pause::AlreadyDone);
        }
        written?;
        crate::refresh_tray(&self.app);
        Ok(paused)
    }

    /// Plan 5.3-7: "이어서 처리", in the same shape as `pause`. `held` goes; a held meeting is taken
    /// whatever its status, a run still stopping is put back in line by its `end`. Otherwise as
    /// before: a continued interrupted recording, a `paused` one, a `transcribe_failed` retry.
    pub fn resume(&self, store: &Store, id: &str) -> Result<Meta, String> {
        let meta = store.update(id, |m| {
            let mut inner = lock(&self.inner);
            let again = matches!(
                m.status,
                Status::Recorded | Status::Paused | Status::TranscribeFailed
            );
            if !m.held && !again {
                return Err("다시 처리할 수 있는 회의가 아닙니다".into());
            }
            m.held = false;
            if !inner.sched.running.iter().any(|r| r == id) {
                self.line_up(&mut inner, id, m.status, &m.languages);
            }
            Ok(())
        })?;
        self.pump();
        crate::refresh_tray(&self.app);
        Ok(meta)
    }

    /// Plan 5.6-4, 5.7-1: another language list for a meeting not transcribed yet, in the shape
    /// of `pause`. Refused while it runs (chunk 0 too), and once a chunk or the transcript exists:
    /// that is E3's (`TRANSCRIBED`). One in line keeps its place: each worker reads `meta.languages`.
    pub fn set_language(
        &self,
        store: &Store,
        id: &str,
        languages: &[String],
    ) -> Result<Meta, String> {
        let languages = lang::parse(languages)?;
        let dir = store.dir_for(id)?;
        store.update(id, |m| {
            let mut inner = lock(&self.inner);
            if inner.sched.running.iter().any(|r| r == id) {
                return Err("처리 중에는 바꿀 수 없습니다".into());
            }
            if m.chunks_done > 0 || dir.join("transcript.json").exists() {
                return Err(TRANSCRIBED.into());
            }
            if languages.len() > 1 {
                inner.mixed.insert(id.to_string());
            } else {
                inner.mixed.remove(id);
            }
            m.languages = languages;
            Ok(())
        })
    }

    /// Plan 5.6-6 (E3), in one shape of 5.7-1 and in this order: refused while it runs or
    /// `sending`, else out of line; `meta.json` written `recorded` with the new list and its
    /// results' fields cleared; the old results deleted; back in line. A file that will not go
    /// leaves it out of line with the error: the next transcription deletes them first (5.6-7),
    /// and so does the next start (5.7-10).
    pub fn retranscribe(
        &self,
        store: &Store,
        id: &str,
        languages: &[String],
        sending: impl FnOnce() -> bool,
    ) -> Result<Meta, String> {
        let languages = lang::parse(languages)?;
        let meta = store.update_then(
            id,
            |m| {
                let mut inner = lock(&self.inner);
                if inner.sched.running.iter().any(|r| r == id) {
                    return Err("처리 중입니다. 먼저 일시정지해 주세요".into());
                }
                if sending() {
                    return Err("보내는 중입니다".into());
                }
                if !can_transition(m.status, Status::Recorded) {
                    return Err(format!("다시 전사할 수 없는 상태입니다: {:?}", m.status));
                }
                inner.sched.waiting.retain(|w| w != id);
                inner.progress.remove(id);
                self.publish_positions(&mut inner);
                (m.status, m.languages, m.chunks_done) = (Status::Recorded, languages, 0);
                (m.backend, m.error, m.wait) = (None, None, None);
                (m.grounding_replaced, m.grounding_checked) = (0, None);
                // "로컬로 요약" may be picked again: the summary goes wherever it is chosen now.
                m.llm_connection = None;
                (m.no_speech, m.held) = (false, false);
                Ok(())
            },
            |m| {
                store.clear_results(id)?;
                self.line_up(&mut lock(&self.inner), id, m.status, &m.languages);
                Ok(m.clone())
            },
        )?;
        self.pump();
        crate::refresh_tray(&self.app);
        meta
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
        let models = self.app.state::<Models>();
        let transcribable = models.ready(&settings.model).is_some();
        // Plan 5.6-14: a meeting of two or more languages also waits for the detection model.
        let lid = models.installed(model::lid());
        let slots = slots(self.builtin_ready());
        let mut inner = lock(&self.inner);
        if inner.closed {
            return;
        }
        let Inner { sched, mixed, .. } = &mut *inner;
        let started = sched.start_next(slots, |id| {
            transcribable && (lid || !mixed.contains(id))
                || store.get(id).is_ok_and(|m| summary_only(&store, &m))
        });
        for id in started {
            let app = self.app.clone();
            let run_id = id.clone();
            let spawned = thread::Builder::new()
                .name("process".into())
                .spawn(move || run(&app, &run_id));
            match spawned {
                // Detached: `cancel` waits on `running` instead, which `end` clears.
                Ok(_) => {
                    inner.jobs.insert(id, Job::default());
                    let _ = self.wake.send(());
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
        // The power thread ends and lets the system sleep again.
        let _ = self.wake.send(());
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
    /// `lid`: the worker had the language detection model too.
    fn failure(&self, exit: Exit, dir: &Path, lid: bool) -> String {
        let why = last_log_line(&dir.join("worker.log"));
        match exit {
            Exit::Input => format!("녹음 파일을 읽을 수 없습니다: {why}"),
            Exit::Model => {
                let models = self.app.state::<Models>();
                let mut check = models.verify(&self.settings().model);
                if lid && check == Check::Intact {
                    check = models.check(&[model::lid()]);
                }
                model_failure(check, &why)
            }
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
            .and_then(|b| serde_json::from_slice::<Plan>(&b).ok())
        {
            // A plan from 1.0.3 has no `chunks_total`: a pause before the next chunk ends would
            // read as one made before the plan (plan 5.3-12).
            let chunks = plan.chunks.len() as u32;
            if store.get(id).is_ok_and(|m| m.chunks_total == 0) {
                store
                    .update(id, |m| {
                        m.chunks_total = chunks;
                        Ok(())
                    })
                    .map_err(End::Failed)?;
            }
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
            _ => {}
        })?;
        let plan = match (ended.code, plan) {
            (Some(0), Some(plan)) if !ended.stalled => plan,
            (_, _) => return Err(End::Failed(self.failure(exit_of(ended, None), dir, false))),
        };
        let bytes = serde_json::to_vec(&plan).map_err(|e| End::Failed(e.to_string()))?;
        store
            .update(id, |m| {
                m.chunks_total = plan.chunks.len() as u32;
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
        (model, vad, lid): (&Path, &Path, Option<&Path>),
        languages: &[String],
        mut on_pct: impl FnMut(u8),
    ) -> Result<(Vec<Segment>, Device), End> {
        let settings = self.settings();
        let mut run = ChunkRun::new(
            first_device(settings.backend, settings.gpu_failed),
            settings.transcribe_flash_attn,
        );
        let threads = engine::threads().to_string();
        let version = self.app.package_info().version.to_string();
        let langs = languages.join(",");
        loop {
            let (start, end) = (chunk.start_ms.to_string(), chunk.end_ms.to_string());
            let fa = u8::from(run.flash_attn).to_string();
            let mut args = vec![
                OsStr::new("--transcribe-worker"),
                dir.as_os_str(),
                OsStr::new(&start),
                OsStr::new(&end),
                OsStr::new(run.device.name()),
                model.as_os_str(),
                vad.as_os_str(),
                // Plan 5.1-1: the product's values. The GPU is the worker's pick.
                OsStr::new("--lang"),
                OsStr::new(&langs),
                OsStr::new("--beam"),
                OsStr::new("5"),
                OsStr::new("--threads"),
                OsStr::new(&threads),
                OsStr::new("--fa"),
                OsStr::new(&fa),
            ];
            // Plan 5.6-9: only a meeting of two or more languages is given one.
            if let Some(lid) = lid {
                args.extend([OsStr::new("--lid-model"), lid.as_os_str()]);
            }
            let command = self.worker(dir, &args)?;
            let mut result = None;
            let ended = run_worker(&self.inner, id, command, STALL, |line| match line {
                Line::Progress { pct } => on_pct(pct),
                Line::Detecting { on } => self.set_progress(id, |p| p.detecting = on),
                Line::Result { device, segments } => result = Some((device, segments)),
                Line::Device { device, name } => {
                    if let Some(job) = lock(&self.inner).jobs.get_mut(id) {
                        job.ran_on(device, name);
                    }
                }
                // Plan 5.1-13: the measurements go to app.log, with whose and which version.
                Line::Diag { line } => log::info!("{line} id={id} app={version}"),
                Line::Plan { .. } => {}
            })?;
            let exit = exit_of(ended, result.as_ref().map(|(device, _)| *device));
            let segments = result
                .map(|(_, s)| filter::filter(s, languages))
                .unwrap_or_default();
            let abnormal = matches!(exit, Exit::Done(_))
                .then(|| filter::abnormal(&segments, chunk.speech_ms))
                .flatten();
            if let Some(why) = abnormal {
                log::warn!("비정상 전사 결과 ({id}, {}ms~): {why}", chunk.start_ms);
            }
            match run.after(exit, abnormal.is_some()) {
                Next::Accept => return Ok((segments, run.device)),
                Next::Run(_) => {}
                Next::FlashAttnOff => {
                    log::warn!(
                        "GPU 워커가 비정상 종료해 flash attention을 껐습니다 ({id}, {}ms~)",
                        chunk.start_ms
                    );
                    if let Err(e) = self.app.state::<SettingsStore>().turn_off_flash_attn() {
                        log::error!("transcribe_flash_attn 저장 실패: {e}");
                    }
                }
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
                Next::Fail => return Err(End::Failed(self.failure(exit, dir, lid.is_some()))),
            }
        }
    }

    fn transcribe(&self, store: &Store, id: &str) -> End {
        let settings = self.settings();
        let models = self.app.state::<Models>();
        let Some((model, vad)) = models.ready(&settings.model) else {
            return End::Requeue;
        };
        let lid = models
            .installed(model::lid())
            .then(|| models.dir.join(model::lid().file));
        let (mut languages, mut waits, mut cleared) = (lang::default_list(), false, Ok(()));
        let started = store.update(id, |m| {
            // `transcribing` itself too: the queue runs a meeting once at a time, so that is a
            // stop whose `paused` could not be written. Recovery would pause it at the next start.
            if m.status != Status::Transcribing && !can_transition(m.status, Status::Transcribing) {
                return Err(format!("전사할 수 없는 상태입니다: {:?}", m.status));
            }
            // Plan 5.7-7: checked again before a worker gets it. A bad list becomes Korean for
            // good, so the meeting does not fail over and over.
            languages = lang::parse(&m.languages).unwrap_or_else(|why| {
                let line = engine::diag_line(
                    "languages_reset",
                    &[
                        ("id", id.into()),
                        ("was", m.languages.join(",")),
                        ("why", why),
                    ],
                );
                log::warn!("{line}");
                lang::default_list()
            });
            m.languages = languages.clone();
            // Plan 5.6-14: as for a missing model, it waits, never fails.
            if languages.len() > 1 && lid.is_none() {
                waits = true;
                return Ok(());
            }
            // Plan 5.6-7: nothing a run in another language left is resumed or shown. With no
            // chunk done either: a clear that failed left it `transcribe_failed` at 0.
            if m.status == Status::Recorded || m.chunks_done == 0 {
                cleared = store.clear_results(id);
            }
            m.status = Status::Transcribing;
            m.error = None;
            Ok(())
        });
        if let Err(e) = started {
            return End::Skipped(e);
        }
        if waits {
            return End::Requeue;
        }
        // Failed with its cause, so the meeting offers "다시 시도" and that clears again.
        if let Err(e) = cleared {
            return End::Failed(e);
        }
        let lid = lid.filter(|_| languages.len() > 1);
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
        let mut done = read_partial(&partial, &plan.chunks, &languages);
        let chunks = plan.chunks.len() as u32;
        let done_ms =
            |done: &[ChunkResult]| -> u64 { done.iter().map(|r| r.end_ms - r.start_ms).sum() };
        let resumed_ms = done_ms(&done);
        let since = Instant::now();
        if let Some(job) = lock(&self.inner).jobs.get_mut(id) {
            job.pace = Some(Pace::new(since, resumed_ms));
        }
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
                if let Some(pace) = lock(&self.inner)
                    .jobs
                    .get_mut(id)
                    .and_then(|j| j.pace.as_mut())
                {
                    pace.progress(Instant::now(), at_ms);
                }
                self.set_progress(id, |p| {
                    p.percent = percent(before_ms, chunk_ms, pct, plan.len_ms);
                    p.eta_secs = eta_secs(
                        at_ms - resumed_ms,
                        since.elapsed(),
                        plan.len_ms.saturating_sub(at_ms),
                    );
                });
                // Plan 5.1-11: judged again at every progress line.
                let _ = self.wake.send(());
            };
            // No speech by VAD: nothing to transcribe.
            let (segments, device) = if chunk.speech_ms == 0 {
                (Vec::new(), None)
            } else {
                let models = (model.as_path(), vad.as_path(), lid.as_deref());
                match self.chunk(id, &dir, chunk, models, &languages, report) {
                    Ok((segments, device)) => (segments, Some(device)),
                    Err(end) => return end,
                }
            };
            done.push(ChunkResult {
                chunk: index as u32,
                start_ms: chunk.start_ms,
                end_ms: chunk.end_ms,
                segments,
                languages: languages.clone(),
            });
            let saved = store.update(id, |m| {
                write_partial(&partial, &done).map_err(|e| format!("{PARTIAL} 저장 실패: {e}"))?;
                // Also for a plan made before `chunks_total` was written.
                (m.chunks_done, m.chunks_total) = (done.len() as u32, chunks);
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
        if let Err(e) = finished {
            return End::Failed(e);
        }
        // Plan 5.1-13: this run's pace, unless resuming left nothing to run.
        let audio_s = plan.len_ms.saturating_sub(resumed_ms) as f64 / 1000.0;
        if audio_s > 0.0 {
            let run_s = since.elapsed().as_secs_f64();
            let device = lock(&self.inner)
                .jobs
                .get(id)
                .and_then(|j| j.device.as_ref())
                .map_or("-", |(device, _)| device.name());
            let line = engine::diag_line(
                "meeting",
                &[
                    ("id", id.into()),
                    ("audio_s", format!("{audio_s:.1}")),
                    ("run_s", format!("{run_s:.1}")),
                    ("rtf", format!("{:.3}", run_s / audio_s)),
                    ("device", device.into()),
                ],
            );
            log::info!("{line}");
        }
        End::Done(Status::Transcribed)
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
                engine_since: None,
                // Plan 5.1-12: only `saver` goes on into the summary.
                slow: p.slow.filter(|s| *s == SlowReason::Saver),
                detecting: false,
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
        // Running, so its languages cannot change under it (`set_language`, `retranscribe`).
        let languages = store
            .get(id)
            .map_or_else(|_| lang::default_list(), |m| m.languages);
        let segments = store.transcript(id).map(|segments| {
            segments
                .into_iter()
                .map(|s| (s.text.trim().to_string(), s.lang))
                .filter(|(t, _)| !t.is_empty())
                .collect::<Vec<_>>()
        });
        let lines = segments
            .as_ref()
            .map(|segments| {
                segments
                    .iter()
                    .map(|(text, _)| text.clone())
                    .collect::<Vec<String>>()
            })
            .map_err(String::clone);
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
                let since = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs());
                self.set_progress(id, |p| p.engine_since = since.ok());
                let started = self.sidecar.start(generation, device);
                self.set_progress(id, |p| p.engine_since = None);
                let endpoint = match started {
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
        // Plan 5.6-16: the prompt's lines carry their language in a mixed meeting.
        let tagged: Vec<String> = segments
            .unwrap_or_default()
            .iter()
            .map(|(text, lang)| summarize::line(text, lang.as_deref(), &languages))
            .collect();
        let (app, run_id, asked) = (self.app.clone(), id.to_string(), glossary.clone());
        let spoken = languages.clone();
        let partial = dir.join(summarize::PARTIAL);
        let task = async move {
            let on_step = |done: u32, known: u32| {
                app.state::<Queue<R>>().set_progress(&run_id, |p| {
                    p.chunk = done;
                    p.chunks = known;
                    p.percent = (done * 100 / known.max(1)) as u8;
                });
            };
            let partial = Some(partial.as_path());
            summarize::summarize(
                &connection,
                key,
                &asked,
                &spoken,
                &tagged,
                on_step,
                recover,
                partial,
            )
            .await
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
        let grounded = grounding::check(&summary, &transcript, &glossary, &languages);
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
    /// recording stop, `Requeue` and a pause taken back while stopping keep the meeting in line
    /// (`back_in_line`).
    fn end(&self, store: &Store, id: &str, end: End) {
        let finished = matches!(end, End::Done(_) | End::Failed(_));
        match &end {
            End::Failed(message) => log::error!("처리 실패 ({id}): {message}"),
            End::Skipped(reason) => log::info!("처리 건너뜀 ({id}): {reason}"),
            _ => {}
        }
        // Plan 5.3-5, 5.7-1: `held` is read and the slot freed at once, under the meeting's lock
        // then the queue's, so a pause lands either before this (and is kept) or after it
        // (`AlreadyDone`).
        let (mut freed, mut status) = (false, None);
        let written = store.update(id, |m| {
            let mut inner = lock(&self.inner);
            let place = back_in_line(&end, inner.jobs.get(id).and_then(|j| j.stop), m.held);
            match &end {
                End::Done(_) => m.held = false,
                End::Failed(message) => {
                    let to = if m.status == Status::Summarizing {
                        Status::SummarizeFailed
                    } else {
                        Status::TranscribeFailed
                    };
                    if can_transition(m.status, to) {
                        m.status = to;
                        m.error = Some(message.clone());
                    }
                    m.held = false;
                }
                // Nothing on disk claims work that is not running (same as recovery, 8.4).
                End::Stopped(_) | End::Skipped(_) | End::Requeue
                    if (m.held || place.is_some() && matches!(end, End::Stopped(_)))
                        && can_transition(m.status, Status::Paused) =>
                {
                    m.status = Status::Paused;
                }
                _ => {}
            }
            status = match end {
                End::Done(status) => Some(status),
                // A quit leaves `paused` to `Store::pause_busy`.
                End::Stopped(Stop::Quit) => None,
                _ => Some(m.status),
            };
            self.free(&mut inner, id, place);
            freed = true;
            Ok(())
        });
        match written {
            Err(e) if freed => log::error!("처리 끝 기록 실패 ({id}): {e}"),
            // Deleted, or being deleted: only the slot is freed.
            Err(e) => {
                log::info!("처리 끝을 기록하지 않습니다 ({id}): {e}");
                let mut inner = lock(&self.inner);
                let place = back_in_line(&end, inner.jobs.get(id).and_then(|j| j.stop), false);
                self.free(&mut inner, id, place);
            }
            Ok(_) => {}
        }
        // The status is on disk before the event that tells the screens to read it.
        self.report(id, status);
        self.pump();
        // After the next one started, so the system is not let go between them.
        let _ = self.wake.send(());
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

    /// A run's end in the line: its slot and the lease freed, and back in line at `place` or out
    /// of it.
    fn free(&self, inner: &mut Inner, id: &str, place: Option<Place>) {
        // Plan 5.3: no idle engine. It would hold 4 GB while the next transcription runs. The
        // meetings passed over for the lease may start now.
        if inner.sched.release_lease(id) {
            self.sidecar.stop("요약 끝");
        }
        inner.sched.finished(id);
        inner.jobs.remove(id);
        match place {
            // Finished while a recording put it back in line: nothing left to run.
            None => inner.sched.waiting.retain(|w| w != id),
            // A recording put it back already, in the order the runs started.
            Some(_) if inner.sched.waiting.iter().any(|w| w == id) => {}
            Some(Place::First) => inner.sched.waiting.push_front(id.to_string()),
            Some(Place::Last) => inner.sched.waiting.push_back(id.to_string()),
        }
    }

    /// A run's last progress, with the status it ended in, once that is on disk.
    fn report(&self, id: &str, status: Option<Status>) {
        let mut inner = lock(&self.inner);
        // Not one that started again meanwhile: its own run reports it.
        let waiting = inner.sched.waiting.iter().any(|w| w == id);
        let entry = if inner.sched.running.iter().any(|r| r == id) {
            None
        } else if waiting {
            inner.progress.get_mut(id).cloned()
        } else {
            inner.progress.remove(id)
        };
        if let (Some(mut p), Some(status)) = (entry, status) {
            p.status = status;
            p.eta_secs = None;
            p.slow = None;
            p.detecting = false;
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

/// Plan 5.1-9, 5.1-11: the queue's power thread, until the app exits. While any meeting runs it
/// holds the system awake (Windows ties that to this thread); every `TICK` and whenever woken it
/// judges the running meetings' slow reasons again.
fn watch<R: Runtime>(app: &AppHandle<R>, woken: &mpsc::Receiver<()>) {
    #[cfg(windows)]
    watch_power_mode();
    // Set while busy, even when taking the hold failed: one try per busy spell.
    let mut held = None;
    loop {
        if woken.recv_timeout(TICK) == Err(RecvTimeoutError::Disconnected) {
            return;
        }
        while woken.try_recv().is_ok() {}
        // Not managed: a test's queue that never runs anything.
        let Some(queue) = app.try_state::<Queue<R>>() else {
            return;
        };
        let Some(busy) = queue.judge() else {
            return;
        };
        if !busy {
            held = None;
        } else if held.is_none() {
            held = Some((queue.awake)());
        }
    }
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
        let mut run = ChunkRun::new(Device::Gpu, false);
        assert_eq!(run.after(Exit::Crash, false), Next::Run(Device::Gpu));
        assert_eq!(run.after(Exit::Crash, false), Next::GpuFailed);
        assert_eq!(run.device, Device::Cpu);
        assert_eq!(run.after(Exit::Crash, false), Next::Fail);

        let mut run = ChunkRun::new(Device::Gpu, false);
        assert_eq!(run.after(Exit::Crash, false), Next::Run(Device::Gpu));
        assert_eq!(run.after(Exit::Done(Device::Gpu), false), Next::Accept);
        assert_eq!(run.device, Device::Gpu);
    }

    /// Plan 5.1-6: a GPU crash with flash attention on turns it off first, uncounted; the CPU's
    /// crash is never flash attention's.
    #[test]
    fn a_gpu_crash_with_flash_attention_turns_it_off_before_counting() {
        let mut run = ChunkRun::new(Device::Gpu, true);
        assert_eq!(run.after(Exit::Crash, false), Next::FlashAttnOff);
        assert_eq!(
            (run.device, run.flash_attn, run.gpu_crashes),
            (Device::Gpu, false, 0)
        );
        assert_eq!(run.after(Exit::Stalled, false), Next::Run(Device::Gpu));
        assert_eq!(run.after(Exit::Crash, false), Next::GpuFailed);

        let mut run = ChunkRun::new(Device::Cpu, true);
        assert_eq!(run.after(Exit::Crash, false), Next::Fail);
    }

    #[test]
    fn input_and_model_errors_fail_without_blaming_the_gpu() {
        for exit in [Exit::Input, Exit::Model] {
            let mut run = ChunkRun::new(Device::Gpu, false);
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
        let mut run = ChunkRun::new(Device::Gpu, false);
        assert_eq!(run.after(Exit::OutOfMemory, false), Next::Fail);
        assert_eq!(run.gpu_crashes, 0);

        let stalled = Ended {
            stalled: true,
            killed: true,
            ..Ended::default()
        };
        assert_eq!(exit_of(stalled, None), Exit::Stalled);
        let mut run = ChunkRun::new(Device::Gpu, false);
        assert_eq!(run.after(Exit::Stalled, false), Next::Run(Device::Gpu));
        assert_eq!(run.after(Exit::Stalled, false), Next::GpuFailed);
    }

    #[test]
    fn abnormal_gpu_result_is_redone_once_on_the_cpu() {
        let mut run = ChunkRun::new(Device::Gpu, false);
        assert_eq!(
            run.after(Exit::Done(Device::Gpu), true),
            Next::Run(Device::Cpu)
        );
        assert_eq!(run.after(Exit::Done(Device::Cpu), true), Next::Accept);
        // Already on the CPU (forced, or the GPU fell back): kept.
        let mut run = ChunkRun::new(Device::Gpu, false);
        assert_eq!(run.after(Exit::Done(Device::Cpu), true), Next::Accept);
        let mut run = ChunkRun::new(Device::Cpu, false);
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
                lang: None,
            }],
            languages: vec!["ko".into()],
        }
    }

    #[test]
    fn resumes_after_the_last_finished_chunk() {
        let ko = vec!["ko".to_string()];
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PARTIAL);
        let plan = [chunk(0, 10), chunk(10, 20), chunk(20, 30)];
        assert!(read_partial(&path, &plan, &ko).is_empty());

        write_partial(&path, &[result(0, &plan[0]), result(1, &plan[1])]).unwrap();
        assert_eq!(read_partial(&path, &plan, &ko).len(), 2);

        // A torn last line (an older appending writer, a disk error) is redone, not trusted.
        let mut text = fs::read_to_string(&path).unwrap();
        text.truncate(text.len() - 10);
        fs::write(&path, text).unwrap();
        assert_eq!(read_partial(&path, &plan, &ko), [result(0, &plan[0])]);

        // Lines that do not match the plan stop the resume there.
        write_partial(&path, &[result(0, &plan[0]), result(1, &chunk(10, 25))]).unwrap();
        assert_eq!(read_partial(&path, &plan, &ko).len(), 1);
        write_partial(&path, &[result(1, &plan[1])]).unwrap();
        assert!(read_partial(&path, &plan, &ko).is_empty());
    }

    /// Plan 5.6-7: a chunk stamped with other languages (left by an E3 whose deletion failed) is
    /// never resumed; a line from before 1.1.0, without a stamp, was Korean.
    #[test]
    fn a_leftover_in_other_languages_is_not_resumed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PARTIAL);
        let plan = [chunk(0, 10), chunk(10, 20)];
        write_partial(&path, &[result(0, &plan[0]), result(1, &plan[1])]).unwrap();
        let ko_vi = vec!["ko".to_string(), "vi".into()];
        assert!(read_partial(&path, &plan, &ko_vi).is_empty());
        let old = r#"{"chunk":0,"start_ms":0,"end_ms":10,"segments":[]}"#;
        fs::write(&path, format!("{old}\n")).unwrap();
        assert_eq!(read_partial(&path, &plan, &["ko".into()]).len(), 1);
        assert!(read_partial(&path, &plan, &["en".into()]).is_empty());
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

    /// Plan 5.1-11: slower than the recording, where the worker runs and what was chosen say why.
    /// Saver goes first and is the only reason while summarizing (no pace).
    #[test]
    fn a_slow_meeting_says_why() {
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        let cpu = |gpu: &str| (Device::Cpu, gpu.to_string());
        // 60 s of audio in 90 s.
        let slow = || {
            let mut pace = Pace::new(t0, 0);
            pace.progress(at(90), 60_000);
            pace
        };
        let judge = |saver, backend, device: &(Device, String)| {
            slow_reason(saver, Some(&mut slow()), backend, Some(device), at(90))
        };
        assert_eq!(
            judge(false, Backend::Auto, &cpu("Radeon")),
            Some(SlowReason::Cpu)
        );
        assert_eq!(
            judge(false, Backend::Gpu, &cpu("Radeon")),
            Some(SlowReason::Cpu)
        );
        assert_eq!(
            judge(false, Backend::Auto, &cpu("")),
            Some(SlowReason::CpuNogpu)
        );
        assert_eq!(
            judge(false, Backend::Cpu, &cpu("Radeon")),
            Some(SlowReason::Slow)
        );
        assert_eq!(judge(false, Backend::Cpu, &cpu("")), Some(SlowReason::Slow));
        let gpu = (Device::Gpu, "Radeon".to_string());
        assert_eq!(judge(false, Backend::Auto, &gpu), Some(SlowReason::Slow));
        assert_eq!(
            judge(true, Backend::Auto, &cpu("")),
            Some(SlowReason::Saver)
        );
        assert_eq!(
            slow_reason(true, None, Backend::Auto, None, at(90)),
            Some(SlowReason::Saver)
        );
        assert_eq!(slow_reason(false, None, Backend::Auto, None, at(90)), None);
        // What the screens read.
        assert_eq!(
            serde_json::to_value(SlowReason::CpuNogpu).unwrap(),
            "cpu_nogpu"
        );

        // Plugged in: the next tick clears saver with no progress line in between.
        let mut pace = Pace::new(t0, 0);
        pace.progress(at(30), 300_000);
        let tick =
            |saver, pace: &mut Pace, s| slow_reason(saver, Some(pace), Backend::Auto, None, at(s));
        assert_eq!(tick(true, &mut pace, 61), Some(SlowReason::Saver));
        assert_eq!(tick(false, &mut pace, 91), None);
    }

    /// Plan 5.1-11: a measurement keeps its last verdict for its first minute, real time is not
    /// slow, and the CPU taking over from the GPU is measured on its own.
    #[test]
    fn the_pace_holds_its_first_minute_and_starts_again_on_the_cpu() {
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        let radeon = (Device::Cpu, "Radeon".to_string());
        let mut pace = Pace::new(t0, 0);
        // 30 s of audio in 40 s.
        pace.progress(at(40), 30_000);
        assert_eq!(pace.judge(at(59), Backend::Auto, Some(&radeon)), None);
        assert_eq!(
            pace.judge(at(60), Backend::Auto, Some(&radeon)),
            Some(SlowReason::Cpu)
        );
        pace.progress(at(70), 70_000);
        assert_eq!(pace.judge(at(75), Backend::Auto, Some(&radeon)), None);

        // The GPU did 10 min of audio in its first minute, then crashed over to the CPU.
        let mut job = Job {
            pace: Some(Pace::new(t0, 0)),
            ..Job::default()
        };
        job.ran_on(Device::Gpu, "Radeon".into());
        let judge = |job: &mut Job, s| {
            let Job { pace, device, .. } = job;
            pace.as_mut()
                .unwrap()
                .judge(at(s), Backend::Auto, device.as_ref())
        };
        job.pace.as_mut().unwrap().progress(at(60), 600_000);
        assert_eq!(judge(&mut job, 61), None);
        job.ran_on(Device::Cpu, "Radeon".into());
        // The CPU runs the chunk again from its start: 30 s of audio in 50 s from its first line.
        job.pace.as_mut().unwrap().progress(at(80), 540_000);
        job.pace.as_mut().unwrap().progress(at(130), 570_000);
        assert_eq!(judge(&mut job, 139), None);
        // Measured from the run's start it would look fast: 570 s of audio in 130 s.
        assert_eq!(judge(&mut job, 140), Some(SlowReason::Cpu));

        // Measured again: the last verdict stays until a minute in.
        job.ran_on(Device::Gpu, "Radeon".into());
        job.ran_on(Device::Cpu, "Radeon".into());
        job.pace.as_mut().unwrap().progress(at(150), 600_000);
        assert_eq!(judge(&mut job, 200), Some(SlowReason::Cpu));
        // From the CPU to the GPU is not measured again.
        job.ran_on(Device::Gpu, "Radeon".into());
        job.pace.as_mut().unwrap().progress(at(210), 1_200_000);
        assert_eq!(judge(&mut job, 211), None);
    }

    /// Plan 5.7-4: a lower stop never covers a higher one, so a delete is never turned back into
    /// a pause and a pause never into a recording's stop.
    #[test]
    fn stops_are_ordered_and_halt_keeps_the_larger() {
        use Stop::*;
        assert!(Recording < Quit && Quit < User && User < Delete);
        let mut job = Job::default();
        job.halt(Recording);
        job.halt(User);
        job.halt(Recording);
        assert_eq!(job.stop, Some(User));
        job.halt(Delete);
        for lower in [Recording, Quit, User] {
            job.halt(lower);
        }
        assert_eq!(job.stop, Some(Delete));
    }

    /// Plan 5.3-5: where a run goes as it ends.
    #[test]
    fn a_run_goes_back_in_line_only_unpaused_and_undeleted() {
        use Stop::*;
        let stopped = |s| End::Stopped(s);
        let skipped = || End::Skipped(String::new());
        let cases = [
            (End::Done(Status::Review), Some(User), true, None),
            (End::Failed(String::new()), None, true, None),
            // Held: it waits `paused` however it stopped.
            (stopped(Recording), Some(Recording), true, None),
            (stopped(User), Some(User), true, None),
            (End::Requeue, None, true, None),
            (skipped(), Some(User), true, None),
            (stopped(Recording), Some(Delete), false, None),
            (End::Requeue, Some(Quit), false, None),
            (
                stopped(Recording),
                Some(Recording),
                false,
                Some(Place::First),
            ),
            (End::Requeue, None, false, Some(Place::First)),
            // Paused during a recording's stop, then taken back.
            (stopped(Recording), Some(User), false, Some(Place::Last)),
            (stopped(User), Some(User), false, Some(Place::Last)),
            (skipped(), None, false, None),
        ];
        for (end, stop, held, place) in cases {
            let what = format!("{stop:?} held={held}");
            assert_eq!(back_in_line(&end, stop, held), place, "{what}");
        }
    }

    /// Plan 5.3-6: a recording starting while a pause winds a run down leaves that run out of
    /// line, so it never holds up the meetings behind it.
    #[test]
    fn a_pausing_run_never_holds_up_the_line_through_a_recording() {
        let mut inner = Inner::default();
        for id in ["a", "b", "c"] {
            inner.sched.enqueue(id);
        }
        inner.sched.start_next(2, |_| true);
        for id in ["a", "b"] {
            inner.jobs.insert(id.into(), Job::default());
        }
        inner.jobs.get_mut("a").unwrap().halt(Stop::User);
        inner.suspend();
        assert_eq!(inner.jobs["a"].stop, Some(Stop::User));
        assert_eq!(inner.jobs["b"].stop, Some(Stop::Recording));
        let s = &mut inner.sched;
        assert_eq!((s.position("a"), s.position("b")), (None, Some(1)));

        // The recording is over and b wound down; a is still stopping.
        s.recording = false;
        s.finished("b");
        assert_eq!(s.start_next(2, |_| true), ids(&["b"]));
        s.finished("a");
        assert_eq!(s.start_next(2, |_| true), ids(&["c"]));
    }

    /// For the unix tests below, which drive real processes through `sh`.
    #[cfg(unix)]
    fn job(inner: &Mutex<Inner>, id: &str) {
        let mut inner = lock(inner);
        inner.sched.enqueue(id);
        inner.sched.start_next(2, |_| true);
        inner.jobs.insert(id.into(), Job::default());
    }

    #[cfg(unix)]
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

    /// A meeting in `status` (`engine_queue`'s store).
    fn meeting(app: &tauri::App<tauri::test::MockRuntime>, status: Status) -> String {
        let store = app.state::<Store>();
        let id = store
            .create("", false, crate::transcribe::lang::default_list())
            .unwrap()
            .id;
        store
            .update(&id, |m| {
                m.status = status;
                Ok(())
            })
            .unwrap();
        id
    }

    fn held(app: &tauri::App<tauri::test::MockRuntime>, id: &str) -> (Status, bool) {
        let meta = app.state::<Store>().get(id).unwrap();
        (meta.status, meta.held)
    }

    /// Plan 5.3-4: a finished meeting's pause writes nothing; a bad id is refused.
    #[test]
    fn pausing_a_finished_meeting_is_already_done_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, queue) = engine_queue(tmp.path(), "supervisor", Duration::from_secs(10));
        let store = app.state::<Store>();
        let id = meeting(&app, Status::Review);
        let path = store.dir_for(&id).unwrap().join("meta.json");
        let before = fs::read(&path).unwrap();
        assert_eq!(queue.pause(&store, &id), Ok(Pause::AlreadyDone));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(queue.pause(&store, "../meetings").is_err());
        assert!(queue.pause(&store, "2026-11-03T0900-0000").is_err());
    }

    /// Plan 5.3-3: retried failures waiting in line can be paused too, and stay out of it.
    #[test]
    fn failed_meetings_retried_in_line_can_be_paused() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, queue) = engine_queue(tmp.path(), "supervisor", Duration::from_secs(10));
        let store = app.state::<Store>();
        // Nothing starts: the test reads the line as it is.
        lock(&queue.inner).sched.recording = true;
        for status in [Status::TranscribeFailed, Status::SummarizeFailed] {
            let id = meeting(&app, status);
            queue.enqueue(&id, status);
            assert_eq!(lock(&queue.inner).sched.position(&id), Some(1));
            assert_eq!(queue.pause(&store, &id), Ok(Pause::Paused));
            assert_eq!(held(&app, &id), (Status::Paused, true));
            let inner = lock(&queue.inner);
            assert_eq!(inner.sched.position(&id), None);
            assert!(!inner.progress.contains_key(&id));
        }
        // A list read again lines up neither (as at the next start).
        queue.enqueue_pending();
        assert!(queue.snapshot().is_empty());
    }

    /// Plan 5.3-5, 5.7-4: a delete landing on a pausing run wins; nothing goes back in line.
    #[test]
    fn a_delete_beats_a_pause() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, queue) = engine_queue(tmp.path(), "supervisor", Duration::from_secs(10));
        let store = app.state::<Store>();
        let id = meeting(&app, Status::Transcribing);
        running(&queue, &id);
        assert_eq!(queue.pause(&store, &id), Ok(Pause::Pausing));
        thread::scope(|s| {
            let deleting = s.spawn(|| store.delete_with(&id, || queue.cancel(&id)));
            while lock(&queue.inner).jobs[&id].stop != Some(Stop::Delete) {
                thread::sleep(Duration::from_millis(10));
            }
            // The run ends as the pause stopped it.
            queue.end(&store, &id, End::Stopped(Stop::User));
            deleting.join().unwrap().unwrap();
        });
        assert!(!store.dir_for(&id).unwrap().exists());
        let inner = lock(&queue.inner);
        assert_eq!(inner.sched.position(&id), None);
        assert!(inner.sched.running.is_empty() && inner.jobs.is_empty());
    }

    /// Plan 5.3-7: "이어서 처리" while the run still stops: its `end` puts it back, last.
    #[test]
    fn resuming_while_it_stops_puts_it_back_last() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, queue) = engine_queue(tmp.path(), "supervisor", Duration::from_secs(10));
        let store = app.state::<Store>();
        let id = meeting(&app, Status::Transcribing);
        running(&queue, &id);
        let behind = meeting(&app, Status::Recorded);
        queue.enqueue(&behind, Status::Recorded);
        assert_eq!(queue.pause(&store, &id), Ok(Pause::Pausing));
        assert_eq!(held(&app, &id), (Status::Transcribing, true));

        assert!(!queue.resume(&store, &id).unwrap().held);
        assert_eq!(lock(&queue.inner).sched.position(&id), None);
        queue.end(&store, &id, End::Stopped(Stop::User));
        assert_eq!(held(&app, &id), (Status::Paused, false));
        let inner = lock(&queue.inner);
        assert_eq!(
            (inner.sched.position(&behind), inner.sched.position(&id)),
            (Some(1), Some(2))
        );
    }

    /// Plan 5.3-5: a held run that ends skipped or back in line for the lease waits `paused`,
    /// out of line, until it is resumed.
    #[test]
    fn a_held_run_that_skips_or_requeues_ends_paused_and_resumes() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, queue) = engine_queue(tmp.path(), "supervisor", Duration::from_secs(10));
        let store = app.state::<Store>();
        let skips = meeting(&app, Status::Transcribed);
        let requeues = meeting(&app, Status::Transcribed);
        for id in [&skips, &requeues] {
            running(&queue, id);
            assert_eq!(queue.pause(&store, id), Ok(Pause::Pausing));
        }
        queue.end(
            &store,
            &skips,
            End::Skipped("요약 경로를 기다립니다".into()),
        );
        queue.end(&store, &requeues, End::Requeue);
        for id in [&skips, &requeues] {
            assert_eq!(held(&app, id), (Status::Paused, true));
            assert_eq!(lock(&queue.inner).sched.position(id), None);
        }
        // Nothing starts here (no model): the line is read as it is.
        assert_eq!(queue.resume(&store, &skips).unwrap().status, Status::Paused);
        assert_eq!(held(&app, &skips), (Status::Paused, false));
        assert_eq!(lock(&queue.inner).sched.position(&skips), Some(1));
    }

    /// Plan 5.3-17: paused while a recording that started has put the run back first in line and
    /// it still winds down. It leaves the line at once and ends `paused`; the meeting behind it
    /// is next.
    #[test]
    fn a_pause_after_a_recording_stopped_the_run_takes_it_out_of_line() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, queue) = engine_queue(tmp.path(), "supervisor", Duration::from_secs(10));
        let store = app.state::<Store>();
        let id = meeting(&app, Status::Transcribing);
        running(&queue, &id);
        let behind = meeting(&app, Status::Recorded);
        queue.enqueue(&behind, Status::Recorded);
        queue.recording_started();
        assert_eq!(lock(&queue.inner).sched.position(&id), Some(1));

        assert_eq!(queue.pause(&store, &id), Ok(Pause::Pausing));
        assert_eq!(lock(&queue.inner).sched.position(&id), None);
        queue.end(&store, &id, End::Stopped(Stop::Recording));
        assert_eq!(held(&app, &id), (Status::Paused, true));
        let mut inner = lock(&queue.inner);
        inner.sched.recording = false;
        assert_eq!(inner.sched.start_next(2, |_| true), ids(&[&behind]));
    }

    /// Plan 5.7-3: a list read before a pause cannot put the paused meeting back in line, since
    /// `held` is read again as it joins.
    #[test]
    fn a_list_read_before_a_pause_does_not_line_the_meeting_up_again() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, queue) = engine_queue(tmp.path(), "supervisor", Duration::from_secs(10));
        let store = app.state::<Store>();
        let id = meeting(&app, Status::Recorded);
        queue.enqueue(&id, Status::Recorded);
        let barrier = std::sync::Barrier::new(2);
        thread::scope(|s| {
            s.spawn(|| {
                let listed = store.list();
                assert!(!listed[0].held);
                barrier.wait();
                // The pause lands here.
                barrier.wait();
                queue.enqueue_listed(listed);
            });
            barrier.wait();
            assert_eq!(queue.pause(&store, &id), Ok(Pause::Paused));
            barrier.wait();
        });
        assert_eq!(held(&app, &id), (Status::Paused, true));
        assert_eq!(lock(&queue.inner).sched.position(&id), None);
        assert!(queue.snapshot().is_empty());
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
            /// Plan 5.1-9's holds: taken, and let go.
            holds: Arc<(AtomicUsize, AtomicUsize)>,
            tmp: tempfile::TempDir,
        }

        /// The stand-in for the OS's hold: counts itself let go.
        struct Hold(Arc<(AtomicUsize, AtomicUsize)>);

        impl Drop for Hold {
            fn drop(&mut self) {
                self.0.1.fetch_add(1, Ordering::SeqCst);
            }
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
            let holds = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
            let taking = holds.clone();
            // Never the real keychain, nor the OS's sleep settings.
            app.manage(Queue {
                secret: Box::new(|_| Ok(Some("sk-test".into()))),
                sidecar: Arc::new(engine.sidecar),
                awake: Box::new(move || {
                    taking.0.fetch_add(1, Ordering::SeqCst);
                    Some(Box::new(Hold(taking.clone())) as Box<dyn Any>)
                }),
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
                holds,
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
                let id = self
                    .store()
                    .create("", false, crate::transcribe::lang::default_list())
                    .unwrap()
                    .id;
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
                let id = self
                    .store()
                    .create("", summarize_local, crate::transcribe::lang::default_list())
                    .unwrap()
                    .id;
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
                    text: "구간 0".into(),
                    lang: None,
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

        /// Plan 5.1-9: the system is held awake once while a meeting is processed, and let go when
        /// nothing runs.
        #[cfg(unix)]
        #[test]
        fn processing_holds_the_system_awake_until_nothing_runs() {
            // The chunk waits for the test.
            let f = fixture(
                &format!(r#"while [ ! -e "$2/../go" ]; do sleep 0.05; done; {RESULT}"#),
                Backend::Auto,
            );
            let id = f.recorded();
            let holds = || {
                (
                    f.holds.0.load(Ordering::SeqCst),
                    f.holds.1.load(Ordering::SeqCst),
                )
            };
            assert_eq!(holds(), (0, 0));
            f.queue().enqueue_pending();
            f.wait("held", || holds() == (1, 0));
            f.wait("transcribing", || f.transcribing(&id));
            assert_eq!(holds(), (1, 0));

            let dir = f.store().dir_for(&id).unwrap();
            fs::write(dir.parent().unwrap().join("go"), "").unwrap();
            f.wait("let go", || holds() == (1, 1));
            assert_eq!(f.status(&id), Status::Review);
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

        /// Plan 5.1-6: `${15}` is `--fa`. The crash turns it off for good, and the GPU finishes.
        #[cfg(unix)]
        #[test]
        fn a_gpu_crash_with_flash_attention_runs_the_gpu_again_without_it() {
            let f = fixture(
                &format!(
                    r#"if [ "$5" = gpu ] && [ "${{15}}" = 1 ]; then echo 'fa fault' >&2; exit 1; fi; {RESULT}"#
                ),
                Backend::Auto,
            );
            let id = f.recorded();
            f.queue().enqueue_pending();
            f.wait("done", || f.status(&id) == Status::Review);
            assert_eq!(f.runs(&id), ["gpu", "gpu"]);
            assert_eq!(f.store().get(&id).unwrap().backend.as_deref(), Some("gpu"));
            let settings = f.app.state::<SettingsStore>().load().unwrap().settings;
            assert!(!settings.transcribe_flash_attn && !settings.gpu_failed);
            assert_eq!(*f.gpu_notices.lock().unwrap(), 0);
        }

        /// With flash attention already off, as before it was there.
        #[cfg(unix)]
        #[test]
        fn two_gpu_crashes_mark_gpu_failed_and_the_cpu_finishes() {
            let f = fixture(
                &format!(r#"if [ "$5" = gpu ]; then echo 'gpu fault' >&2; exit 1; fi; {RESULT}"#),
                Backend::Auto,
            );
            f.app
                .state::<SettingsStore>()
                .turn_off_flash_attn()
                .unwrap();
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

        /// Plan 5.3, DX6: while the engine starts, the meeting's progress says since when, so the
        /// screens show "요약 엔진 준비 중 · N초" instead of a summary stuck at 0/0; ready, it is gone.
        #[test]
        fn a_builtin_summary_says_while_its_engine_starts() {
            let _model = summary_model();
            let f = fixture(SPEECH, Backend::Auto);
            f.install_summary();
            let id = f.transcribed(false, None);
            f.queue().enqueue_pending();
            f.wait("review", || f.status(&id) == Status::Review);
            let events = f.events.lock().unwrap();
            let mine: Vec<_> = events.iter().filter(|e| e["id"] == id.as_str()).collect();
            let set = mine.iter().position(|e| e["engine_since"].is_u64());
            let set = set.expect("an event while the engine starts");
            assert_eq!(mine[set]["status"], "summarizing");
            let cleared = mine[set + 1..]
                .iter()
                .position(|e| e["engine_since"].is_null());
            assert_eq!(mine[set + 1 + cleared.unwrap()]["status"], "summarizing");
            assert!(mine.last().unwrap()["engine_since"].is_null());
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

        /// Plan 5.6-6: a `paused` meeting with its `transcript.json` is summary only once a chunk
        /// was done. A `transcript.json` E3 left at `chunks_done` 0 is never summarized.
        #[test]
        fn a_paused_meeting_is_summary_only_once_a_chunk_was_done() {
            let f = fixture(RESULT, Backend::Cpu);
            let (id, store) = (f.transcribed(false, None), f.store());
            let paused = |done| {
                store
                    .update(&id, |m| {
                        (m.status, m.chunks_done) = (Status::Paused, done);
                        Ok(())
                    })
                    .unwrap()
            };
            assert!(!summary_only(&store, &paused(0)));
            assert!(summary_only(&store, &paused(1)));
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
                // Its end reported too: back in line, first.
                inner.sched.blocked.contains(&id)
                    && inner.sched.running.is_empty()
                    && inner
                        .progress
                        .get(&id)
                        .is_some_and(|p| p.position == Some(1))
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

        /// Plan 5.3-12: a plan saved by 1.0.3 has no `chunks_total`. It is written as the run
        /// starts, so a pause before the next chunk ends still reads "구간 1/2에서 멈춤".
        #[cfg(unix)]
        #[test]
        fn a_plan_saved_without_its_total_gets_it_when_the_run_starts() {
            let f = fixture(&format!("{GATED}{RESULT}"), Backend::Cpu);
            let id = f.recorded();
            let dir = f.store().dir_for(&id).unwrap();
            fs::write(dir.join("chunks.json"), PLAN).unwrap();
            f.queue().enqueue_pending();
            f.wait("transcribing", || f.transcribing(&id));
            assert_eq!(f.store().get(&id).unwrap().chunks_total, 2);
            fs::write(dir.parent().unwrap().join("go"), "").unwrap();
            f.wait("done", || f.queue().snapshot().is_empty());
        }

        /// Plan 5.3-17: a paused run stays `paused` and held through a recording and a restart,
        /// and "이어서 처리" picks it up from the chunk it stopped in.
        #[cfg(unix)]
        #[test]
        fn a_paused_meeting_stays_paused_until_it_is_resumed() {
            // Hangs the first time any chunk runs, answers after that.
            let f = fixture(
                &format!(
                    r#"if [ ! -e "$2/../slept" ]; then touch "$2/../slept"; echo '{{"type":"progress","pct":10}}'; exec sleep 30; fi; {RESULT}"#
                ),
                Backend::Auto,
            );
            let id = f.recorded();
            f.queue().enqueue_pending();
            let marker = f.tmp.path().join("meetings").join("slept");
            f.wait("chunk hanging", || marker.exists() && f.transcribing(&id));

            assert_eq!(f.queue().pause(&f.store(), &id), Ok(Pause::Pausing));
            f.wait("paused", || f.queue().snapshot().is_empty());
            let meta = f.store().get(&id).unwrap();
            assert_eq!((meta.status, meta.held), (Status::Paused, true));
            // Plan 5.3-8: where it stopped, for "구간 1/2에서 멈춤" after a restart too.
            assert_eq!((meta.chunks_done, meta.chunks_total), (0, 2));
            assert_eq!(f.events.lock().unwrap().last().unwrap()["status"], "paused");

            // A recording, a meeting recorded meanwhile, and the next start leave it as it is.
            f.queue().recording_started();
            let other = f.recorded();
            f.queue().recording_finished();
            f.wait("the other done", || f.status(&other) == Status::Review);
            f.queue().enqueue_pending();
            thread::sleep(Duration::from_millis(200));
            let meta = f.store().get(&id).unwrap();
            assert_eq!((meta.status, meta.held), (Status::Paused, true));
            assert!(f.queue().snapshot().is_empty());

            f.queue().resume(&f.store(), &id).unwrap();
            f.wait("review", || f.status(&id) == Status::Review);
            assert!(!f.store().get(&id).unwrap().held);
            // The stopped chunk ran again from its start.
            assert_eq!(f.runs(&id), ["gpu", "gpu"]);
        }

        /// Plan 5.7-1: a built-in summary starting (`begin` takes the meeting's lock, then the
        /// queue's) beside pauses and resumes (the same order) never deadlocks.
        #[test]
        fn a_summary_starting_beside_pauses_and_resumes_never_deadlocks() {
            let _model = summary_model();
            let f = fixture(SPEECH, Backend::Auto);
            f.install_summary();
            let id = f.transcribed(false, None);
            let (app, run_id) = (f.app.handle().clone(), id.clone());
            let (done, finished) = mpsc::channel();
            thread::spawn(move || {
                let (queue, store) = (app.state::<Mock>(), app.state::<Store>());
                queue.enqueue_pending();
                for _ in 0..20 {
                    let _ = queue.pause(&store, &run_id);
                    thread::sleep(Duration::from_millis(5));
                    let _ = queue.resume(&store, &run_id);
                    thread::sleep(Duration::from_millis(5));
                }
                let _ = done.send(());
            });
            let ended = finished.recv_timeout(Duration::from_secs(30));
            assert!(
                ended.is_ok(),
                "deadlocked; queue {:?}",
                f.queue().snapshot()
            );
            f.wait("review", || f.status(&id) == Status::Review);
            assert!(!f.store().get(&id).unwrap().held);
            f.engine_gone();
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

        /// The worker waits for `meetings/go` before it answers.
        const GATED: &str = r#"while [ ! -e "$2/../go" ]; do sleep 0.05; done; "#;

        fn codes(codes: &[&str]) -> Vec<String> {
            codes.iter().map(|c| c.to_string()).collect()
        }

        fn header(f: &Fixture, id: &str) -> String {
            let log = f.store().dir_for(id).unwrap().join("worker.log");
            fs::read_to_string(log)
                .unwrap()
                .lines()
                .filter(|l| l.starts_with("--- --transcribe-worker"))
                .collect::<Vec<_>>()
                .join("\n")
        }

        /// Plan 5.6-6, 5.6-30 (E3): `recorded` with the new languages and the old results' fields
        /// cleared ("로컬로 요약" unpinned), every old file gone before a worker runs, the chunk
        /// plan kept, then transcribed again in the new language.
        #[cfg(unix)]
        #[test]
        fn e3_clears_the_results_resets_the_meeting_and_transcribes_it_again() {
            let f = fixture(&format!("{GATED}{RESULT}"), Backend::Cpu);
            let id = f.transcribed(true, Some("cloud"));
            let dir = f.store().dir_for(&id).unwrap();
            for name in crate::store::RESULTS {
                fs::write(dir.join(name), "옛 결과").unwrap();
            }
            fs::write(dir.join("chunks.json"), PLAN).unwrap();
            f.store()
                .update(&id, |m| {
                    m.status = Status::Review;
                    m.title = "주간 회의".into();
                    (m.backend, m.chunks_done, m.chunks_total) = (Some("gpu".into()), 2, 2);
                    (m.grounding_replaced, m.grounding_checked) = (1, Some(true));
                    m.no_speech = true;
                    Ok(())
                })
                .unwrap();
            let meta = f
                .queue()
                .retranscribe(&f.store(), &id, &codes(&["en"]), || false)
                .unwrap();
            assert_eq!(
                (meta.status, meta.languages.as_slice(), meta.chunks_done),
                (Status::Recorded, &codes(&["en"])[..], 0)
            );
            assert_eq!(
                (meta.backend, meta.llm_connection, meta.error),
                (None, None, None)
            );
            assert_eq!((meta.grounding_replaced, meta.grounding_checked), (0, None));
            assert!(!meta.no_speech && !meta.held);
            // The title, the plan and "로컬로 요약" stay.
            assert_eq!((meta.title.as_str(), meta.chunks_total), ("주간 회의", 2));
            assert!(meta.summarize_local);
            f.wait("transcribing", || f.transcribing(&id));
            assert!(
                crate::store::RESULTS
                    .iter()
                    .all(|name| !dir.join(name).exists())
            );
            assert!(dir.join("chunks.json").exists());
            fs::write(dir.parent().unwrap().join("go"), "").unwrap();
            f.wait("done", || f.queue().snapshot().is_empty());
            assert_eq!(f.status(&id), Status::Review);
            // Only the English list filters now: the Korean phrase stays (plan 5.6-13).
            assert_eq!(
                f.transcript(&id),
                serde_json::json!({"segments": [
                    {"start_ms": 0, "end_ms": 10000, "text": "구간 0"},
                    {"start_ms": 0, "end_ms": 10000, "text": "시청해주셔서 감사합니다."}
                ]})
            );
            let header = header(&f, &id);
            assert!(
                header.contains("--lang en ") && !header.contains("--lid-model"),
                "{header}"
            );
        }

        /// Plan 5.6-6, 5.7-7: refused while it runs or sends, before it was transcribed, once it
        /// was sent, and for a bad list or id. Nothing changes then.
        #[cfg(unix)]
        #[test]
        fn e3_is_refused_while_running_sending_or_out_of_its_states() {
            let f = fixture(&format!("{GATED}{RESULT}"), Backend::Cpu);
            let (queue, store) = (f.queue(), f.store());
            let e3 = |id: &str, codes: &[String], sending: bool| {
                queue.retranscribe(&store, id, codes, || sending)
            };
            let review = f.transcribed(false, None);
            store.set_status(&review, Status::Summarizing).unwrap();
            store.set_status(&review, Status::Review).unwrap();
            let before = store.get(&review).unwrap();
            let sending = e3(&review, &codes(&["en"]), true).unwrap_err();
            assert_eq!(sending, "보내는 중입니다");
            for bad in [vec![], codes(&["en", "en"]), codes(&["ja"])] {
                assert!(e3(&review, &bad, false).is_err(), "{bad:?}");
            }
            assert!(e3("../meetings", &codes(&["en"]), false).is_err());
            assert_eq!(store.get(&review).unwrap(), before);

            let recorded = f.recorded();
            let refused = e3(&recorded, &codes(&["en"]), false).unwrap_err();
            assert!(refused.contains("다시 전사할 수 없는 상태"), "{refused}");
            let sent = f.transcribed(false, None);
            store
                .update(&sent, |m| {
                    m.status = Status::Sent;
                    Ok(())
                })
                .unwrap();
            assert!(e3(&sent, &codes(&["en"]), false).is_err());

            // Running: E3 and a language change both wait for "일시정지".
            queue.enqueue(&recorded, Status::Recorded);
            f.wait("transcribing", || f.transcribing(&recorded));
            let running = e3(&recorded, &codes(&["en"]), false).unwrap_err();
            assert_eq!(running, "처리 중입니다. 먼저 일시정지해 주세요");
            let changed = queue.set_language(&store, &recorded, &codes(&["en"]));
            assert_eq!(changed.unwrap_err(), "처리 중에는 바꿀 수 없습니다");
            fs::write(
                store
                    .dir_for(&recorded)
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("go"),
                "",
            )
            .unwrap();
            f.wait("done", || queue.snapshot().is_empty());
            assert_eq!(store.get(&recorded).unwrap().languages, ["ko"]);
        }

        /// Plan 5.6-4, 5.6-5: the languages change while it records and stay through its end;
        /// once a chunk or the transcript exists, only E3 changes them.
        #[test]
        fn languages_change_until_a_chunk_is_done_and_survive_the_recording_end() {
            let f = fixture(RESULT, Backend::Cpu);
            let (queue, store) = (f.queue(), f.store());
            let id = store
                .create("", false, crate::transcribe::lang::default_list())
                .unwrap()
                .id;
            let meta = queue
                .set_language(&store, &id, &codes(&["vi", "ko"]))
                .unwrap();
            assert_eq!(
                (meta.status, meta.languages),
                (Status::Recording, codes(&["ko", "vi"]))
            );
            store.finish_recording(&id, 1_000, vec![]).unwrap();
            assert_eq!(store.get(&id).unwrap().languages, ["ko", "vi"]);
            for bad in [vec![], codes(&["ko", "ko"]), codes(&["xx"])] {
                assert!(queue.set_language(&store, &id, &bad).is_err(), "{bad:?}");
            }
            assert!(queue.set_language(&store, "../x", &codes(&["ko"])).is_err());

            store
                .update(&id, |m| {
                    m.chunks_done = 1;
                    Ok(())
                })
                .unwrap();
            let refused = queue
                .set_language(&store, &id, &codes(&["en"]))
                .unwrap_err();
            assert_eq!(refused, TRANSCRIBED);
            let id = f.transcribed(false, None);
            let refused = queue
                .set_language(&store, &id, &codes(&["en"]))
                .unwrap_err();
            assert_eq!(refused, TRANSCRIBED);
        }

        /// Plan 5.6-14: without the detection model a Korean meeting runs and a mixed one waits
        /// in line, never failing; once the model arrives it runs with `--lid-model`.
        #[cfg(unix)]
        #[test]
        fn a_mixed_meeting_waits_for_the_detection_model_and_a_korean_one_does_not() {
            let f = fixture(RESULT, Backend::Cpu);
            let models = f.app.state::<Models>().dir.clone();
            let medium = model::needed("medium-q5_0").unwrap()[0];
            let sparse = |file: &str, size: u64| {
                fs::File::create(models.join(file))
                    .unwrap()
                    .set_len(size)
                    .unwrap();
            };
            sparse(medium.file, medium.size);
            fs::remove_file(models.join(model::lid().file)).unwrap();
            let settings = Settings {
                model: "medium-q5_0".into(),
                backend: Backend::Cpu,
                ..Settings::default()
            };
            f.app.state::<SettingsStore>().save(settings).unwrap();
            let mixed = f.recorded();
            f.queue()
                .set_language(&f.store(), &mixed, &codes(&["ko", "vi"]))
                .unwrap();
            let korean = f.recorded();
            f.queue().enqueue_pending();
            f.wait("korean", || f.status(&korean) == Status::Review);
            assert_eq!(f.status(&mixed), Status::Recorded);
            let waiting = f.queue().snapshot();
            assert!(
                waiting
                    .iter()
                    .any(|p| p.id == mixed && p.position == Some(1))
            );
            assert!(!header(&f, &korean).contains("--lid-model"));

            sparse(model::lid().file, model::lid().size);
            f.queue().pump();
            f.wait("mixed", || f.status(&mixed) == Status::Review);
            let header = header(&f, &mixed);
            let lid = format!("--lid-model {}", models.join(model::lid().file).display());
            assert!(
                header.contains("--lang ko,vi ") && header.contains(&lid),
                "{header}"
            );
        }

        /// Plan 5.6-14: a meeting given a second language while in line waits for the detection
        /// model too: it never starts, not even to be put back.
        #[cfg(unix)]
        #[test]
        fn a_meeting_given_a_second_language_in_line_waits_for_the_detection_model() {
            let f = fixture(&format!("{GATED}{RESULT}"), Backend::Cpu);
            let models = f.app.state::<Models>().dir.clone();
            let medium = model::needed("medium-q5_0").unwrap()[0];
            fs::File::create(models.join(medium.file))
                .unwrap()
                .set_len(medium.size)
                .unwrap();
            fs::remove_file(models.join(model::lid().file)).unwrap();
            let settings = Settings {
                model: "medium-q5_0".into(),
                backend: Backend::Cpu,
                ..Settings::default()
            };
            f.app.state::<SettingsStore>().save(settings).unwrap();
            // Both slots taken, so the next one waits in line.
            let first = [f.recorded(), f.recorded()];
            f.queue().enqueue_pending();
            f.wait("first running", || {
                first.iter().all(|id| f.transcribing(id))
            });
            let next = f.recorded();
            f.queue().enqueue_pending();
            f.queue()
                .set_language(&f.store(), &next, &codes(&["ko", "vi"]))
                .unwrap();
            let dir = f.store().dir_for(&next).unwrap();
            fs::write(dir.parent().unwrap().join("go"), "").unwrap();
            f.wait("first done", || {
                first.iter().all(|id| f.status(id) == Status::Review)
            });
            thread::sleep(Duration::from_millis(200));
            assert_eq!(f.status(&next), Status::Recorded);
            let events = f.events.lock().unwrap();
            let next: Vec<_> = events.iter().filter(|e| e["id"] == next).collect();
            assert!(!next.is_empty() && next.iter().all(|e| e["position"] == 1));
        }

        /// Plan 5.6-7: a leftover that will not go fails the run with its cause, never silently,
        /// and "다시 시도" clears again before it transcribes.
        #[cfg(unix)]
        #[test]
        fn a_leftover_that_will_not_go_fails_the_run_and_its_retry_clears_it() {
            let f = fixture(RESULT, Backend::Cpu);
            let id = f.recorded();
            let dir = f.store().dir_for(&id).unwrap();
            // A folder by a result's name: deleting it as a file fails.
            fs::create_dir(dir.join("summary.md")).unwrap();
            f.queue().enqueue_pending();
            f.wait("failed", || f.status(&id) == Status::TranscribeFailed);
            let error = f.store().get(&id).unwrap().error.unwrap();
            assert!(error.starts_with("summary.md"), "{error}");

            fs::remove_dir(dir.join("summary.md")).unwrap();
            fs::write(dir.join("summary.md"), "옛 요약").unwrap();
            f.queue().resume(&f.store(), &id).unwrap();
            f.wait("done", || f.status(&id) == Status::Review);
            assert!(!dir.join("summary.md").exists());
        }

        /// Plan 5.6-7: a `recorded` meeting starts without what another run left: the partial
        /// transcript is not resumed and the edited summary is gone.
        #[cfg(unix)]
        #[test]
        fn a_recorded_meeting_starts_without_what_another_run_left() {
            let f = fixture(RESULT, Backend::Cpu);
            let id = f.recorded();
            let dir = f.store().dir_for(&id).unwrap();
            fs::write(dir.join("chunks.json"), PLAN).unwrap();
            let old = r#"{"chunk":0,"start_ms":0,"end_ms":10000,"segments":[{"start_ms":0,"end_ms":10000,"text":"옛 언어"}],"languages":["ko"]}"#;
            fs::write(dir.join(PARTIAL), format!("{old}\n")).unwrap();
            fs::write(dir.join("summary.md"), "옛 요약").unwrap();
            f.queue().enqueue_pending();
            f.wait("done", || f.queue().snapshot().is_empty());
            assert_eq!(f.runs(&id), ["cpu"]);
            assert_eq!(f.transcript(&id)["segments"][0]["text"], "구간 0");
            assert!(!dir.join("summary.md").exists());
        }

        /// Plan 5.6-27: the worker's detection shows on the meeting's progress while it lasts.
        #[cfg(unix)]
        #[test]
        fn detection_shows_on_the_progress_while_it_lasts() {
            let script = format!(
                r#"echo '{{"type":"detecting","on":true}}'; {GATED}echo '{{"type":"detecting","on":false}}'; {RESULT}"#
            );
            let f = fixture(&script, Backend::Cpu);
            let id = f.recorded();
            f.queue().enqueue_pending();
            let detecting = || {
                f.queue()
                    .snapshot()
                    .iter()
                    .any(|p| p.id == id && p.detecting)
            };
            f.wait("detecting", detecting);
            let dir = f.store().dir_for(&id).unwrap();
            fs::write(dir.parent().unwrap().join("go"), "").unwrap();
            f.wait("done", || f.queue().snapshot().is_empty());
            let events = f.events.lock().unwrap();
            let last = events.last().unwrap();
            assert_eq!(
                (&last["status"], &last["detecting"]),
                (&serde_json::json!("review"), &serde_json::json!(false))
            );
        }

        /// Plan 5.7-7: a list on disk that is not valid is written back as Korean and runs so,
        /// instead of failing again at every start.
        #[cfg(unix)]
        #[test]
        fn languages_that_are_not_valid_on_disk_run_as_korean() {
            let f = fixture(RESULT, Backend::Cpu);
            let id = f.recorded();
            f.store()
                .update(&id, |m| {
                    m.languages = codes(&["ko", "xx"]);
                    Ok(())
                })
                .unwrap();
            f.queue().enqueue_pending();
            f.wait("done", || f.queue().snapshot().is_empty());
            let meta = f.store().get(&id).unwrap();
            assert_eq!(
                (meta.status, meta.languages),
                (Status::Review, codes(&["ko"]))
            );
            let header = header(&f, &id);
            assert!(
                header.contains("--lang ko ") && !header.contains("--lid-model"),
                "{header}"
            );
        }

        /// Plan 5.6-6, 5.7-1: the line read at the same time never runs the meeting before its
        /// old results are gone, nor twice.
        #[cfg(unix)]
        #[test]
        fn e3_beside_the_line_runs_the_meeting_once() {
            let f = fixture(RESULT, Backend::Cpu);
            let id = f.transcribed(false, None);
            f.store().set_status(&id, Status::Summarizing).unwrap();
            f.store().set_status(&id, Status::Review).unwrap();
            let app = f.app.handle().clone();
            thread::scope(|s| {
                s.spawn(move || {
                    for _ in 0..50 {
                        app.state::<Mock>().enqueue_pending();
                    }
                });
                f.queue()
                    .retranscribe(&f.store(), &id, &codes(&["en"]), || false)
                    .unwrap();
            });
            f.wait("done", || f.queue().snapshot().is_empty());
            assert_eq!(f.runs(&id), ["cpu"]);
            let meta = f.store().get(&id).unwrap();
            assert_eq!(
                (meta.status, meta.languages),
                (Status::Review, codes(&["en"]))
            );
        }
    }
}
