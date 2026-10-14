//! The transcription worker (architecture 5.2–5.4, eng review D5): the app runs its own executable
//! with `--plan-chunks` or `--transcribe-worker`, and only that process ever calls whisper.cpp.
//! A native crash ends the worker, never the app or a recording.
//!
//! The worker prints one JSON object per line on stdout (`Line`) and exits with `0` (done),
//! `EXIT_INPUT`, `EXIT_MODEL`, or anything else for a crash. Messages for the recorder go to stderr,
//! which the app keeps as the meeting's `worker.log`. It exits without running destructors
//! (`exit_now`), so a GPU backend crashing in teardown cannot undo a result already printed.

use crate::audio::mix;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::Path;
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

enum Fail {
    Input(String),
    Model(String),
    Crash(String),
}

fn print(line: &Line) {
    // Stdout is line buffered: each line reaches the app as soon as it is printed.
    println!(
        "{}",
        serde_json::to_string(line).expect("worker line serializes")
    );
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
fn threads() -> i32 {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1) as i32)
}

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

/// `--transcribe-worker <meeting dir> <start ms> <end ms> <gpu|cpu> <model> <VAD model>`: one
/// chunk with the TRN-04 parameters, same as S2's `whisper-cli -l ko --vad -vm`.
fn transcribe_chunk(args: &[OsString]) -> Result<(), Fail> {
    let [dir, start, end, device, model, vad_model] = args else {
        return Err(Fail::Input(
            "사용법: --transcribe-worker <회의 폴더> <시작 ms> <끝 ms> <gpu|cpu> <모델> <VAD 모델>"
                .into(),
        ));
    };
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

    let load = |gpu: bool| {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(gpu);
        WhisperContext::new_with_params(model, params)
    };
    // TRN-05: a GPU that will not start falls back to the CPU. whisper.cpp itself also runs on
    // the CPU when it finds no GPU device.
    let (ctx, device) = match device {
        Device::Gpu => match load(true) {
            Ok(ctx) => (ctx, Device::Gpu),
            Err(e) => {
                eprintln!("GPU로 모델을 불러오지 못해 CPU로 바꿉니다: {e}");
                (load(false).map_err(model_failed)?, Device::Cpu)
            }
        },
        Device::Cpu => (load(false).map_err(model_failed)?, Device::Cpu),
    };
    let mut state = ctx.create_state().map_err(model_failed)?;

    // whisper-cli's default sampling (beam 5), so speed and accuracy match what S2 measures.
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });
    params.set_language(Some("ko"));
    params.set_n_threads(threads());
    params.set_no_context(true);
    params.set_suppress_nst(true);
    // Stdout carries the JSON lines only.
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_print_special(false);
    params.set_progress_callback_safe(move |pct: i32| report(pct.clamp(0, 100) as u8));

    state
        .full(params, &speech.samples)
        .map_err(|e| Fail::Crash(format!("전사 실패: {e}")))?;
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
        _ => None,
    }
}

/// The exit code for how a mode ended. A failure's message goes to stderr (`worker.log`).
fn code_of(result: Result<(), Fail>) -> i32 {
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
    #[cfg(not(target_os = "macos"))]
    std::process::exit(code)
}

/// Runs the worker and exits when this process was started as one; returns for the app.
pub fn worker_main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(run) = mode(&args) else {
        return;
    };
    // The app holds our stdin open. If it goes away (a crash, a force quit), so do we, instead of
    // keeping the GPU busy next to the workers the restarted app starts.
    std::thread::spawn(|| {
        let _ = std::io::stdin().read(&mut [0; 1]);
        exit_now(EXIT_CRASH);
    });
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
    }

    #[test]
    fn worker_lines_roundtrip_as_documented() {
        let line = r#"{"type":"result","device":"gpu","segments":[{"start_ms":1,"end_ms":2,"text":"네"}]}"#;
        let parsed: Line = serde_json::from_str(line).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), line);
        let progress: Line = serde_json::from_str(r#"{"type":"progress","pct":42}"#).unwrap();
        assert_eq!(progress, Line::Progress { pct: 42 });
    }
}
