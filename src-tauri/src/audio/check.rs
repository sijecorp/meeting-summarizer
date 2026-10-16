//! The settings' audio tab checks (ONB-03, architecture 4.4): the microphone's level for a few
//! seconds, and whether a tone we play reaches the system audio track. Both open their streams on
//! the calling thread, which must be a blocking one: a permission prompt holds the opening.

use super::Track;
use super::capture::Input;
use super::writer::{SILENT_RMS, Source};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, DeviceId, FromSample, Host, Sample, SampleFormat, SizedSample, StreamConfig};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const MIC_LISTEN: Duration = Duration::from_secs(5);
/// How often a check looks for a recording. The mic's level goes out as often: 25 a second, like
/// `recording://level`.
const TICK: Duration = Duration::from_millis(40);
const TONE_HZ: f32 = 1_000.0;
/// -20 dBFS peak: clearly heard, never loud.
const TONE_PEAK: f32 = 0.1;
const TONE_SECS: f32 = 1.0;
/// Fades in and out, so the tone does not click.
const FADE_SECS: f32 = 0.01;
/// The tone and some room for the output and loopback latency.
const SYSTEM_LISTEN: Duration = Duration::from_millis(1_200);
/// Architecture 4.4: heard when the loopback's RMS is above this.
const PASS_DBFS: f32 = -40.0;
const RECORDING: &str = "녹음 중에는 테스트할 수 없습니다";
/// A recording that started mid-check: said apart from `RECORDING`, which would read as still
/// recording once it ends.
const STOPPED: &str = "녹음이 시작되어 테스트를 멈췄습니다";

/// One check at a time in the whole app: a second would open the same devices, and a settings
/// screen left and opened again mid-check knows nothing of the first.
static CHECKING: AtomicBool = AtomicBool::new(false);

/// Holds `CHECKING` until dropped.
struct Checking;

impl Checking {
    fn claim() -> Result<Self, String> {
        if CHECKING.swap(true, Ordering::SeqCst) {
            return Err("이미 테스트 중입니다".into());
        }
        Ok(Self)
    }
}

impl Drop for Checking {
    fn drop(&mut self) {
        CHECKING.store(false, Ordering::SeqCst);
    }
}

/// Plays a 1 kHz tone for a second and listens to the system audio meanwhile; true when it was
/// heard. macOS hears every output through a tap of its own, so the tone goes to the default
/// output and `output_device` is ignored. Windows plays it on `output_device` (the default when
/// `None`) and listens to that device's WASAPI loopback. `recording` says a recording runs: one
/// at the start refuses the check, and one that starts meanwhile stops the tone at once, so it
/// never lands in `system.wav`, and ends it with `STOPPED`.
pub fn system_audio(
    output_device: Option<String>,
    recording: impl Fn() -> bool,
) -> Result<bool, String> {
    loopback_dbfs(output_device, recording).map(passes)
}

/// What the system audio heard over the tone: its RMS in dBFS.
fn loopback_dbfs(
    output_device: Option<String>,
    recording: impl Fn() -> bool,
) -> Result<f32, String> {
    let _checking = Checking::claim()?;
    if recording() {
        return Err(RECORDING.into());
    }
    let host = cpal::default_host();
    let output = output_device.filter(|_| !cfg!(target_os = "macos"));
    let device = pick(&host, Track::System, output)?;
    let (sources, received) = mpsc::channel();
    // Listening starts first, so it hears the whole tone.
    let mut input = Input::new(Track::System, None);
    input
        .open_stream(&device, &sources)
        .map_err(|e| format!("시스템 오디오를 열 수 없습니다: {e}"))?;
    let mut source = received.recv().map_err(|e| e.to_string())?;
    // Opening may have waited on a permission prompt.
    if recording() {
        return Err(STOPPED.into());
    }
    let tone = play(&device).map_err(|e| format!("출력 장치를 열 수 없습니다: {e}"))?;
    // From the tone on only: silence from before would water the level down, and a slow output
    // could have filled the stream's 2 s and dropped the tone.
    take(&mut source);
    let end = Instant::now() + SYSTEM_LISTEN;
    while Instant::now() < end {
        // Returning drops the tone first.
        if recording() {
            return Err(STOPPED.into());
        }
        thread::sleep(TICK);
    }
    drop(tone);
    // 1.2 s fits in the stream's 2 s, so one read covers the window.
    let (sum, count) = take(&mut source);
    Ok(dbfs(rms(sum, count)))
}

/// Opens the microphone `input_device` names (the default one when `None`) for 5 s and sends its
/// level, 0..100 like the recording screen's meter, 25 times a second. True when anything louder
/// than REC-05's silence (-50 dBFS) came in. A recording, at the start or meanwhile, ends it with
/// an error, as in `system_audio`.
pub fn microphone(
    input_device: Option<String>,
    recording: impl Fn() -> bool,
    level: impl Fn(u8),
) -> Result<bool, String> {
    let _checking = Checking::claim()?;
    if recording() {
        return Err(RECORDING.into());
    }
    let host = cpal::default_host();
    let device = pick(&host, Track::Mic, input_device)?;
    let (sources, received) = mpsc::channel();
    let mut input = Input::new(Track::Mic, None);
    input
        .open_stream(&device, &sources)
        .map_err(|e| format!("마이크를 열 수 없습니다: {e}"))?;
    let mut source = received.recv().map_err(|e| e.to_string())?;
    let mut heard = false;
    let end = Instant::now() + MIC_LISTEN;
    while Instant::now() < end {
        thread::sleep(TICK);
        if recording() {
            return Err(STOPPED.into());
        }
        // A tick without a device buffer keeps the last level rather than flicking to 0.
        let (sum, count) = take(&mut source);
        if count > 0 {
            let rms = rms(sum, count);
            heard |= rms >= SILENT_RMS;
            level(meter(rms));
        }
    }
    level(0);
    Ok(heard)
}

/// The pinned device, or the default one. A pinned device that is gone fails: the check must not
/// pass on some other device.
fn pick(host: &Host, track: Track, pinned: Option<String>) -> Result<Device, String> {
    let name = match track {
        Track::Mic => "마이크",
        Track::System => "출력 장치",
    };
    match pinned {
        Some(id) => id
            .parse::<DeviceId>()
            .ok()
            .and_then(|id| host.device_by_id(&id))
            .ok_or_else(|| format!("고른 {name}가 연결되어 있지 않습니다")),
        None => match track {
            Track::Mic => host.default_input_device(),
            Track::System => host.default_output_device(),
        }
        .ok_or_else(|| format!("{name}를 찾을 수 없습니다")),
    }
}

/// Sum of squares and count of the samples delivered since the last call.
fn take(source: &mut Source) -> (f64, usize) {
    let ready = source.samples.slots();
    let Ok(chunk) = source.samples.read_chunk(ready) else {
        return (0.0, 0);
    };
    let (a, b) = chunk.as_slices();
    let sum = a.iter().chain(b).map(|&s| f64::from(s * s)).sum();
    chunk.commit_all();
    (sum, ready)
}

fn rms(sum: f64, count: usize) -> f32 {
    if count == 0 {
        0.0
    } else {
        (sum / count as f64).sqrt() as f32
    }
}

/// Silence reads -120.
fn dbfs(rms: f32) -> f32 {
    20.0 * rms.max(1e-6).log10()
}

/// -60..0 dBFS as 0..100, the recording screen's meter scale (Recording.tsx).
fn meter(rms: f32) -> u8 {
    ((dbfs(rms) + 60.0) / 60.0 * 100.0)
        .clamp(0.0, 100.0)
        .round() as u8
}

fn passes(loopback_dbfs: f32) -> bool {
    loopback_dbfs > PASS_DBFS
}

/// One second of the faded 1 kHz sine at `rate`, mono.
fn tone(rate: u32) -> Vec<f32> {
    let rate = rate as f32;
    let len = (rate * TONE_SECS) as usize;
    let fade = rate * FADE_SECS;
    (0..len)
        .map(|i| {
            let edge = (i.min(len - 1 - i) as f32 / fade).min(1.0);
            TONE_PEAK * edge * (std::f32::consts::TAU * TONE_HZ * i as f32 / rate).sin()
        })
        .collect()
}

/// The tone on every channel of `device`, then silence until the stream drops.
fn play(device: &Device) -> Result<cpal::Stream, String> {
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let tone = tone(config.sample_rate());
    let stream_config = config.config();
    let stream = match config.sample_format() {
        SampleFormat::F32 => output::<f32>(device, stream_config, tone),
        SampleFormat::I16 => output::<i16>(device, stream_config, tone),
        SampleFormat::I32 => output::<i32>(device, stream_config, tone),
        other => Err(format!("지원하지 않는 샘플 형식입니다: {other}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

fn output<T: SizedSample + FromSample<f32>>(
    device: &Device,
    config: StreamConfig,
    tone: Vec<f32>,
) -> Result<cpal::Stream, String> {
    let channels = usize::from(config.channels);
    let mut next = 0;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                // Real-time thread: the tone was made before, so nothing allocates here.
                for frame in data.chunks_mut(channels) {
                    frame.fill(tone.get(next).copied().unwrap_or(0.0).to_sample());
                    next += 1;
                }
            },
            |e| eprintln!("테스트음 재생 오류: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(samples: &[f32]) -> (f64, usize) {
        let sum = samples.iter().map(|&s| f64::from(s * s)).sum();
        (sum, samples.len())
    }

    #[test]
    fn the_tone_is_a_faded_1khz_sine_at_minus_20_dbfs() {
        let tone = tone(48_000);
        assert_eq!(tone.len(), 48_000);
        // No click at either end.
        assert!(tone[0].abs() < 1e-6 && tone[47_999].abs() < 1e-3);
        let peak = tone.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!((0.099..=TONE_PEAK).contains(&peak), "{peak}");
        let crossings = tone
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!((1_995..=2_005).contains(&crossings), "{crossings}");
        // A sine's RMS is 3 dB under its peak.
        let middle = power(&tone[480..47_520]);
        assert!((dbfs(rms(middle.0, middle.1)) + 23.0).abs() < 0.1);
    }

    #[test]
    fn the_system_check_passes_above_minus_40_dbfs() {
        let level = |samples: &[f32]| {
            let (sum, count) = power(samples);
            dbfs(rms(sum, count))
        };
        let tone = tone(48_000);
        assert!(passes(level(&tone)));
        // The same tone 30 dB down: -53 dBFS.
        let quiet: Vec<f32> = tone.iter().map(|s| s * 0.031_6).collect();
        assert!((level(&quiet) + 53.0).abs() < 0.2, "{}", level(&quiet));
        assert!(!passes(level(&quiet)));
        // No permission on macOS: only zeros, or nothing at all.
        assert_eq!(level(&[0.0; 480]), -120.0);
        assert_eq!(dbfs(rms(0.0, 0)), -120.0);
        assert!(!passes(-120.0));
    }

    /// Both refuse before opening a device, so this runs without one.
    #[test]
    fn one_check_at_a_time_and_none_beside_a_recording() {
        assert_eq!(system_audio(None, || true), Err(RECORDING.into()));
        assert_eq!(microphone(None, || true, |_| {}), Err(RECORDING.into()));
        let first = Checking::claim().unwrap();
        let second = microphone(None, || false, |_| {});
        assert_eq!(second, Err("이미 테스트 중입니다".into()));
        assert!(Checking::claim().is_err());
        drop(first);
        assert!(Checking::claim().is_ok());
    }

    /// Real devices, to tune `PASS_DBFS` and `SYSTEM_LISTEN`: plays the tone and prints the dBFS
    /// that came back. `cargo test --lib on_this_machine -- --ignored --nocapture`. The permission prompts
    /// name the terminal.
    #[test]
    #[ignore]
    fn on_this_machine() {
        println!("system audio: {:?} dBFS", loopback_dbfs(None, || false));
        let peak = std::sync::atomic::AtomicU8::new(0);
        let heard = microphone(
            None,
            || false,
            |level| {
                peak.fetch_max(level, Ordering::Relaxed);
            },
        );
        println!("microphone: {heard:?}, loudest {peak:?} of 100");
    }

    #[test]
    fn the_mic_meter_reads_minus_60_to_0_dbfs_as_0_to_100() {
        assert_eq!(meter(1.0), 100);
        assert_eq!(meter(0.031_62), 50);
        assert_eq!(meter(0.001), 0);
        assert_eq!(meter(0.0), 0);
        // REC-05's silence line is the meter's 17.
        assert_eq!(meter(SILENT_RMS), 17);
    }
}
