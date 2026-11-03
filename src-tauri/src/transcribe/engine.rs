//! The transcription worker (architecture 5.2–5.4, eng review D5): the app runs its own executable
//! with `--plan-chunks` or `--transcribe-worker`, and only that process ever calls whisper.cpp.
//! A native crash ends the worker, never the app or a recording.
//!
//! The worker prints one JSON object per line on stdout (`Line`) and exits with `0` (done),
//! `EXIT_INPUT`, `EXIT_MODEL`, or anything else for a crash. Messages for the recorder go to stderr,
//! which the app keeps as the meeting's `worker.log`. It exits without running destructors
//! (`exit_now`), so a GPU backend crashing in teardown cannot undo a result already printed.

use crate::audio::mix;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, OsStr, OsString};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperVadContext,
    WhisperVadContextParams, WhisperVadParams,
};

/// Bad arguments or unreadable audio.
pub const EXIT_INPUT: i32 = 2;
/// The model file would not load: damaged or missing. Never counts as a GPU failure.
pub const EXIT_MODEL: i32 = 3;
const EXIT_CRASH: i32 = 1;

/// TRN-03: chunks of about 10 minutes, cut in silence. A chunk is never shorter than
/// `MIN_CHUNK_MS` (except the last) nor longer than `MAX_CHUNK_MS`.
const TARGET_CHUNK_MS: u64 = 10 * 60_000;
const MIN_CHUNK_MS: u64 = 5 * 60_000;
const MAX_CHUNK_MS: u64 = 12 * 60_000;
/// A cut stays this far from speech when the silence allows it.
const CUT_MARGIN_MS: u64 = 500;
/// Speech detection reads 10 min windows that overlap by 30 s (architecture 5.2).
const VAD_WINDOW_MS: u64 = 10 * 60_000;
const VAD_OVERLAP_MS: u64 = 30_000;
const SAMPLES_PER_MS: u64 = 16;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    Gpu,
    Cpu,
}

impl Device {
    /// As on the command line and in `meta.json`'s `backend`.
    pub fn name(self) -> &'static str {
        match self {
            Device::Gpu => "gpu",
            Device::Cpu => "cpu",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Chunk {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Speech inside the chunk by VAD (TRN-07's empty-result check).
    pub speech_ms: u64,
}

/// Times are from the start of the recording.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// One stdout line of the worker.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Line {
    Progress {
        pct: u8,
    },
    /// `--plan-chunks`.
    Plan {
        len_ms: u64,
        chunks: Vec<Chunk>,
    },
    /// `--transcribe-worker`. `device` is what actually ran: a GPU that fails to start falls
    /// back to the CPU (TRN-05).
    Result {
        device: Device,
        segments: Vec<Segment>,
    },
    /// `--transcribe-worker`, once the model has loaded (plan 5.1-3): where it runs, and the GPU
    /// picked for it, empty when there is none.
    Device {
        device: Device,
        name: String,
    },
    /// `--transcribe-worker`, before its result (plan 5.1-4): a `diag_line`.
    Diag {
        line: String,
    },
}

/// A measurement line (plan 5.1-4): `kind`, then `key=value` pairs. A value with whitespace or a
/// `"` goes in double quotes, inside which `"` and `\` are escaped with a backslash.
pub(crate) fn diag_line(kind: &str, pairs: &[(&str, String)]) -> String {
    let mut line = kind.to_string();
    for (key, value) in pairs {
        line += &format!(" {key}=");
        if value.contains(|c: char| c.is_whitespace() || c == '"') {
            line.push('"');
            for c in value.chars() {
                if matches!(c, '"' | '\\') {
                    line.push('\\');
                }
                line.push(c);
            }
            line.push('"');
        } else {
            line += value;
        }
    }
    line
}

/// The reading side of `diag_line`: the kind and the pairs, up to anything malformed.
pub(super) fn read_diag(line: &str) -> (&str, Vec<(String, String)>) {
    let (kind, mut rest) = line.split_once(' ').unwrap_or((line, ""));
    let mut pairs = Vec::new();
    while let Some((key, after)) = rest.split_once('=') {
        let (value, left) = match after.strip_prefix('"') {
            Some(quoted) => {
                let (mut value, mut chars) = (String::new(), quoted.char_indices());
                let end = loop {
                    match chars.next() {
                        Some((_, '\\')) => value.extend(chars.next().map(|(_, c)| c)),
                        Some((i, '"')) => break Some(i + 1),
                        Some((_, c)) => value.push(c),
                        None => break None,
                    }
                };
                let Some(end) = end else { break };
                (value, &quoted[end..])
            }
            None => {
                let end = after.find(' ').unwrap_or(after.len());
                (after[..end].to_string(), &after[end..])
            }
        };
        pairs.push((key.to_string(), value));
        rest = left.strip_prefix(' ').unwrap_or(left);
    }
    (kind, pairs)
}

/// Union of the speech spans found by overlapping windows: speech seen by two windows, or cut
/// off at one window's end, becomes one span.
pub fn merge(mut spans: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    spans.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// TRN-03. `speech` is sorted and merged. Each chunk ends at the silent point nearest
/// 10 minutes in, between 5 and 12 minutes; with no silence there, at 10 minutes.
pub fn plan(speech: &[(u64, u64)], len_ms: u64) -> Vec<Chunk> {
    let mut gaps = Vec::new();
    let mut prev = 0;
    for &(start, end) in speech {
        if start > prev {
            gaps.push((prev, start));
        }
        prev = prev.max(end);
    }
    if len_ms > prev {
        gaps.push((prev, len_ms));
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < len_ms {
        let target = start + TARGET_CHUNK_MS;
        let end = if len_ms <= target {
            len_ms
        } else {
            let (lo, hi) = (start + MIN_CHUNK_MS, start + MAX_CHUNK_MS);
            gaps.iter()
                .map(|&(a, b)| {
                    // The point in this silence nearest the target, kept off the speech edges.
                    let margin = CUT_MARGIN_MS.min((b - a) / 2);
                    target.clamp(a + margin, b - margin)
                })
                .filter(|cut| (lo..=hi).contains(cut))
                .min_by_key(|cut| cut.abs_diff(target))
                .unwrap_or(target)
        };
        let speech_ms = speech
            .iter()
            .map(|&(a, b)| b.min(end).saturating_sub(a.max(start)))
            .sum();
        chunks.push(Chunk {
            start_ms: start,
            end_ms: end,
            speech_ms,
        });
        start = end;
    }
    chunks
}

pub(super) enum Fail {
    Input(String),
    Model(String),
    Crash(String),
}

/// One result line; the LLM supervisor (`sidecar`) writes its lines the same way.
pub(crate) fn print(line: &impl Serialize) {
    // Stdout is line buffered: each line reaches the app as soon as it is printed.
    println!(
        "{}",
        serde_json::to_string(line).expect("worker line serializes")
    );
}

/// A child's stdout to its end, line by line: `Some` for each of our JSON lines, `None` for
/// anything else (a native library printing, bad UTF-8), which is skipped instead of ending the
/// read. Bytes, not text, for that reason. The queue reads workers and the sidecar its supervisor
/// with it.
pub(crate) fn read_lines<T: DeserializeOwned>(stdout: impl Read, mut each: impl FnMut(Option<T>)) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
        each(serde_json::from_slice(&line).ok());
        line.clear();
    }
}

/// Prints `pct` when it changes.
fn progress() -> impl FnMut(u8) {
    let mut last = None;
    move |pct| {
        if last != Some(pct) {
            last = Some(pct);
            print(&Line::Progress { pct });
        }
    }
}

/// Two meetings may transcribe at once (TRN-01), so each worker takes half the logical CPUs:
/// the physical cores on SMT machines, half the cores on Apple Silicon. Tune after S2.
pub(crate) fn threads() -> i32 {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1) as i32)
}

/// The devices whisper.cpp numbers `gpu_device` over (`whisper_backend_init_gpu`): ggml's GPUs
/// and integrated GPUs in registration order, as (integrated, description).
fn gpus() -> Vec<(bool, String)> {
    use whisper_rs_sys as sys;
    // SAFETY: ggml's device registry lives as long as the process, and a device's description is
    // a C string it owns.
    unsafe {
        (0..sys::ggml_backend_dev_count())
            .map(|i| sys::ggml_backend_dev_get(i))
            .filter_map(|dev| {
                let integrated = match sys::ggml_backend_dev_type(dev) {
                    sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU => false,
                    sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU => true,
                    _ => return None,
                };
                let name = CStr::from_ptr(sys::ggml_backend_dev_description(dev));
                Some((integrated, name.to_string_lossy().into_owned()))
            })
            .collect()
    }
}

/// Plan 5.1-8: the `gpu_device`s to load the model on, in order, over `gpus()`' kinds. The first
/// discrete GPU, else the first integrated one, or `forced`; one other than whisper.cpp's default
/// (0) falls back to 0 once. The CPU comes after these. `None`: `forced` names no GPU.
fn gpu_order(integrated: &[bool], forced: Option<usize>) -> Option<Vec<usize>> {
    let first = match forced {
        Some(n) if n >= integrated.len() => return None,
        Some(n) => n,
        None if integrated.is_empty() => return Some(Vec::new()),
        None => integrated.iter().position(|i| !i).unwrap_or(0),
    };
    Some(if first == 0 { vec![0] } else { vec![first, 0] })
}

/// The diagnostic's `power` and `saver` (plan 5.1-4): on AC or battery, battery saver on or off.
fn power() -> (&'static str, &'static str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: fills `status` and nothing else.
        if unsafe { GetSystemPowerStatus(&mut status) } != 0 {
            let power = match status.ACLineStatus {
                0 => "battery",
                1 => "ac",
                _ => "?",
            };
            let saver = match status.SystemStatusFlag {
                0 => "0",
                1 => "1",
                _ => "?",
            };
            return (power, saver);
        }
    }
    ("?", "?")
}

/// `ok` or `err:<code>` once a worker opted out of EcoQoS (plan 5.1-7); never set off Windows.
static ECOQOS: OnceLock<String> = OnceLock::new();

/// whisper.cpp takes model paths as C strings: they must be UTF-8 without NUL.
fn path_str(path: &OsStr) -> Result<&str, Fail> {
    match path.to_str() {
        Some(p) if !p.contains('\0') && Path::new(p).is_file() => Ok(p),
        _ => Err(Fail::Model(format!(
            "모델 파일이 없습니다: {}",
            path.to_string_lossy()
        ))),
    }
}

/// `--plan-chunks <meeting dir> <VAD model>`: speech by VAD over the mixed recording, read in
/// overlapping windows, then the chunk plan.
fn plan_chunks(args: &[OsString]) -> Result<(), Fail> {
    let [dir, vad_model] = args else {
        return Err(Fail::Input(
            "사용법: --plan-chunks <회의 폴더> <VAD 모델>".into(),
        ));
    };
    let dir = Path::new(dir);
    let len = mix::recording_len(dir).map_err(Fail::Input)?;
    let mut vad = WhisperVadContext::new(path_str(vad_model)?, WhisperVadContextParams::default())
        .map_err(|e| Fail::Model(format!("VAD 모델을 불러올 수 없습니다: {e}")))?;

    let window = VAD_WINDOW_MS * SAMPLES_PER_MS;
    let step = (VAD_WINDOW_MS - VAD_OVERLAP_MS) * SAMPLES_PER_MS;
    let mut report = progress();
    let mut spans = Vec::new();
    let mut start = 0;
    while start < len {
        let samples = mix::mixed(dir, start, window as usize).map_err(Fail::Input)?;
        let start_ms = start / SAMPLES_PER_MS;
        let found = vad
            .segments_from_samples(WhisperVadParams::default(), &samples)
            .map_err(|e| Fail::Crash(format!("음성 구간을 찾지 못했습니다: {e}")))?;
        // Timestamps are in centiseconds.
        spans.extend(found.map(|s| {
            (
                start_ms + (s.start.max(0.0) * 10.0) as u64,
                start_ms + (s.end.max(0.0) * 10.0) as u64,
            )
        }));
        report(((start + window).min(len) * 100 / len) as u8);
        if start + window >= len {
            break;
        }
        start += step;
    }
    let len_ms = len / SAMPLES_PER_MS;
    print(&Line::Plan {
        len_ms,
        chunks: plan(&merge(spans), len_ms),
    });
    Ok(())
}

/// After each speech span but the last, as whisper.cpp does: 0.1 s more of the audio, then 0.1 s
/// of silence.
const SPEECH_OVERLAP: usize = 1_600;
const SPEECH_GAP: usize = 1_600;

/// TRN-04's VAD step, done here. whisper.cpp cuts the audio down to speech only inside
/// `whisper_full`; whisper-rs 0.16's `WhisperState::full` calls `whisper_full_with_state`, which
/// skips that step, so `enable_vad` alone would transcribe silence and noise too (and does: tiny
/// on noise says "MBC 뉴스 …입니다"). This is the same cut: speech spans joined with the overlap
/// and gap above, and a way back to chunk time.
struct Speech {
    samples: Vec<f32>,
    /// (start in `samples`, start in the chunk, length), in samples.
    pieces: Vec<(usize, usize, usize)>,
}

impl Speech {
    fn new(audio: &[f32], spans: &[(usize, usize)]) -> Self {
        let mut speech = Speech {
            samples: Vec::new(),
            pieces: Vec::new(),
        };
        for (i, &(start, end)) in spans.iter().enumerate() {
            let end = if i + 1 < spans.len() {
                end + SPEECH_OVERLAP
            } else {
                end
            }
            .min(audio.len());
            if start >= end {
                continue;
            }
            if !speech.pieces.is_empty() {
                speech
                    .samples
                    .resize(speech.samples.len() + SPEECH_GAP, 0.0);
            }
            speech
                .pieces
                .push((speech.samples.len(), start, end - start));
            speech.samples.extend_from_slice(&audio[start..end]);
        }
        speech
    }

    /// A position in `samples` back in the chunk: one to one inside a piece. In a gap (or past
    /// the end), a segment's start moves on to the next piece and its end back to the piece
    /// before, so no segment stretches over the silence that was cut out.
    fn to_chunk(&self, at: usize, is_start: bool) -> usize {
        let after = self.pieces.partition_point(|&(joined, _, _)| joined <= at);
        let Some((joined, start, len)) = after.checked_sub(1).map(|i| self.pieces[i]) else {
            return self.pieces.first().map_or(0, |p| p.1);
        };
        if at < joined + len {
            return start + (at - joined);
        }
        match self.pieces.get(after) {
            Some(&(_, next, _)) if is_start => next,
            _ => start + len,
        }
    }
}

fn cs_to_samples(cs: f32) -> usize {
    (cs.max(0.0) * 160.0) as usize
}

const USAGE: &str = "사용법: --transcribe-worker <회의 폴더> <시작 ms> <끝 ms> <gpu|cpu> <모델> <VAD 모델> [--lang <코드>] [--beam <1..8>] [--threads <n>] [--fa <0|1>] [--gpu-device <n>]";

/// The flags after the positional arguments (plan 5.1-1), each at most once and with its value.
#[derive(Debug, PartialEq)]
struct Options {
    lang: String,
    beam: i32,
    threads: i32,
    flash_attn: bool,
    /// `None`: picked by `gpu_order`.
    gpu_device: Option<usize>,
}

fn options(flags: &[OsString]) -> Result<Options, Fail> {
    let mut options = Options {
        lang: "ko".into(),
        beam: 5,
        threads: threads(),
        flash_attn: true,
        gpu_device: None,
    };
    let cpus = std::thread::available_parallelism().map_or(i32::MAX, |n| n.get() as i32);
    let mut seen = Vec::new();
    let mut flags = flags.iter();
    while let Some(flag) = flags.next() {
        let flag = flag.to_string_lossy();
        let bad = || Fail::Input(format!("잘못된 인자: {flag}. {USAGE}"));
        let value = flags.next().and_then(|v| v.to_str()).ok_or_else(bad)?;
        if seen.contains(&flag) {
            return Err(bad());
        }
        let number = |range: std::ops::RangeInclusive<i32>| {
            value
                .parse()
                .ok()
                .filter(|n| range.contains(n))
                .ok_or_else(bad)
        };
        match &*flag {
            "--lang" if !value.contains('\0') && whisper_rs::get_lang_id(value).is_some() => {
                options.lang = value.into();
            }
            "--beam" => options.beam = number(1..=8)?,
            "--threads" => options.threads = number(1..=cpus)?,
            "--fa" => options.flash_attn = number(0..=1)? == 1,
            // Checked against the GPUs found once they are counted (`gpu_order`).
            "--gpu-device" => options.gpu_device = Some(number(0..=i32::MAX)? as usize),
            _ => return Err(bad()),
        }
        seen.push(flag);
    }
    Ok(options)
}

/// `--transcribe-worker <meeting dir> <start ms> <end ms> <gpu|cpu> <model> <VAD model> [flags]`:
/// one chunk with the TRN-04 parameters, same as S2's `whisper-cli -l ko --vad -vm`, and the
/// plan 5.1 flags (`options`).
fn transcribe_chunk(args: &[OsString]) -> Result<(), Fail> {
    let (positional, flags) = args.split_at(args.len().min(6));
    let [dir, start, end, device, model, vad_model] = positional else {
        return Err(Fail::Input(USAGE.into()));
    };
    let options = options(flags)?;
    let number = |s: &OsStr| {
        s.to_str()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| Fail::Input(format!("잘못된 숫자: {}", s.to_string_lossy())))
    };
    let (start_ms, end_ms) = (number(start)?, number(end)?);
    let device = match device.to_str() {
        Some("gpu") => Device::Gpu,
        Some("cpu") => Device::Cpu,
        _ => {
            return Err(Fail::Input(format!(
                "잘못된 백엔드: {}",
                device.to_string_lossy()
            )));
        }
    };
    let (model, vad_model) = (path_str(model)?, path_str(vad_model)?);
    if end_ms <= start_ms {
        return Err(Fail::Input(format!("잘못된 구간: {start_ms}..{end_ms}")));
    }
    let audio = mix::mixed(
        Path::new(dir),
        start_ms * SAMPLES_PER_MS,
        ((end_ms - start_ms) * SAMPLES_PER_MS) as usize,
    )
    .map_err(Fail::Input)?;
    let mut vad = WhisperVadContext::new(vad_model, WhisperVadContextParams::default())
        .map_err(|e| Fail::Model(format!("VAD 모델을 불러올 수 없습니다: {e}")))?;
    let spans: Vec<(usize, usize)> = vad
        .segments_from_samples(WhisperVadParams::default(), &audio)
        .map_err(|e| Fail::Crash(format!("음성 구간을 찾지 못했습니다: {e}")))?
        .map(|s| (cs_to_samples(s.start), cs_to_samples(s.end)))
        .collect();
    let speech = Speech::new(&audio, &spans);
    drop(audio);
    let mut report = progress();
    report(0);
    if speech.samples.is_empty() {
        // No speech: nothing for Whisper, and no model to load.
        print(&Line::Result {
            device,
            segments: Vec::new(),
        });
        return Ok(());
    }

    let gpus = gpus();
    let integrated: Vec<bool> = gpus.iter().map(|g| g.0).collect();
    let order = gpu_order(&integrated, options.gpu_device).ok_or_else(|| {
        Fail::Input(format!(
            "GPU {}번이 없습니다 (찾은 GPU {}개). {USAGE}",
            options.gpu_device.unwrap_or_default(),
            gpus.len()
        ))
    })?;
    let load = |gpu: Option<usize>| {
        let mut params = WhisperContextParameters::default();
        params
            .use_gpu(gpu.is_some())
            .gpu_device(gpu.unwrap_or_default() as i32)
            .flash_attn(options.flash_attn);
        WhisperContext::new_with_params(model, params)
    };
    // TRN-05, plan 5.1-8: a GPU that will not start falls back to the other one, then to the CPU,
    // which is also where whisper.cpp runs when there is no GPU.
    let loading = Instant::now();
    let tries = if device == Device::Gpu {
        &order[..]
    } else {
        &[]
    };
    let mut loaded = None;
    for &gpu in tries {
        match load(Some(gpu)) {
            Ok(ctx) => {
                loaded = Some((ctx, Some(gpu)));
                break;
            }
            Err(e) => eprintln!("GPU {gpu}번으로 모델을 불러오지 못했습니다: {e}"),
        }
    }
    let (ctx, used) = match loaded {
        Some(loaded) => loaded,
        None => {
            if !tries.is_empty() {
                eprintln!("CPU로 바꿉니다");
            }
            (load(None).map_err(model_failed)?, None)
        }
    };
    let load_s = loading.elapsed().as_secs_f64();
    let device = if used.is_some() {
        Device::Gpu
    } else {
        Device::Cpu
    };
    let name = used
        .or(order.first().copied())
        .map_or("", |i| gpus[i].1.as_str());
    print(&Line::Device {
        device,
        name: name.into(),
    });
    let mut state = ctx.create_state().map_err(model_failed)?;

    // whisper-cli's default sampling (beam 5), so speed and accuracy match what S2 measures.
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: options.beam,
        patience: -1.0,
    });
    params.set_language(Some(&options.lang));
    params.set_n_threads(options.threads);
    params.set_no_context(true);
    params.set_suppress_nst(true);
    // Stdout carries the JSON lines only.
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_print_special(false);
    params.set_progress_callback_safe(move |pct: i32| report(pct.clamp(0, 100) as u8));

    let running = Instant::now();
    state
        .full(params, &speech.samples)
        .map_err(|e| Fail::Crash(format!("전사 실패: {e}")))?;
    let run_s = running.elapsed().as_secs_f64();
    let mut segments = Vec::new();
    for segment in state.as_iter() {
        let text = segment
            .to_str_lossy()
            .map_err(|e| Fail::Crash(e.to_string()))?;
        // Centiseconds into the speech-only audio, back to recording time. Whisper may run a
        // little past the audio.
        let at = |cs: i64, is_start: bool| {
            let sample = speech.to_chunk(cs_to_samples(cs as f32), is_start) as u64;
            (start_ms + sample / SAMPLES_PER_MS).min(end_ms)
        };
        let start = at(segment.start_timestamp(), true);
        segments.push(Segment {
            start_ms: start,
            // A segment wholly inside a gap would map its start forward and its end back.
            end_ms: at(segment.end_timestamp(), false).max(start),
            text: text.trim().to_string(),
        });
    }
    let audio_s = (end_ms - start_ms) as f64 / 1000.0;
    let (power, saver) = power();
    let line = diag_line(
        "chunk",
        &[
            ("device", device.name().into()),
            ("gpu", name.into()),
            ("gpus", gpus.len().to_string()),
            ("gpu_device", used.map_or("-".into(), |g| g.to_string())),
            ("threads", options.threads.to_string()),
            ("beam", options.beam.to_string()),
            ("fa", u8::from(options.flash_attn).to_string()),
            ("lang", options.lang.clone()),
            ("audio_s", format!("{audio_s:.1}")),
            (
                "speech_s",
                format!("{:.1}", speech.samples.len() as f64 / 16_000.0),
            ),
            ("load_s", format!("{load_s:.2}")),
            ("run_s", format!("{run_s:.2}")),
            ("rtf", format!("{:.3}", run_s / audio_s)),
            ("power", power.into()),
            ("saver", saver.into()),
            ("ecoqos", ECOQOS.get().map_or("-", String::as_str).into()),
        ],
    );
    print(&Line::Diag { line });
    print(&Line::Result { device, segments });
    Ok(())
}

fn model_failed(e: whisper_rs::WhisperError) -> Fail {
    Fail::Model(format!("모델을 불러올 수 없습니다: {e}"))
}

type Mode = fn(&[OsString]) -> Result<(), Fail>;

fn mode(args: &[OsString]) -> Option<Mode> {
    match args.first().and_then(|a| a.to_str()) {
        Some("--plan-chunks") => Some(plan_chunks),
        Some("--transcribe-worker") => Some(transcribe_chunk),
        Some("--transcribe-bench") => Some(super::bench::bench),
        _ => None,
    }
}

/// The exit code for how a mode ended. A failure's message goes to stderr (`worker.log`).
pub(super) fn code_of(result: Result<(), Fail>) -> i32 {
    let (code, message) = match result {
        Ok(()) => return 0,
        Err(Fail::Input(m)) => (EXIT_INPUT, m),
        Err(Fail::Model(m)) => (EXIT_MODEL, m),
        Err(Fail::Crash(m)) => (EXIT_CRASH, m),
    };
    eprintln!("{message}");
    code
}

/// Exits without destructors or atexit handlers: once the result line is out, a Metal or
/// Vulkan teardown crash must not turn it into a crash exit (and so into `gpu_failed`).
fn exit_now(code: i32) -> ! {
    let _ = std::io::stdout().flush();
    // SAFETY: `_exit` only ends the process; stdout was flushed above, stderr is unbuffered.
    #[cfg(target_os = "macos")]
    unsafe {
        libc::_exit(code)
    }
    // Plan 5.1-5: `std::process::exit` runs static destructors, which can crash in the Vulkan
    // teardown. SAFETY: ends this process at once, as `_exit` does above.
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        TerminateProcess(GetCurrentProcess(), code as u32);
    }
    #[cfg(not(target_os = "macos"))]
    std::process::exit(code)
}

/// Runs the worker and exits when this process was started as one; returns for the app.
pub fn worker_main() {
    run_as(std::env::args_os().skip(1).collect());
}

fn run_as(args: Vec<OsString>) {
    let Some(run) = mode(&args) else {
        return;
    };
    #[cfg(windows)]
    let _ = ECOQOS
        .set(crate::no_ecoqos(None).map_or_else(|code| format!("err:{code}"), |()| "ok".into()));
    // The app holds a worker's stdin open. If it goes away (a crash, a force quit), so does the
    // worker, instead of keeping the GPU busy next to the workers the restarted app starts. Not the
    // bench (plan 5.1-2): no app stands behind it, and a Windows GUI program has no stdin at all.
    if args[0] != "--transcribe-bench" {
        std::thread::spawn(|| {
            let _ = std::io::stdin().read(&mut [0; 1]);
            exit_now(EXIT_CRASH);
        });
    }
    exit_now(code_of(run(&args[1..])));
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    fn assert_bounded(chunks: &[Chunk], len_ms: u64) {
        assert_eq!(chunks.first().unwrap().start_ms, 0);
        assert_eq!(chunks.last().unwrap().end_ms, len_ms);
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].end_ms, pair[1].start_ms);
        }
        for c in &chunks[..chunks.len() - 1] {
            let len = c.end_ms - c.start_ms;
            assert!((5 * MIN..=12 * MIN).contains(&len), "{c:?}");
        }
    }

    #[test]
    fn merges_spans_seen_by_overlapping_windows() {
        let spans = vec![
            (590_000, 600_000),
            (100, 900),
            (595_000, 610_000),
            (1_000, 2_000),
        ];
        assert_eq!(
            merge(spans),
            [(100, 900), (1_000, 2_000), (590_000, 610_000)]
        );
    }

    #[test]
    fn cuts_in_the_silence_nearest_ten_minutes() {
        // 50 s of speech, 3 s of silence, for 35 minutes.
        let speech: Vec<(u64, u64)> = (0..)
            .map(|i| (i * 53_000, i * 53_000 + 50_000))
            .take_while(|&(_, end)| end < 35 * MIN)
            .collect();
        let chunks = plan(&speech, 35 * MIN);
        assert_bounded(&chunks, 35 * MIN);
        for c in &chunks[..chunks.len() - 1] {
            // In a gap, 500 ms clear of speech, and within a minute of the target.
            let gap = speech
                .windows(2)
                .find(|w| w[0].1 < c.end_ms && c.end_ms < w[1].0)
                .expect("cut is in silence");
            assert!(c.end_ms >= gap[0].1 + 500 && c.end_ms + 500 <= gap[1].0);
            assert!((c.end_ms - c.start_ms).abs_diff(10 * MIN) < MIN, "{c:?}");
        }
        let speech_total: u64 = chunks.iter().map(|c| c.speech_ms).sum();
        assert_eq!(speech_total, speech.iter().map(|(a, b)| b - a).sum::<u64>());
    }

    #[test]
    fn prefers_a_long_silence_before_the_target() {
        // Speech everywhere except 9:00–9:40.
        let speech = [(0, 9 * MIN), (9 * MIN + 40_000, 25 * MIN)];
        let chunks = plan(&speech, 25 * MIN);
        assert_eq!(chunks[0].end_ms, 9 * MIN + 39_500);
        assert_eq!(chunks[0].speech_ms, 9 * MIN);
        assert_bounded(&chunks, 25 * MIN);
    }

    #[test]
    fn continuous_speech_is_cut_at_ten_minutes_and_no_chunk_is_too_long() {
        let chunks = plan(&[(0, 25 * MIN)], 25 * MIN);
        let ends: Vec<u64> = chunks.iter().map(|c| c.end_ms).collect();
        assert_eq!(ends, [10 * MIN, 20 * MIN, 25 * MIN]);
        assert_bounded(&chunks, 25 * MIN);
    }

    #[test]
    fn silent_or_short_recordings() {
        let chunks = plan(&[], 21 * MIN);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.speech_ms == 0));
        assert_bounded(&chunks, 21 * MIN);

        let short = plan(&[(1_000, 5_000)], 9 * MIN);
        assert_eq!(
            short,
            [Chunk {
                start_ms: 0,
                end_ms: 9 * MIN,
                speech_ms: 4_000
            }]
        );
        assert!(plan(&[], 0).is_empty());
    }

    #[test]
    fn speech_is_joined_like_whisper_cpp_and_maps_back_to_chunk_time() {
        let audio: Vec<f32> = (0..20_000).map(|i| i as f32).collect();
        let speech = Speech::new(
            &audio,
            &[(1_000, 2_000), (10_000, 12_000), (19_990, 25_000)],
        );
        // 1,000 + 0.1 s overlap; 0.1 s gap; 2,000 + overlap; gap; the last, cut at the end.
        assert_eq!(
            speech.pieces,
            [
                (0, 1_000, 2_600),
                (4_200, 10_000, 3_600),
                (9_400, 19_990, 10)
            ]
        );
        assert_eq!(speech.samples.len(), 9_410);
        assert_eq!(
            (speech.samples[4_200], speech.samples[4_199]),
            (10_000.0, 0.0)
        );
        for is_start in [true, false] {
            assert_eq!(speech.to_chunk(0, is_start), 1_000);
            assert_eq!(speech.to_chunk(2_599, is_start), 3_599);
            assert_eq!(speech.to_chunk(4_200, is_start), 10_000);
            assert_eq!(speech.to_chunk(9_405, is_start), 19_995);
        }
        // In the gap after the first piece (chunk 3,600..10,000 was cut out): a start moves on to
        // the next speech, an end stays at the speech before. Never seconds early or late.
        assert_eq!(speech.to_chunk(3_000, true), 10_000);
        assert_eq!(speech.to_chunk(3_000, false), 3_600);
        // Past the end (Whisper overruns a little): both stay at the last speech.
        assert_eq!(speech.to_chunk(9_500, true), 20_000);
        assert_eq!(speech.to_chunk(9_500, false), 20_000);

        let none = Speech::new(&audio, &[]);
        assert!(none.samples.is_empty());
        assert_eq!(none.to_chunk(5, true), 0);
    }

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn bad_arguments_and_missing_files_get_the_contract_exit_codes() {
        let tmp = tempfile::tempdir().unwrap();
        let model = tmp.path().join("model.bin");
        std::fs::write(&model, b"not a model").unwrap();
        let model = model.to_str().unwrap();
        let dir = tmp.path().join("no-meeting");
        let dir = dir.to_str().unwrap();

        let worker = |args: &[&str]| {
            let args = os(args);
            code_of(mode(&args).expect("a worker mode")(&args[1..]))
        };
        assert!(mode(&os(&["--other"])).is_none() && mode(&[]).is_none());
        assert_eq!(worker(&["--transcribe-worker", "x"]), EXIT_INPUT);
        assert_eq!(worker(&["--plan-chunks"]), EXIT_INPUT);
        let chunk = |start: &str, device: &str, model: &str| {
            worker(&[
                "--transcribe-worker",
                dir,
                start,
                "1000",
                device,
                model,
                model,
            ])
        };
        assert_eq!(chunk("abc", "gpu", model), EXIT_INPUT);
        assert_eq!(chunk("0", "npu", model), EXIT_INPUT);
        assert_eq!(chunk("0", "cpu", "/no/such/model.bin"), EXIT_MODEL);
        // Arguments fine, but no recording there.
        assert_eq!(chunk("0", "cpu", model), EXIT_INPUT);
        assert_eq!(worker(&["--plan-chunks", dir, model]), EXIT_INPUT);

        // Plan 5.1-1: flags pass to the next check (the missing model); a bad one stops first.
        let flagged = |flags: &[&str]| {
            let mut args = vec!["--transcribe-worker", dir, "0", "1000", "cpu"];
            args.extend(["/no/such/model.bin", "/no/such/model.bin"]);
            args.extend(flags);
            worker(&args)
        };
        let all = ["--lang", "en", "--beam", "1", "--threads", "1", "--fa", "0"];
        assert_eq!(flagged(&all), EXIT_MODEL);
        assert_eq!(flagged(&["--gpu-device", "0"]), EXIT_MODEL);
        for bad in [
            &["--speed", "1"][..],
            &["--beam", "1", "--beam", "2"],
            &["--beam", "0"],
            &["--beam", "9"],
            &["--threads", "0"],
            &["--fa", "2"],
            &["--gpu-device", "-1"],
            &["--lang", "xx"],
            &["--lang"],
            &["--beam", "--fa", "1"],
        ] {
            assert_eq!(flagged(bad), EXIT_INPUT, "{bad:?}");
        }
    }

    /// Plan 5.1-2: the bench runs without a stdin (a Windows GUI program has none) and gets to its
    /// own argument check, where a worker would have exited at the closed stdin. This test binary
    /// is the bench, in a child whose stdin is null.
    #[test]
    fn the_bench_outlives_a_closed_stdin() {
        const CHILD: &str = "MS_BENCH_STDIN_CHILD";
        if std::env::var_os(CHILD).is_some() {
            run_as(os(&["--transcribe-bench", "/no/meeting", "--range", "5-1"]));
            unreachable!("the bench exits");
        }
        let name = "transcribe::engine::tests::the_bench_outlives_a_closed_stdin";
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env(CHILD, "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(EXIT_INPUT), "{stderr}");
        assert!(stderr.contains("잘못된 인자: --range"), "{stderr}");
    }

    #[test]
    fn flags_default_to_the_product_values() {
        let parsed = options(&[]).ok().unwrap();
        assert_eq!(
            parsed,
            Options {
                lang: "ko".into(),
                beam: 5,
                threads: threads(),
                flash_attn: true,
                gpu_device: None,
            }
        );
        let parsed = options(&os(&["--gpu-device", "1", "--lang", "vi"]))
            .ok()
            .unwrap();
        assert_eq!((parsed.gpu_device, parsed.lang.as_str()), (Some(1), "vi"));
    }

    /// Plan 5.1-8 over made-up device lists (`true`: integrated), in registration order.
    #[test]
    fn the_discrete_gpu_goes_first_and_another_pick_falls_back_to_the_default() {
        assert_eq!(gpu_order(&[], None), Some(vec![]));
        assert_eq!(gpu_order(&[true], None), Some(vec![0]));
        // A laptop whose integrated GPU registers first.
        assert_eq!(gpu_order(&[true, false], None), Some(vec![1, 0]));
        assert_eq!(gpu_order(&[false, true], None), Some(vec![0]));
        assert_eq!(gpu_order(&[true, true], None), Some(vec![0]));
        assert_eq!(gpu_order(&[true, false, false], None), Some(vec![1, 0]));
        // `--gpu-device` wins, still with the default behind it.
        assert_eq!(gpu_order(&[false, true], Some(1)), Some(vec![1, 0]));
        assert_eq!(gpu_order(&[true, false], Some(0)), Some(vec![0]));
        assert_eq!(gpu_order(&[true], Some(1)), None);
        assert_eq!(gpu_order(&[], Some(0)), None);
    }

    #[test]
    fn diag_values_with_spaces_or_quotes_are_quoted_and_read_back() {
        let pairs = [
            ("device", "gpu".to_string()),
            ("gpu", "NVIDIA GeForce RTX 3060".into()),
            ("gpus", "2".into()),
            ("odd", r#"a "b" \c"#.into()),
            ("path", r"C:\x".into()),
            ("empty", String::new()),
            ("rtf", "0.250".into()),
        ];
        let line = diag_line("chunk", &pairs);
        assert_eq!(
            line,
            r#"chunk device=gpu gpu="NVIDIA GeForce RTX 3060" gpus=2 odd="a \"b\" \\c" path=C:\x empty= rtf=0.250"#
        );
        let (kind, read) = read_diag(&line);
        assert_eq!(kind, "chunk");
        let expected: Vec<_> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        assert_eq!(read, expected);
    }

    #[test]
    fn worker_lines_roundtrip_as_documented() {
        let line = r#"{"type":"result","device":"gpu","segments":[{"start_ms":1,"end_ms":2,"text":"네"}]}"#;
        let parsed: Line = serde_json::from_str(line).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), line);
        let progress: Line = serde_json::from_str(r#"{"type":"progress","pct":42}"#).unwrap();
        assert_eq!(progress, Line::Progress { pct: 42 });
        for line in [
            r#"{"type":"device","device":"cpu","name":""}"#,
            r#"{"type":"diag","line":"chunk device=gpu gpu=\"Apple M2\""}"#,
        ] {
            let parsed: Line = serde_json::from_str(line).unwrap();
            assert_eq!(serde_json::to_string(&parsed).unwrap(), line);
        }
    }
}
