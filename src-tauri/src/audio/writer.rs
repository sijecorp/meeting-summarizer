//! Writer thread (architecture 4.1). Per track: downmix → 16 kHz → align to the clock → 16-bit
//! WAV. It also runs the checks that need the recorded audio or the clock: levels, silence, mic
//! permission, maximum length, disk space and sleep.

use super::clock::{self, Aligner, Reading, Sleep, SleepWatch};
use super::{Emit, Event, Level, Shared, Stopped, Track, Warning, free_space};
use crate::store::{Gap, Store};
use hound::WavWriter;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use std::io::{Seek, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::time::Duration;

const TICK: Duration = Duration::from_millis(20);
const FLUSH_EVERY_MS: u64 = 5_000;
const LEVEL_EVERY_MS: u64 = 100;
const DISK_EVERY_MS: u64 = 60_000;
const MAX_LENGTH_MS: u64 = 4 * 3_600_000;
const LENGTH_WARNING_MS: u64 = MAX_LENGTH_MS - 10 * 60_000;
const SILENCE_PROMPT_MS: u64 = 15 * 60_000;
/// -50 dBFS (REC-05).
const SILENT_RMS: f32 = 0.003_162_3;
/// Below this the recording stops safely (REC-11).
const STOP_FREE_BYTES: u64 = 200_000_000;

/// A capture stream as the writer sees it: interleaved samples at the device's rate.
pub struct Source {
    pub track: Track,
    pub samples: rtrb::Consumer<f32>,
    pub rate: u32,
    pub channels: u16,
}

/// Downmix to mono, then resample to 16 kHz with rubato's anti-aliased FFT resampler.
pub struct Pipe {
    channels: usize,
    resampler: Fft<f32>,
    /// Mono frames waiting for a full resampler chunk.
    input: Vec<f32>,
    output: Vec<f32>,
    /// The resampler's startup delay, trimmed so the audio is not shifted late.
    skip: usize,
}

impl Pipe {
    pub fn new(rate: u32, channels: u16) -> Result<Self, String> {
        let resampler = Fft::new(
            rate as usize,
            clock::RATE as usize,
            1024,
            1,
            FixedSync::Input,
        )
        .map_err(|e| format!("{rate} Hz 오디오를 변환할 수 없습니다: {e}"))?;
        Ok(Self {
            channels: channels.max(1).into(),
            input: Vec::with_capacity(resampler.input_frames_max() * 2),
            output: vec![0.0; resampler.output_frames_max()],
            skip: resampler.output_delay(),
            resampler,
        })
    }

    /// Appends the 16 kHz mono samples made from whole interleaved frames to `out`.
    pub fn push(&mut self, interleaved: &[f32], out: &mut Vec<f32>) {
        let channels = self.channels;
        self.input.extend(
            interleaved
                .chunks_exact(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32),
        );
        let mut used = 0;
        while self.input.len() - used >= self.resampler.input_frames_next() {
            let need = self.resampler.input_frames_next();
            let room = self.output.len();
            let (read, written) = self
                .resampler
                .process_into_buffer(
                    &InterleavedSlice::new(&self.input[used..used + need], 1, need).unwrap(),
                    &mut InterleavedSlice::new_mut(&mut self.output, 1, room).unwrap(),
                    None,
                )
                .expect("buffers are sized by the resampler itself");
            used += read;
            let skip = self.skip.min(written);
            self.skip -= skip;
            out.extend_from_slice(&self.output[skip..written]);
        }
        self.input.drain(..used);
    }
}

struct TrackOut<W: Write + Seek> {
    wav: WavWriter<W>,
    align: Aligner,
    source: Option<(Source, Pipe)>,
    /// 16 kHz samples produced this tick.
    fresh: Vec<f32>,
    sum_sq: f64,
    count: u64,
    /// Device samples of the first 3 s of microphone audio still to check (REC-08).
    zero_left: Option<u64>,
    zero_check_started: bool,
}

impl<W: Write + Seek> TrackOut<W> {
    fn new(wav: WavWriter<W>, check_zeros: bool) -> Self {
        Self {
            wav,
            align: Aligner::default(),
            source: None,
            fresh: Vec::new(),
            sum_sq: 0.0,
            count: 0,
            zero_left: None,
            zero_check_started: !check_zeros,
        }
    }

    fn write(&mut self, sample: f32) -> hound::Result<()> {
        self.sum_sq += f64::from(sample * sample);
        self.count += 1;
        self.wav
            .write_sample((sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
    }

    /// Takes what the device delivered, aligns it to the clock and writes it.
    /// Returns true when the first 3 s of microphone audio turned out to be exact zeros.
    fn pull(&mut self, elapsed_ms: u64) -> hound::Result<bool> {
        let mut all_zero = false;
        self.fresh.clear();
        if let Some((source, pipe)) = &mut self.source {
            let slots = source.samples.slots();
            let ready = slots - slots % pipe.channels;
            if let Ok(chunk) = source.samples.read_chunk(ready) {
                let (a, b) = chunk.as_slices();
                if let Some(left) = self.zero_left {
                    let silent = a.iter().chain(b).all(|&s| s == 0.0);
                    let left = left.saturating_sub(ready as u64);
                    self.zero_left = (silent && left > 0).then_some(left);
                    all_zero = silent && left == 0;
                }
                pipe.push(a, &mut self.fresh);
                pipe.push(b, &mut self.fresh);
                chunk.commit_all();
            }
        }
        let fresh = std::mem::take(&mut self.fresh);
        let keep = self.align.keep(fresh.len() as u64) as usize;
        let written = fresh[fresh.len() - keep..]
            .iter()
            .try_for_each(|&s| self.write(s));
        self.fresh = fresh;
        written?;
        for _ in 0..self.align.check(elapsed_ms) {
            self.write(0.0)?;
        }
        Ok(all_zero)
    }

    /// RMS since the last call.
    fn take_rms(&mut self) -> f32 {
        let rms = if self.count == 0 {
            0.0
        } else {
            (self.sum_sq / self.count as f64).sqrt() as f32
        };
        (self.sum_sq, self.count) = (0.0, 0);
        rms
    }
}

#[derive(Debug, PartialEq)]
pub enum Stop {
    User,
    MaxLength,
    Sleep,
    DiskLow,
    WriteError(String),
}

impl Stop {
    /// Why the recording ended, for the recorder. `None` when they stopped it.
    fn message(&self) -> Option<String> {
        match self {
            Stop::User => None,
            Stop::MaxLength => Some("최대 녹음 길이(4시간)가 되어 녹음을 종료했습니다".into()),
            Stop::Sleep => Some("절전이 5분을 넘어 절전 시점에서 녹음을 종료했습니다".into()),
            Stop::DiskLow => Some("디스크 공간이 부족해 녹음을 종료했습니다".into()),
            Stop::WriteError(e) => Some(format!("녹음 파일을 쓰지 못해 녹음을 종료했습니다: {e}")),
        }
    }
}

pub struct Writer<W: Write + Seek> {
    id: String,
    dir: PathBuf,
    mic: TrackOut<W>,
    system: TrackOut<W>,
    sources: Receiver<Source>,
    shared: Arc<Shared>,
    emit: Emit,
    start: Reading,
    sleep: SleepWatch,
    gaps: Vec<Gap>,
    elapsed_ms: u64,
    next_flush_ms: u64,
    next_level_ms: u64,
    next_disk_ms: u64,
    silent_ms: u64,
    length_warned: bool,
}

impl<W: Write + Seek> Writer<W> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        dir: PathBuf,
        mic: WavWriter<W>,
        system: WavWriter<W>,
        sources: Receiver<Source>,
        shared: Arc<Shared>,
        emit: Emit,
        start: Reading,
    ) -> Self {
        Self {
            id,
            dir,
            mic: TrackOut::new(mic, true),
            system: TrackOut::new(system, false),
            sources,
            shared,
            emit,
            start,
            sleep: SleepWatch::new(start),
            gaps: Vec::new(),
            elapsed_ms: 0,
            next_flush_ms: FLUSH_EVERY_MS,
            next_level_ms: LEVEL_EVERY_MS,
            next_disk_ms: DISK_EVERY_MS,
            silent_ms: 0,
            length_warned: false,
        }
    }

    fn warn(&self, kind: &'static str, message: String) {
        (self.emit)(Event::Warning(Warning { kind, message }));
    }

    fn track(&mut self, track: Track) -> &mut TrackOut<W> {
        match track {
            Track::Mic => &mut self.mic,
            Track::System => &mut self.system,
        }
    }

    /// A reconnected device replaces the track's source. What the old one still holds is kept.
    fn attach(&mut self, source: Source) -> hound::Result<()> {
        let elapsed_ms = self.elapsed_ms;
        let out = self.track(source.track);
        out.pull(elapsed_ms)?;
        match Pipe::new(source.rate, source.channels) {
            Ok(pipe) => {
                if !out.zero_check_started {
                    out.zero_check_started = true;
                    out.zero_left = Some(3 * u64::from(source.rate) * u64::from(source.channels));
                }
                out.source = Some((source, pipe));
            }
            Err(e) => {
                out.source = None;
                let kind = match source.track {
                    Track::Mic => "mic_device",
                    Track::System => "system_device",
                };
                self.warn(kind, e);
            }
        }
        Ok(())
    }

    /// One pass of the writer loop. Returns why the recording must end, if it must.
    pub fn tick(&mut self, now: Reading) -> Option<Stop> {
        match self.sleep.poll(now) {
            Sleep::Awake => {}
            Sleep::Short(len_ms) => {
                self.gaps.push(Gap {
                    at_ms: self.elapsed_ms,
                    len_ms,
                    reason: "sleep".into(),
                });
                self.warn(
                    "sleep",
                    format!(
                        "절전으로 녹음이 약 {}초 동안 멈췄다가 이어졌습니다",
                        len_ms.div_ceil(1000)
                    ),
                );
            }
            // Nothing after the sleep point is written (REC-06).
            Sleep::Long(_) => return Some(Stop::Sleep),
        }
        self.elapsed_ms = now.awake_ms.saturating_sub(self.start.awake_ms);
        let elapsed = self.elapsed_ms;

        let io = (|| {
            while let Ok(source) = self.sources.try_recv() {
                self.attach(source)?;
            }
            if self.mic.pull(elapsed)? {
                self.warn(
                    "mic_permission",
                    "마이크 소리가 들어오지 않습니다. 시스템 설정에서 이 앱의 마이크 권한을 확인하세요"
                        .into(),
                );
            }
            self.system.pull(elapsed)?;
            if elapsed >= self.next_flush_ms {
                self.next_flush_ms = elapsed + FLUSH_EVERY_MS;
                // Rewrites the WAV headers: a crash now loses at most 5 s (REC-02).
                self.mic.wav.flush()?;
                self.system.wav.flush()?;
            }
            Ok::<_, hound::Error>(())
        })();
        if let Err(e) = io {
            return Some(Stop::WriteError(e.to_string()));
        }

        if elapsed >= self.next_level_ms {
            let since = elapsed + LEVEL_EVERY_MS - self.next_level_ms;
            self.next_level_ms = elapsed + LEVEL_EVERY_MS;
            let (mic, system) = (self.mic.take_rms(), self.system.take_rms());
            let before = self.silent_ms;
            self.silent_ms = if mic < SILENT_RMS && system < SILENT_RMS {
                before + since
            } else {
                0
            };
            if before < SILENCE_PROMPT_MS && self.silent_ms >= SILENCE_PROMPT_MS {
                self.warn(
                    "silence",
                    "15분 동안 소리가 없습니다. 녹음을 계속할까요? 회의가 끝났다면 녹음을 종료하세요"
                        .into(),
                );
            }
            let opening = self.shared.opening_since_ms.load(Ordering::SeqCst);
            (self.emit)(Event::Level(Level {
                elapsed_ms: elapsed,
                mic,
                system,
                waiting_permission: opening != 0 && now.awake_ms.saturating_sub(opening) > 1_000,
            }));
        }

        if elapsed >= LENGTH_WARNING_MS && !self.length_warned {
            self.length_warned = true;
            self.warn(
                "length",
                "최대 녹음 길이(4시간)까지 10분 남았습니다. 4시간이 되면 자동으로 종료합니다"
                    .into(),
            );
        }
        if elapsed >= MAX_LENGTH_MS {
            return Some(Stop::MaxLength);
        }
        if elapsed >= self.next_disk_ms {
            self.next_disk_ms = elapsed + DISK_EVERY_MS;
            if free_space(&self.dir).is_ok_and(|free| free < STOP_FREE_BYTES) {
                return Some(Stop::DiskLow);
            }
        }
        None
    }

    /// Final flush, then `recorded` (architecture 4.6 safe stop). A WAV that cannot be finalized
    /// keeps the header of its last successful 5 s flush, so it stays readable.
    pub fn finish(self, store: &Store, stop: Stop) {
        for (name, wav) in [("mic.wav", self.mic.wav), ("system.wav", self.system.wav)] {
            if let Err(e) = wav.finalize() {
                eprintln!("{name} 마무리 실패 ({}): {e}", self.id);
            }
        }
        if let Err(e) = store.finish_recording(&self.id, self.elapsed_ms, self.gaps) {
            eprintln!("녹음 상태 저장 실패 ({}): {e}", self.id);
        }
        (self.emit)(Event::Stopped(Stopped {
            id: self.id,
            message: stop.message(),
        }));
    }
}

/// The writer thread: every 20 ms, until a stop is requested or the writer decides to stop.
pub fn run<W: Write + Seek>(mut writer: Writer<W>, store: &Store) {
    loop {
        std::thread::sleep(TICK);
        let stop = writer.tick(clock::now()).or_else(|| {
            writer
                .shared
                .stop
                .load(Ordering::SeqCst)
                .then_some(Stop::User)
        });
        if let Some(stop) = stop {
            // Also stops the capture thread when the writer ended the recording itself.
            writer.shared.stop.store(true, Ordering::SeqCst);
            writer.finish(store, stop);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Status;
    use std::fs::File;
    use std::io::{self, BufWriter};
    use std::sync::Mutex;
    use std::sync::mpsc;

    /// 1 kHz on every channel, run through a `Pipe` in 10 ms device callbacks.
    fn resample(rate: u32, channels: u16, seconds: u32) -> Vec<f32> {
        let mut pipe = Pipe::new(rate, channels).unwrap();
        let input: Vec<f32> = (0..rate * seconds)
            .flat_map(|i| {
                let s = (std::f32::consts::TAU * 1_000.0 * i as f32 / rate as f32).sin() * 0.5;
                std::iter::repeat_n(s, channels.into())
            })
            .collect();
        let mut out = Vec::new();
        for callback in input.chunks((rate / 100 * u32::from(channels)) as usize) {
            pipe.push(callback, &mut out);
        }
        out
    }

    #[test]
    fn downmixes_and_resamples_to_16khz_mono() {
        for (rate, channels) in [(48_000, 2), (44_100, 1)] {
            let out = resample(rate, channels, 10);
            // All but the resampler's last partial chunk and startup delay: within 100 ms.
            let short = 160_000 - out.len() as i64;
            assert!(
                (0..1_600).contains(&short),
                "{rate} Hz: {} samples",
                out.len()
            );
            // Still 1 kHz at 16 kHz: 2 zero crossings per period, 2,000 per second.
            let crossings = out[16_000..32_000]
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count();
            assert!(
                (1_998..=2_002).contains(&crossings),
                "{rate} Hz: {crossings}"
            );
        }
    }

    /// Writes through to a real file until the byte budget runs out, then fails like a full disk.
    struct FailAfter {
        file: File,
        left: usize,
    }

    impl Write for FailAfter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.left == 0 {
                return Err(io::Error::other("디스크가 가득 찼습니다"));
            }
            let n = buf.len().min(self.left);
            self.file.write_all(&buf[..n])?;
            self.left -= n;
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    impl Seek for FailAfter {
        fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
            self.file.seek(pos)
        }
    }

    type Events = Arc<Mutex<Vec<Event>>>;

    fn writer(
        store: &Store,
        budgets: [usize; 2],
    ) -> (
        Writer<BufWriter<FailAfter>>,
        PathBuf,
        mpsc::Sender<Source>,
        Events,
    ) {
        let id = store.create("").unwrap().id;
        let dir = store.dir_for(&id).unwrap();
        let [mic, system] =
            [("mic.wav", budgets[0]), ("system.wav", budgets[1])].map(|(name, left)| {
                let file = File::create(dir.join(name)).unwrap();
                WavWriter::new(BufWriter::new(FailAfter { file, left }), super::super::SPEC)
                    .unwrap()
            });
        let (tx, rx) = mpsc::channel();
        let events = Events::default();
        let sink = events.clone();
        let emit: Emit = Arc::new(move |e| sink.lock().unwrap().push(e));
        let start = Reading {
            awake_ms: 1_000,
            total_ms: 1_000,
        };
        let w = Writer::new(
            id,
            dir.clone(),
            mic,
            system,
            rx,
            Arc::default(),
            emit,
            start,
        );
        (w, dir, tx, events)
    }

    fn at(elapsed_ms: u64) -> Reading {
        Reading {
            awake_ms: 1_000 + elapsed_ms,
            total_ms: 1_000 + elapsed_ms,
        }
    }

    /// REC-11, architecture 4.6: a failing disk ends the recording as `recorded`, and the WAV
    /// keeps the header of its last 5 s flush, so it reads back with exactly that length.
    #[test]
    fn write_error_stops_safely_with_a_valid_wav() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("meetings")).unwrap();
        // Room for the 5 s flush (160 KB) but not the one at 10 s.
        let (mut w, dir, _tx, events) = writer(&store, [44 + 6 * 32_000, usize::MAX]);
        let id = w.id.clone();

        let mut flushed = 0;
        let mut stop = None;
        for tick in 1..=1_000 {
            stop = w.tick(at(tick * 20));
            if tick * 20 == 5_000 {
                flushed = w.mic.align.written;
            }
            if stop.is_some() {
                break;
            }
        }
        let stop = stop.expect("the mic sink must fail before 20 s");
        assert!(matches!(stop, Stop::WriteError(_)), "{stop:?}");
        w.finish(&store, stop);

        assert_eq!(store.get(&id).unwrap().status, Status::Recorded);
        let mic = hound::WavReader::open(dir.join("mic.wav")).unwrap();
        assert_eq!(u64::from(mic.len()), flushed);
        assert!(flushed > 70_000);
        assert_eq!(
            mic.into_samples::<i16>().filter(Result::is_ok).count() as u64,
            flushed
        );
        // The healthy track was finalized with everything written.
        let system = hound::WavReader::open(dir.join("system.wav")).unwrap();
        assert!(u64::from(system.len()) > flushed);

        let events = events.lock().unwrap();
        let Some(Event::Stopped(stopped)) = events.last() else {
            panic!("no stopped event");
        };
        assert_eq!(stopped.id, id);
        assert!(
            stopped
                .message
                .as_ref()
                .unwrap()
                .contains("녹음 파일을 쓰지 못해")
        );
    }

    fn mic_source(tx: &mpsc::Sender<Source>, samples: &[f32]) {
        let (mut producer, consumer) = rtrb::RingBuffer::new(samples.len());
        for &s in samples {
            producer.push(s).unwrap();
        }
        tx.send(Source {
            track: Track::Mic,
            samples: consumer,
            rate: 16_000,
            channels: 1,
        })
        .unwrap();
    }

    fn warned(events: &Events, kind: &str) -> bool {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Warning(w) if w.kind == kind))
    }

    /// REC-08: the first 3 s of microphone audio all exactly zero → permission warning.
    #[test]
    fn three_seconds_of_exact_zeros_from_the_mic_warns_about_permission() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("meetings")).unwrap();

        let (mut w, _, tx, events) = writer(&store, [usize::MAX; 2]);
        mic_source(&tx, &[0.0; 47_999]);
        w.tick(at(3_000));
        assert!(!warned(&events, "mic_permission"), "not 3 s yet");
        mic_source(&tx, &[0.0; 1]);
        w.tick(at(3_020));
        assert!(warned(&events, "mic_permission"));

        let (mut w, _, tx, events) = writer(&store, [usize::MAX; 2]);
        let mut samples = [0.0; 48_000];
        samples[100] = 0.001;
        mic_source(&tx, &samples);
        w.tick(at(3_000));
        assert!(!warned(&events, "mic_permission"));
    }
}
