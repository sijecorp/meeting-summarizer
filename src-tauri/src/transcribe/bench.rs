//! The transcription bench (plan 5.1-14): `--transcribe-bench <meeting dir>` runs this executable
//! as `--transcribe-worker` over one range of a recorded meeting, once per combination of settings,
//! and appends a row per combination to the meeting's `bench.tsv`. It runs from the installed app,
//! so a laptop without a dev setup measures what the product does. The app never runs it.

use super::engine::{Chunk, Device, Fail, Line, read_diag, read_lines, threads};
use super::{filter, model};
use crate::queue::{STALL, last_log_line};
use serde::Deserialize;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const USAGE: &str = "사용법: --transcribe-bench <회의 폴더> [--reference <기준 전사본 txt>] [--range <시작ms>-<끝ms>] [--only <조합,...>] [--smoke] [--model <파일>] [--vad <파일>] [--lang <코드>]. 조합 이름은 <gpu|cpu>/<모델>/b<beam>/fa<0|1>/<half|cores>(예: gpu/large-v3-turbo-q8_0/b5/fa1/half)이고 bench.tsv 첫 열에 있다. half는 논리 CPU의 절반(제품 기본), cores는 물리 코어 수";

/// Without `--range`: the first 10 minutes.
const RANGE: (u64, u64) = (0, 10 * 60_000);
/// Compared when `--model` is not given, the product default first. Only installed ones run.
const MODELS: [&str; 2] = ["large-v3-turbo-q8_0", "medium-q5_0"];
/// The worker's copy of the meeting, in the temp folder: this, then the bench's pid.
const WORK: &str = "meeting-summarizer-bench-";
/// The installed app's data folder (`identifier` in tauri.conf.json).
const APP_ID: &str = "com.sije.meeting-summarizer";
/// bench.tsv's columns: the combination, the worker's `diag` keys (plan 5.1-4) with the times summed
/// over the range's chunks, then the bench's own.
const COLUMNS: [&str; 21] = [
    "combo",
    "model",
    "device",
    "gpu",
    "gpus",
    "gpu_device",
    "threads",
    "beam",
    "fa",
    "lang",
    "audio_s",
    "speech_s",
    "load_s",
    "run_s",
    "rtf",
    "wall_s",
    "cer_pct",
    "power",
    "saver",
    "ecoqos",
    "status",
];

#[derive(Debug, PartialEq)]
struct Args {
    dir: PathBuf,
    reference: Option<PathBuf>,
    range: (u64, u64),
    only: Option<Vec<String>>,
    smoke: bool,
    model: Option<PathBuf>,
    vad: Option<PathBuf>,
    lang: String,
}

/// Every flag at most once, as the worker's (`engine::options`).
fn parse(args: &[OsString]) -> Result<Args, Fail> {
    let usage = || Fail::Input(USAGE.into());
    let (dir, flags) = args.split_first().ok_or_else(usage)?;
    if dir.to_string_lossy().starts_with("--") {
        return Err(usage());
    }
    let mut parsed = Args {
        dir: dir.into(),
        reference: None,
        range: RANGE,
        only: None,
        smoke: false,
        model: None,
        vad: None,
        lang: "ko".into(),
    };
    let mut seen = Vec::new();
    let mut flags = flags.iter();
    while let Some(flag) = flags.next() {
        let flag = flag.to_string_lossy();
        let bad = || Fail::Input(format!("잘못된 인자: {flag}. {USAGE}"));
        if seen.contains(&flag) {
            return Err(bad());
        }
        let mut value = || flags.next().ok_or_else(bad);
        match &*flag {
            "--smoke" => parsed.smoke = true,
            "--reference" => parsed.reference = Some(value()?.into()),
            "--model" => parsed.model = Some(value()?.into()),
            "--vad" => parsed.vad = Some(value()?.into()),
            "--range" => parsed.range = value()?.to_str().and_then(range).ok_or_else(bad)?,
            "--only" => {
                let names = value()?.to_string_lossy();
                parsed.only = Some(names.split(',').map(str::to_string).collect());
            }
            "--lang" => {
                parsed.lang = value()?
                    .to_str()
                    .filter(|l| !l.contains('\0') && whisper_rs::get_lang_id(l).is_some())
                    .ok_or_else(bad)?
                    .into();
            }
            _ => return Err(bad()),
        }
        seen.push(flag);
    }
    Ok(parsed)
}

/// `<start ms>-<end ms>`, the start first.
fn range(value: &str) -> Option<(u64, u64)> {
    let (start, end) = value.split_once('-')?;
    let range = (start.parse().ok()?, end.parse().ok()?);
    (range.0 < range.1).then_some(range)
}

#[derive(Clone, Debug, PartialEq)]
struct Combo {
    /// `<device>/<model>/b<beam>/fa<0|1>/<half|cores>`, what `--only` takes.
    name: String,
    device: Device,
    /// (name, file)
    model: (String, PathBuf),
    beam: u8,
    fa: u8,
    threads: i32,
}

/// Every combination of the plan 5.1-14 axes: the product default first, then those one setting
/// away from it, then two, so an interrupted bench already has the comparisons that matter most.
fn combos(models: &[(String, PathBuf)], half: i32, cores: i32) -> Vec<Combo> {
    let mut threads = vec![("half", half)];
    if cores != half {
        threads.push(("cores", cores));
    }
    let mut all = Vec::new();
    for (d, device) in [Device::Gpu, Device::Cpu].into_iter().enumerate() {
        for (m, model) in models.iter().enumerate() {
            for (b, beam) in [5, 1].into_iter().enumerate() {
                for (f, fa) in [1, 0].into_iter().enumerate() {
                    for (t, &(label, n)) in threads.iter().enumerate() {
                        let away = [d, m, b, f, t].iter().filter(|&&i| i > 0).count();
                        let name = format!("{}/{}/b{beam}/fa{fa}/{label}", device.name(), model.0);
                        let combo = Combo {
                            name,
                            device,
                            model: model.clone(),
                            beam,
                            fa,
                            threads: n,
                        };
                        all.push((away, combo));
                    }
                }
            }
        }
    }
    all.sort_by_key(|(away, _)| *away);
    all.into_iter().map(|(_, combo)| combo).collect()
}

/// `--only` keeps the combinations it names, in bench order; `--smoke` the first, the product
/// default.
fn select(
    mut combos: Vec<Combo>,
    only: Option<&[String]>,
    smoke: bool,
) -> Result<Vec<Combo>, Fail> {
    if let Some(only) = only {
        if let Some(unknown) = only.iter().find(|n| !combos.iter().any(|c| &c.name == *n)) {
            return Err(Fail::Input(format!(
                "이 PC에 없는 조합: {unknown}. {USAGE}"
            )));
        }
        combos.retain(|c| only.contains(&c.name));
    }
    if smoke {
        combos.truncate(1);
    }
    Ok(combos)
}

/// The installed app's local data folder, where Tauri's `app_local_data_dir` puts it.
fn app_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| Path::new(&h).join("Library/Application Support"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share"));
    base.map(|base| base.join(APP_ID))
}

/// The models to compare, by name, and the VAD model: the arguments, else the installed app's.
fn models(args: &Args, app: Option<PathBuf>) -> Result<(Vec<(String, PathBuf)>, PathBuf), Fail> {
    let installed = app.map(|app| app.join("models"));
    let find = |file: &str| {
        installed
            .as_ref()
            .map(|dir| dir.join(file))
            .filter(|p| p.is_file())
    };
    let models: Vec<(String, PathBuf)> = match &args.model {
        Some(path) => {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let name = stem.strip_prefix("ggml-").unwrap_or(&stem).to_string();
            vec![(name, path.clone())]
        }
        None => MODELS
            .iter()
            .filter_map(|name| Some((name.to_string(), find(model::needed(name).ok()?[0].file)?)))
            .collect(),
    };
    match args.vad.clone().or_else(|| find(model::VAD.file)) {
        Some(vad)
            if vad.is_file() && !models.is_empty() && models.iter().all(|m| m.1.is_file()) =>
        {
            Ok((models, vad))
        }
        _ => Err(Fail::Model(format!(
            "모델 파일이 없습니다. 앱에서 전사 모델을 받거나 --model과 --vad로 줍니다 (찾은 곳: {})",
            installed.map_or("-".into(), |dir| dir.display().to_string())
        ))),
    }
}

/// The cores without SMT siblings, for the `cores` thread count.
#[cfg(windows)]
fn physical_cores() -> Option<usize> {
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, RelationProcessorCore,
        SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    let mut len = 0;
    // SAFETY: with no buffer it only writes the size it needs into `len`.
    unsafe {
        GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut len)
    };
    // u64s, so the records' 8-byte fields are aligned.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes.
    let filled = unsafe {
        GetLogicalProcessorInformationEx(RelationProcessorCore, buf.as_mut_ptr().cast(), &mut len)
    };
    if filled == 0 {
        return None;
    }
    let bytes: Vec<u8> = buf
        .iter()
        .flat_map(|w| w.to_ne_bytes())
        .take(len as usize)
        .collect();
    // One record per core, each of its own size.
    let size_at = std::mem::offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Size);
    let (mut at, mut cores) = (0, 0);
    while let Some(size) = bytes.get(at + size_at..at + size_at + 4) {
        let size = u32::from_ne_bytes(size.try_into().ok()?) as usize;
        if size == 0 {
            break;
        }
        at += size;
        cores += 1;
    }
    (cores > 0).then_some(cores)
}

/// The cores without SMT siblings, for the `cores` thread count.
#[cfg(target_os = "macos")]
fn physical_cores() -> Option<usize> {
    let mut cores: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: `hw.physicalcpu` is an int, written into `cores` and nothing else.
    let read = unsafe {
        libc::sysctlbyname(
            c"hw.physicalcpu".as_ptr(),
            (&raw mut cores).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    usize::try_from(cores).ok().filter(|&n| read == 0 && n > 0)
}

/// shortcut: no count off Windows and macOS, so `cores` is `half`; read sysfs if Linux ships.
#[cfg(not(any(windows, target_os = "macos")))]
fn physical_cores() -> Option<usize> {
    None
}

/// One worker run: what it printed, how it ended, how long it took, its last stderr line.
struct Run {
    lines: Vec<Line>,
    code: Option<i32>,
    /// Killed after `stall` without a line, as the queue does (`STALL`).
    stalled: bool,
    wall_s: f64,
    why: String,
}

fn spawn(exe: &Path, args: &[&OsStr], work: &Path, stall: Duration) -> Result<Run, Fail> {
    let log = work.join("worker.log");
    let stderr = File::create(&log)
        .map_err(|e| Fail::Crash(format!("worker.log를 만들 수 없습니다: {e}")))?;
    let started = Instant::now();
    let mut child = Command::new(exe)
        .args(args)
        // Held open and never written, as the queue does: a worker exits once its stdin closes.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .map_err(|e| Fail::Crash(format!("워커를 시작할 수 없습니다: {e}")))?;
    let mut lines = Vec::new();
    let stdout = child.stdout.take();
    let stalled = thread::scope(|s| {
        let (beat, beats) = mpsc::channel::<()>();
        let child = &mut child;
        let watch = s.spawn(move || {
            let timed_out = loop {
                if let Err(e) = beats.recv_timeout(stall) {
                    break e == RecvTimeoutError::Timeout;
                }
            };
            if timed_out {
                let _ = child.kill();
            }
            timed_out
        });
        if let Some(stdout) = stdout {
            read_lines(stdout, |line| {
                let _ = beat.send(());
                lines.extend(line);
            });
        }
        drop(beat);
        watch.join().unwrap_or(false)
    });
    let code = child
        .wait()
        .map_err(|e| Fail::Crash(format!("워커를 기다릴 수 없습니다: {e}")))?
        .code();
    Ok(Run {
        lines,
        code,
        stalled,
        wall_s: started.elapsed().as_secs_f64(),
        why: last_log_line(&log),
    })
}

#[derive(Default)]
struct Measured {
    /// Each run's `diag` pairs. A chunk without speech has none.
    diags: Vec<Vec<(String, String)>>,
    wall_s: f64,
    /// The filtered transcript, for CER.
    text: String,
    /// How the first run that ended without a result ended.
    failed: Option<String>,
}

/// One combination over the range, a worker per chunk as the queue runs them.
fn measure(
    exe: &Path,
    work: &Path,
    (vad, lang): (&Path, &str),
    combo: &Combo,
    pieces: &[(u64, u64)],
    stall: Duration,
) -> Result<Measured, Fail> {
    let mut measured = Measured::default();
    let (beam, fa, threads) = (
        combo.beam.to_string(),
        combo.fa.to_string(),
        combo.threads.to_string(),
    );
    for &(start, end) in pieces {
        let (start, end) = (start.to_string(), end.to_string());
        let args = [
            OsStr::new("--transcribe-worker"),
            work.as_os_str(),
            OsStr::new(&start),
            OsStr::new(&end),
            OsStr::new(combo.device.name()),
            combo.model.1.as_os_str(),
            vad.as_os_str(),
            OsStr::new("--lang"),
            OsStr::new(lang),
            OsStr::new("--beam"),
            OsStr::new(&beam),
            OsStr::new("--threads"),
            OsStr::new(&threads),
            OsStr::new("--fa"),
            OsStr::new(&fa),
        ];
        let run = spawn(exe, &args, work, stall)?;
        measured.wall_s += run.wall_s;
        let mut done = false;
        for line in run.lines {
            match line {
                Line::Diag { line } => measured.diags.push(read_diag(&line).1),
                Line::Result { segments, .. } => {
                    done = true;
                    for segment in filter::filter(segments) {
                        measured.text += &segment.text;
                        measured.text.push(' ');
                    }
                }
                _ => {}
            }
        }
        if !done || run.code != Some(0) {
            // It would fail the same way on the next chunk: only time would be lost.
            let code = run.code.map_or("-".into(), |c| c.to_string());
            let how = if run.stalled {
                "stalled".into()
            } else {
                format!("exit {code}")
            };
            measured.failed = Some(format!("{how}: {}", run.why));
            break;
        }
    }
    Ok(measured)
}

/// A bench.tsv row by `COLUMNS`. Without a `diag` line (a failure) a setting is the one asked for.
/// A failed combination has no times: the chunks before its failure are not the range.
fn row(combo: &Combo, lang: &str, audio_s: f64, measured: &Measured, cer: Option<f64>) -> String {
    let sum = |key: &str| -> f64 {
        let values = measured.diags.iter().flatten().filter(|(k, _)| k == key);
        values.filter_map(|(_, v)| v.parse::<f64>().ok()).sum()
    };
    let asked = [
        ("device", combo.device.name().to_string()),
        ("threads", combo.threads.to_string()),
        ("beam", combo.beam.to_string()),
        ("fa", combo.fa.to_string()),
        ("lang", lang.to_string()),
    ];
    let timed = measured.failed.is_none() && !measured.diags.is_empty();
    let values = COLUMNS.iter().map(|&column| {
        let value = match column {
            "combo" => combo.name.clone(),
            "model" => combo.model.0.clone(),
            "audio_s" => format!("{audio_s:.1}"),
            "speech_s" | "load_s" | "run_s" | "rtf" if !timed => String::new(),
            "speech_s" | "load_s" | "run_s" => format!("{:.2}", sum(column)),
            "rtf" => format!("{:.3}", sum("run_s") / audio_s),
            "wall_s" => format!("{:.1}", measured.wall_s),
            "cer_pct" => cer.map_or(String::new(), |c| format!("{:.2}", c * 100.0)),
            "status" => measured.failed.clone().unwrap_or_else(|| "ok".into()),
            key => {
                let last = measured.diags.last();
                last.and_then(|d| d.iter().find(|(k, _)| k == key))
                    .map(|(_, v)| v.as_str())
                    .or_else(|| {
                        asked
                            .iter()
                            .find(|(k, _)| *k == key)
                            .map(|(_, v)| v.as_str())
                    })
                    .unwrap_or_default()
                    .to_string()
            }
        };
        value.replace(['\t', '\n', '\r'], " ")
    });
    values.collect::<Vec<_>>().join("\t")
}

/// A reference made with 전사본 복사 has each line's time in front, `[mm:ss]`: not what was said.
fn spoken(reference: &str) -> String {
    let line = |line: &'_ str| -> String {
        let time = line.strip_prefix('[').and_then(|rest| rest.split_once(']'));
        match time {
            Some((time, said))
                if !time.is_empty() && time.chars().all(|c| c.is_ascii_digit() || c == ':') =>
            {
                said.into()
            }
            _ => line.into(),
        }
    };
    reference.lines().map(line).collect::<Vec<_>>().join("\n")
}

/// What CER compares (plan 5.1-14): NFC, so composed and decomposed Hangul or accents are the same
/// letter, and letters and digits only, so spacing and punctuation do not count.
fn cer_chars(text: &str) -> Vec<char> {
    let nfc = icu_normalizer::ComposingNormalizerBorrowed::new_nfc().normalize(text);
    nfc.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Character edit distance over the reference's length; `None` for a reference with no letters.
fn cer(reference: &str, hypothesis: &str) -> Option<f64> {
    let (reference, hypothesis) = (cer_chars(reference), cer_chars(hypothesis));
    if reference.is_empty() {
        return None;
    }
    // Levenshtein, one row at a time.
    let mut row: Vec<usize> = (0..=hypothesis.len()).collect();
    for (i, r) in reference.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, h) in hypothesis.iter().enumerate() {
            let next = (diagonal + usize::from(r != h))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    Some(row[hypothesis.len()] as f64 / reference.len() as f64)
}

/// What the bench should take before anything is measured: real-time factors guessed on the slow
/// side for a GPU and a laptop CPU, and a few seconds per worker to load the model.
fn estimate_s(combos: &[Combo], audio_s: f64, workers: usize) -> f64 {
    let rtf = |device: Device| if device == Device::Gpu { 0.2 } else { 1.0 };
    let load_s = 5.0 * workers as f64;
    combos
        .iter()
        .map(|c| audio_s * rtf(c.device) + load_s)
        .sum()
}

/// `--transcribe-bench`. Ends with bench.tsv's path on stderr; on Windows the file is all there is.
pub(super) fn bench(args: &[OsString]) -> Result<(), Fail> {
    let exe = std::env::current_exe()
        .map_err(|e| Fail::Crash(format!("실행 파일을 찾을 수 없습니다: {e}")))?;
    let tsv = run(args, &exe, app_dir())?;
    eprintln!("결과: {}", tsv.display());
    Ok(())
}

/// The bench with `exe` as the worker and `app` as the installed app's data folder.
fn run(args: &[OsString], exe: &Path, app: Option<PathBuf>) -> Result<PathBuf, Fail> {
    let args = parse(args)?;
    let reference = match &args.reference {
        Some(path) => Some(
            fs::read_to_string(path)
                .as_deref()
                .map(spoken)
                .map_err(|e| {
                    Fail::Input(format!(
                        "기준 전사본을 UTF-8 글로 읽을 수 없습니다 ({}): {e}",
                        path.display()
                    ))
                })?,
        ),
        None => None,
    };
    let (models, vad) = models(&args, app)?;
    let half = threads();
    let cores = physical_cores().map_or(half, |n| n as i32);
    let combos = select(
        combos(&models, half, cores),
        args.only.as_deref(),
        args.smoke,
    )?;
    // The worker reads a copy: the meeting folder gets bench.tsv and nothing else.
    let temp = std::env::temp_dir();
    remove_stale(&temp);
    let work = temp.join(format!("{WORK}{}", std::process::id()));
    let result = fs::create_dir_all(&work)
        .map_err(|e| Fail::Crash(format!("임시 폴더를 만들 수 없습니다: {e}")))
        .and_then(|()| run_in(&work, exe, &args, (&vad, reference.as_deref()), &combos));
    remove(&work);
    result
}

/// The copies of benches that were killed before they removed theirs. By age, not by pid (a pid
/// is reused): a day old, so a bench still running next to this one keeps its own.
fn remove_stale(temp: &Path) {
    let Ok(entries) = fs::read_dir(temp) else {
        return;
    };
    let day = Duration::from_secs(24 * 60 * 60);
    for entry in entries.flatten() {
        let age = entry
            .metadata()
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok()?.elapsed().ok());
        if entry.file_name().to_string_lossy().starts_with(WORK) && age.is_some_and(|a| a >= day) {
            remove(&entry.path());
        }
    }
}

fn remove(work: &Path) {
    if let Err(e) = fs::remove_dir_all(work) {
        eprintln!("임시 폴더를 지울 수 없습니다 ({}): {e}", work.display());
    }
}

fn run_in(
    work: &Path,
    exe: &Path,
    args: &Args,
    (vad, reference): (&Path, Option<&str>),
    combos: &[Combo],
) -> Result<PathBuf, Fail> {
    for name in ["mic.wav", "system.wav"] {
        let from = args.dir.join(name);
        if name == "mic.wav" || from.exists() {
            fs::copy(&from, work.join(name)).map_err(|e| {
                Fail::Input(format!(
                    "녹음 파일을 복사할 수 없습니다 ({}): {e}",
                    from.display()
                ))
            })?;
        }
    }

    // The meeting's own chunks when it was processed, so the bench cuts where the app did.
    #[derive(Deserialize)]
    struct Plan {
        chunks: Vec<Chunk>,
    }
    let saved = fs::read(args.dir.join("chunks.json")).ok();
    let chunks = match saved.and_then(|b| serde_json::from_slice::<Plan>(&b).ok()) {
        Some(plan) => plan.chunks,
        None => {
            eprintln!("구간을 나누는 중입니다");
            let plan = [
                OsStr::new("--plan-chunks"),
                work.as_os_str(),
                vad.as_os_str(),
            ];
            let run = spawn(exe, &plan, work, STALL)?;
            let chunks = run.lines.into_iter().find_map(|line| match line {
                Line::Plan { chunks, .. } => Some(chunks),
                _ => None,
            });
            chunks.ok_or_else(|| Fail::Crash(format!("구간을 나누지 못했습니다: {}", run.why)))?
        }
    };
    let (from, to) = args.range;
    let pieces: Vec<(u64, u64)> = chunks
        .iter()
        .map(|c| (c.start_ms.max(from), c.end_ms.min(to)))
        .filter(|(start, end)| start < end)
        .collect();
    if pieces.is_empty() {
        let len_ms = chunks.last().map_or(0, |c| c.end_ms);
        return Err(Fail::Input(format!(
            "구간 {from}-{to}ms가 녹음(길이 {len_ms}ms) 밖입니다"
        )));
    }
    let audio_s = pieces.iter().map(|(start, end)| end - start).sum::<u64>() as f64 / 1000.0;
    // The range as measured: a reference transcript covers this, not past the recording's end.
    let (from, to) = (pieces[0].0, pieces[pieces.len() - 1].1);

    let tsv = args.dir.join("bench.tsv");
    let mut out = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&tsv)
        .map_err(|e| Fail::Crash(format!("bench.tsv를 열 수 없습니다: {e}")))?;
    // A row at a time: whatever finished before an interruption stays.
    let mut write = |line: &str| {
        out.write_all(format!("{line}\n").as_bytes())
            .map_err(|e| Fail::Crash(format!("bench.tsv에 쓸 수 없습니다: {e}")))
    };
    let header = format!(
        "# meeting-summarizer {} --transcribe-bench, 구간 {from}-{to}ms({audio_s:.0}초), 조합 {}개, 예상 약 {:.0}분(GPU는 오디오 길이의 0.2배, CPU는 1배로 어림), 기준 전사본 {}",
        env!("CARGO_PKG_VERSION"),
        combos.len(),
        (estimate_s(combos, audio_s, pieces.len()) / 60.0).ceil(),
        if reference.is_some() {
            "있음"
        } else {
            "없음"
        },
    );
    write(&header)?;
    write(&COLUMNS.join("\t"))?;
    eprintln!("{header}\n결과 파일: {}", tsv.display());
    for (i, combo) in combos.iter().enumerate() {
        eprintln!("[{}/{}] {}", i + 1, combos.len(), combo.name);
        let measured = measure(exe, work, (vad, &args.lang), combo, &pieces, STALL)?;
        let cer = reference
            .filter(|_| measured.failed.is_none())
            .and_then(|r| cer(r, &measured.text));
        let line = row(combo, &args.lang, audio_s, &measured, cer);
        write(&line)?;
        eprintln!("{line}");
    }
    Ok(tsv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::engine::code_of;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn arguments_parse_with_defaults_and_bad_ones_get_the_usage() {
        let parsed = parse(&os(&["m"])).ok().unwrap();
        assert_eq!(
            parsed,
            Args {
                dir: "m".into(),
                reference: None,
                range: (0, 600_000),
                only: None,
                smoke: false,
                model: None,
                vad: None,
                lang: "ko".into(),
            }
        );
        let all = [
            "m",
            "--range",
            "60000-120000",
            "--only",
            "gpu/x/b5/fa1/half,cpu/x/b1/fa0/cores",
            "--smoke",
            "--lang",
            "vi",
            "--reference",
            "r.txt",
            "--model",
            "x.bin",
            "--vad",
            "v.bin",
        ];
        let parsed = parse(&os(&all)).ok().unwrap();
        assert_eq!(
            parsed,
            Args {
                dir: "m".into(),
                reference: Some("r.txt".into()),
                range: (60_000, 120_000),
                only: Some(vec![
                    "gpu/x/b5/fa1/half".into(),
                    "cpu/x/b1/fa0/cores".into()
                ]),
                smoke: true,
                model: Some("x.bin".into()),
                vad: Some("v.bin".into()),
                lang: "vi".into(),
            }
        );
        for bad in [
            &[][..],
            &["--smoke"],
            &["m", "--speed", "1"],
            &["m", "--smoke", "--smoke"],
            &["m", "--range", "5-1"],
            &["m", "--range", "5-5"],
            &["m", "--range", "5"],
            &["m", "--range", "a-9"],
            &["m", "--range"],
            &["m", "--lang", "xx"],
            &["m", "--model"],
        ] {
            assert!(matches!(parse(&os(bad)), Err(Fail::Input(_))), "{bad:?}");
        }
    }

    #[test]
    fn the_product_default_runs_first_then_one_change_at_a_time() {
        let models = vec![
            ("turbo".to_string(), PathBuf::from("t.bin")),
            ("medium".to_string(), PathBuf::from("m.bin")),
        ];
        let all = combos(&models, 4, 8);
        assert_eq!(all.len(), 32);
        let mut names: Vec<&str> = all.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names[0], "gpu/turbo/b5/fa1/half");
        assert_eq!(
            (all[0].device, all[0].beam, all[0].fa, all[0].threads),
            (Device::Gpu, 5, 1, 4)
        );
        let mut one_away = names[1..6].to_vec();
        one_away.sort_unstable();
        assert_eq!(
            one_away,
            [
                "cpu/turbo/b5/fa1/half",
                "gpu/medium/b5/fa1/half",
                "gpu/turbo/b1/fa1/half",
                "gpu/turbo/b5/fa0/half",
                "gpu/turbo/b5/fa1/cores",
            ]
        );
        assert_eq!(names[31], "cpu/medium/b1/fa0/cores");
        let cores = all.iter().find(|c| c.name == "gpu/turbo/b5/fa1/cores");
        assert_eq!(cores.map(|c| c.threads), Some(8));
        names.dedup();
        assert_eq!(names.len(), 32);
        // A machine whose physical cores are half its CPUs has no second thread count.
        assert_eq!(combos(&models[..1], 4, 4).len(), 8);

        let smoke = select(all.clone(), None, true).ok().unwrap();
        assert_eq!(smoke, all[..1]);
        let only = [
            "cpu/medium/b1/fa0/cores".to_string(),
            "gpu/turbo/b5/fa1/half".to_string(),
        ];
        let picked = select(all.clone(), Some(&only), false).ok().unwrap();
        let picked: Vec<&str> = picked.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(picked, ["gpu/turbo/b5/fa1/half", "cpu/medium/b1/fa0/cores"]);
        let typo = ["gpu/turbo/b5/fa1/all".to_string()];
        assert!(matches!(
            select(all, Some(&typo), false),
            Err(Fail::Input(_))
        ));
    }

    #[test]
    fn models_default_to_the_installed_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("models");
        fs::create_dir(&dir).unwrap();
        // turbo is not installed.
        for file in ["ggml-medium-q5_0.bin", model::VAD.file] {
            fs::write(dir.join(file), b"m").unwrap();
        }
        let args = parse(&os(&["m"])).ok().unwrap();
        let (models, vad) = models(&args, Some(tmp.path().into())).ok().unwrap();
        let medium = dir.join("ggml-medium-q5_0.bin");
        assert_eq!(models, [("medium-q5_0".to_string(), medium.clone())]);
        assert_eq!(vad, dir.join(model::VAD.file));

        let named = parse(&os(&["m", "--model", medium.to_str().unwrap()]));
        let args = named.ok().unwrap();
        let (models, _) = super::models(&args, Some(tmp.path().into())).ok().unwrap();
        assert_eq!(models, [("medium-q5_0".to_string(), medium.clone())]);

        let missing = parse(&os(&["m", "--model", "/no/such.bin"])).ok().unwrap();
        assert!(matches!(
            super::models(&missing, Some(tmp.path().into())),
            Err(Fail::Model(_))
        ));
        fs::remove_file(dir.join(model::VAD.file)).unwrap();
        assert!(matches!(
            super::models(&args, Some(tmp.path().into())),
            Err(Fail::Model(_))
        ));
        assert!(matches!(super::models(&args, None), Err(Fail::Model(_))));
    }

    #[test]
    fn cer_counts_letters_after_nfc_and_ignores_spacing_and_punctuation() {
        // Decomposed Hangul (as macOS writes file names) and a combining accent.
        assert_eq!(cer("가나다", "\u{1100}\u{1161}나다"), Some(0.0));
        assert_eq!(cer("café", "cafe\u{301}"), Some(0.0));
        assert_eq!(
            cer("안녕하세요, 여러분!\n", "안녕 하세요… 여러분"),
            Some(0.0)
        );
        assert_eq!(cer("kitten", "sitting"), Some(0.5));
        assert_eq!(cer("회의를 시작합니다", "회의 시작합니다"), Some(1.0 / 8.0));
        assert_eq!(cer("abc", ""), Some(1.0));
        assert_eq!(cer("", "abc"), None);
        assert_eq!(cer(" ,. ", "abc"), None);
        // Lines copied with 전사본 복사 keep their times out of it; other brackets stay.
        let copied = "[00:01] 회의를 시작합니다\n[1:02:03] [웃음] 네\n[a] 끝";
        assert_eq!(spoken(copied), " 회의를 시작합니다\n [웃음] 네\n[a] 끝");
        assert_eq!(
            cer(&spoken(copied), "회의를 시작합니다 웃음 네 a 끝"),
            Some(0.0)
        );
    }

    /// A fake worker: plans two chunks, transcribes the same sentence for every chunk, and fails
    /// to load the model with `--fa 0` past the first chunk. It logs its arguments, and whether its
    /// stdin is a pipe.
    #[cfg(unix)]
    const FAKE: &str = r##"#!/bin/sh
echo "$*" >> 'LOG'
[ -p /dev/stdin ] || echo 'stdin is not a pipe' >> 'LOG'
case "$1" in
--plan-chunks)
  echo '{"type":"plan","len_ms":700000,"chunks":[{"start_ms":0,"end_ms":400000,"speech_ms":1},{"start_ms":400000,"end_ms":700000,"speech_ms":1}]}' ;;
--transcribe-worker)
  if [ "${15}" = 0 ] && [ "$3" != 0 ]; then echo '모델을 불러올 수 없습니다' >&2; exit 3; fi
  echo '{"type":"progress","pct":50}'
  printf '{"type":"diag","line":"chunk device=%s gpu=\\"Fake GPU\\" gpus=1 gpu_device=0 threads=%s beam=%s fa=%s lang=%s audio_s=1 speech_s=100.0 load_s=0.50 run_s=20.00 rtf=0.1 power=ac saver=0 ecoqos=-"}\n' "$5" "${13}" "${11}" "${15}" "$9"
  echo '{"type":"result","device":"cpu","segments":[{"start_ms":0,"end_ms":1,"text":"오늘 회의를 시작합니다"}]}' ;;
esac
"##;

    #[cfg(unix)]
    #[test]
    fn rows_reach_bench_tsv_one_per_combination_and_the_meeting_gets_nothing_else() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let meeting = tmp.path().join("meeting");
        fs::create_dir(&meeting).unwrap();
        fs::write(meeting.join("mic.wav"), b"RIFF").unwrap();
        let log = tmp.path().join("calls.log");
        let exe = tmp.path().join("worker.sh");
        fs::write(&exe, FAKE.replace("LOG", log.to_str().unwrap())).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let model = tmp.path().join("ggml-tiny.bin");
        let vad = tmp.path().join("vad.bin");
        let reference = tmp.path().join("reference.txt");
        fs::write(&model, b"m").unwrap();
        fs::write(&vad, b"v").unwrap();
        // One letter off over 20.
        fs::write(
            &reference,
            "오늘 회의를 시작합니다.\n오늘 회의를 시작합니까?\n",
        )
        .unwrap();
        let path = |p: &Path| p.to_str().unwrap().to_string();
        let mut args = vec![path(&meeting), "--model".into(), path(&model)];
        args.extend([
            "--vad".into(),
            path(&vad),
            "--reference".into(),
            path(&reference),
        ]);
        args.extend([
            "--only".into(),
            "cpu/tiny/b1/fa1/half,gpu/tiny/b5/fa0/half,gpu/tiny/b5/fa1/half".into(),
        ]);
        let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();

        let tsv = run(&args, &exe, None).ok().unwrap();
        assert_eq!(tsv, meeting.join("bench.tsv"));
        let mut left: Vec<_> = fs::read_dir(&meeting)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["bench.tsv", "mic.wav"]);

        let calls = fs::read_to_string(&log).unwrap();
        let calls: Vec<&str> = calls.lines().collect();
        let work = calls[0].split(' ').nth(1).unwrap();
        assert_eq!(
            calls[0],
            format!("--plan-chunks {work} {}", path(&vad)),
            "{calls:?}"
        );
        assert!(!Path::new(work).exists(), "the copy is removed");
        let threads = threads();
        assert_eq!(
            calls[1],
            format!(
                "--transcribe-worker {work} 0 400000 gpu {} {} --lang ko --beam 5 --threads {threads} --fa 1",
                path(&model),
                path(&vad)
            )
        );
        // The default range ends in the second chunk.
        assert!(calls[2].starts_with(&format!("--transcribe-worker {work} 400000 600000 gpu")));
        assert!(calls[3].contains(" 0 400000 gpu ") && calls[3].ends_with("--fa 0"));
        assert!(calls[4].contains(" 400000 600000 gpu ") && calls[4].ends_with("--fa 0"));
        assert!(calls[5].contains(" 0 400000 cpu ") && calls[5].contains("--beam 1"));
        assert_eq!(calls.len(), 7, "{calls:?}");

        let text = fs::read_to_string(&tsv).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("# meeting-summarizer"), "{}", lines[0]);
        assert!(lines[0].contains("조합 3개, 예상 약 "), "{}", lines[0]);
        assert_eq!(lines[1], COLUMNS.join("\t"));
        assert_eq!(lines.len(), 5);
        let rows: Vec<Vec<(&str, &str)>> = lines[2..]
            .iter()
            .map(|l| COLUMNS.iter().copied().zip(l.split('\t')).collect())
            .collect();
        let get = |row: usize, key: &str| rows[row].iter().find(|(k, _)| *k == key).unwrap().1;
        let t = threads.to_string();
        for (key, value) in [
            ("combo", "gpu/tiny/b5/fa1/half"),
            ("model", "tiny"),
            ("device", "gpu"),
            ("gpu", "Fake GPU"),
            ("gpus", "1"),
            ("gpu_device", "0"),
            ("threads", &t),
            ("beam", "5"),
            ("fa", "1"),
            ("lang", "ko"),
            ("audio_s", "600.0"),
            ("speech_s", "200.00"),
            ("load_s", "1.00"),
            ("run_s", "40.00"),
            ("rtf", "0.067"),
            ("cer_pct", "5.00"),
            ("power", "ac"),
            ("saver", "0"),
            ("ecoqos", "-"),
            ("status", "ok"),
        ] {
            assert_eq!(get(0, key), value, "{key}");
        }
        assert!(get(0, "wall_s").parse::<f64>().is_ok());
        // The failure in the second chunk: the settings it ran with, no times (the first chunk is not
        // the range), no CER, and why.
        for (key, value) in [
            ("combo", "gpu/tiny/b5/fa0/half"),
            ("device", "gpu"),
            ("gpu", "Fake GPU"),
            ("threads", &t),
            ("fa", "0"),
            ("audio_s", "600.0"),
            ("speech_s", ""),
            ("load_s", ""),
            ("run_s", ""),
            ("rtf", ""),
            ("cer_pct", ""),
            ("status", "exit 3: 모델을 불러올 수 없습니다"),
        ] {
            assert_eq!(get(1, key), value, "{key}");
        }
        assert_eq!(
            (get(2, "combo"), get(2, "beam"), get(2, "cer_pct")),
            ("cpu/tiny/b1/fa1/half", "1", "5.00")
        );

        // Again on the chunks the app saved, appended under a header of its own.
        let saved = r#"{"len_ms":300000,"chunks":[{"start_ms":0,"end_ms":300000,"speech_ms":1}]}"#;
        fs::write(meeting.join("chunks.json"), saved).unwrap();
        let smoke = os(&[
            &path(&meeting),
            "--model",
            &path(&model),
            "--vad",
            &path(&vad),
        ]);
        let smoke = [smoke, os(&["--smoke"])].concat();
        run(&smoke, &exe, None).ok().unwrap();
        let calls = fs::read_to_string(&log).unwrap();
        assert_eq!(calls.lines().count(), 8);
        assert!(calls.lines().last().unwrap().contains(" 0 300000 gpu "));
        let again = fs::read_to_string(&tsv).unwrap();
        assert!(again.starts_with(&text));
        let added: Vec<&str> = again[text.len()..].lines().collect();
        assert_eq!(added.len(), 3);
        // The range the recording has, not the 10 minutes asked for.
        assert!(
            added[0].contains("구간 0-300000ms(300초), 조합 1개"),
            "{}",
            added[0]
        );
        assert!(added[0].contains("기준 전사본 없음"));
        assert!(added[2].starts_with("gpu/tiny/b5/fa1/half\ttiny\tgpu\t"));
        assert!(!calls.contains("stdin is not a pipe"));
    }

    /// A worker silent for the stall is killed, and the row says so with the settings asked for.
    #[cfg(unix)]
    #[test]
    fn a_silent_worker_is_killed_and_its_row_says_stalled() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("worker.sh");
        fs::write(&exe, "#!/bin/sh\necho 'hanging' >&2\nexec sleep 30\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let combo = Combo {
            name: "gpu/tiny/b5/fa1/half".into(),
            device: Device::Gpu,
            model: ("tiny".into(), "m.bin".into()),
            beam: 5,
            fa: 1,
            threads: 4,
        };
        let started = Instant::now();
        let pieces = [(0, 1000), (1000, 2000)];
        // Long enough for the stderr line to be read even when the whole suite loads the machine.
        let stall = Duration::from_millis(1500);
        let measured = measure(
            &exe,
            tmp.path(),
            (Path::new("v.bin"), "ko"),
            &combo,
            &pieces,
            stall,
        );
        let measured = measured.ok().unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        let row = row(&combo, "ko", 2.0, &measured, None);
        let get = |key: &str| {
            COLUMNS
                .iter()
                .zip(row.split('\t'))
                .find(|(k, _)| **k == key)
                .unwrap()
                .1
        };
        for (key, value) in [
            ("device", "gpu"),
            ("threads", "4"),
            ("run_s", ""),
            ("rtf", ""),
            ("status", "stalled: hanging"),
        ] {
            assert_eq!(get(key), value, "{key}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_day_old_bench_copy_is_removed_and_a_newer_one_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, new) = (format!("{WORK}1"), format!("{WORK}2"));
        for name in [&old, &new, "other"] {
            fs::create_dir(tmp.path().join(name)).unwrap();
            fs::write(tmp.path().join(name).join("mic.wav"), b"RIFF").unwrap();
        }
        let day_ago = std::time::SystemTime::now() - Duration::from_secs(25 * 60 * 60);
        for name in [&old, "other"] {
            let dir = File::open(tmp.path().join(name)).unwrap();
            dir.set_modified(day_ago).unwrap();
        }
        remove_stale(tmp.path());
        let mut left: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, [new, "other".into()]);
    }

    /// Plan 5.1-14 from a checkout: the bench with the release app built next to this test binary as
    /// the worker, on the installed app's models. `cargo build --release` first, then
    /// `TRANSCRIBE_EVAL_DIR=<meeting folder> [TRANSCRIBE_EVAL_REFERENCE=<txt>] cargo test --release
    /// --lib -- --ignored --exact transcribe::bench::tests::transcribe_eval --nocapture`.
    #[test]
    #[ignore]
    #[allow(clippy::assertions_on_constants)]
    fn transcribe_eval() {
        assert!(
            !cfg!(debug_assertions),
            "디버그 빌드의 속도는 실측이 아닙니다. --release로 돌립니다"
        );
        let exe = std::env::current_exe().unwrap();
        let release = exe.parent().and_then(Path::parent).unwrap();
        let app = release.join(format!(
            "meeting-summarizer{}",
            std::env::consts::EXE_SUFFIX
        ));
        assert!(
            app.is_file(),
            "{}가 없습니다. 먼저 cargo build --release로 빌드합니다",
            app.display()
        );
        let dir = std::env::var_os("TRANSCRIBE_EVAL_DIR").expect("TRANSCRIBE_EVAL_DIR에 회의 폴더");
        let mut args = vec![dir];
        if let Some(reference) = std::env::var_os("TRANSCRIBE_EVAL_REFERENCE") {
            args.extend(["--reference".into(), reference]);
        }
        let ended = run(&args, &app, app_dir()).map(|tsv| println!("결과: {}", tsv.display()));
        assert_eq!(code_of(ended), 0);
    }
}
