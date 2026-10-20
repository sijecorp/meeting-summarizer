//! Manual upload (REC-12): a recording made elsewhere becomes a meeting like one recorded here.
//! The file is decoded (symphonia), downmixed and resampled to 16 kHz like a capture stream
//! (`writer::Pipe`), and written as the meeting's `mic.wav`. There is no `system.wav`: `mix` reads
//! the one track alone. The file picker's list (`RECORDING_FILES`, ipc.ts) follows the formats
//! built in (Cargo.toml).

use super::SPEC;
use super::writer::Pipe;
use crate::store::{Meta, Store};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use time::{Duration, OffsetDateTime};

/// REC-04's limit holds for uploads too.
const MAX_SAMPLES: u64 = 4 * 60 * 60 * SPEC.sample_rate as u64;
const UNSUPPORTED: &str =
    "지원하지 않는 오디오 형식입니다. mp3, m4a, wav, flac, ogg 파일을 골라 주세요";
const UNSUPPORTED_CODEC: &str =
    "이 파일의 오디오 코덱은 지원하지 않습니다(Opus, ALAC 등). mp3나 wav로 바꿔 가져와 주세요";
const TOO_LONG: &str = "4시간보다 긴 녹음은 가져올 수 없습니다";

/// `path` as a new `recorded` meeting. Its start is the file's last change (a recorder saves at the
/// end) less its length, never later than now less its length. `progress` hears how much of the
/// file is decoded, 0 to 100, each time that changes.
pub fn import(
    store: &Store,
    path: &Path,
    summarize_local: bool,
    progress: impl FnMut(u8),
) -> Result<Meta, String> {
    let temp = store.import_temp();
    let decoded = decode(path, &temp, progress);
    let samples = match decoded {
        Ok(samples) => samples,
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
    };
    let duration_ms = samples * 1000 / u64::from(SPEC.sample_rate);
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let created_at = started(modified, SystemTime::now(), duration_ms);
    store.import(&temp, created_at, duration_ms, summarize_local)
}

/// When the recording began: it ended at the file's last change, or now if that is unknown or in
/// the future.
fn started(modified: Option<SystemTime>, now: SystemTime, duration_ms: u64) -> OffsetDateTime {
    let ended = modified.filter(|m| *m <= now).unwrap_or(now);
    OffsetDateTime::from(ended) - Duration::milliseconds(duration_ms as i64)
}

/// The file being decoded, counting how far it has been read: the decode's progress for every
/// container (not all of them say how many frames they hold).
struct Reading {
    file: File,
    at: Arc<AtomicU64>,
}

impl Read for Reading {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.file.read(buf)?;
        self.at.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl Seek for Reading {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let at = self.file.seek(to)?;
        self.at.store(at, Ordering::Relaxed);
        Ok(at)
    }
}

impl MediaSource for Reading {
    fn is_seekable(&self) -> bool {
        self.file.is_seekable()
    }

    fn byte_len(&self) -> Option<u64> {
        self.file.byte_len()
    }
}

/// Decodes the first audio track of `path` into `out`, a 16 kHz mono 16-bit WAV. Returns its
/// length in samples. A corrupt frame is skipped, as players do. `progress`: the share of the file
/// read so far, in percent, when it changes.
fn decode(path: &Path, out: &Path, mut progress: impl FnMut(u8)) -> Result<u64, String> {
    let file = File::open(path).map_err(|e| format!("파일을 열 수 없습니다: {e}"))?;
    let size = file.metadata().map_or(0, |m| m.len()).max(1);
    let at = Arc::new(AtomicU64::new(0));
    let file = Reading {
        file,
        at: at.clone(),
    };
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            MediaSourceStream::new(Box::new(file), Default::default()),
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|_| UNSUPPORTED.to_string())?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("오디오 트랙이 없는 파일입니다")?;
    let track_id = track.id;
    // Refused before decoding when the container says how long it is.
    let params = &track.codec_params;
    if let (Some(frames), Some(rate)) = (params.n_frames, params.sample_rate)
        && frames / u64::from(rate.max(1)) > MAX_SAMPLES / u64::from(SPEC.sample_rate)
    {
        return Err(TOO_LONG.into());
    }
    let mut decoder = symphonia::default::get_codecs()
        .make(params, &DecoderOptions::default())
        .map_err(|_| UNSUPPORTED_CODEC.to_string())?;

    let mut wav = hound::WavWriter::create(out, SPEC)
        .map_err(|e| format!("녹음 파일을 만들 수 없습니다: {e}"))?;
    // Made from the decoded audio, not the container, which may not say its rate or channels; made
    // again if they change midway.
    let mut pipe: Option<(SignalSpec, Pipe)> = None;
    let mut interleaved: Option<SampleBuffer<f32>> = None;
    let mut mono = Vec::new();
    let mut written = 0u64;
    let mut told = None;
    loop {
        let percent = (at.load(Ordering::Relaxed).min(size) * 100 / size) as u8;
        if told != Some(percent) {
            told = Some(percent);
            progress(percent);
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            // The end of the file; a chained Ogg stream ends with its first part.
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("파일을 끝까지 읽지 못했습니다: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let audio = match decoder.decode(&packet) {
            Ok(audio) => audio,
            // A corrupt frame: symphonia reports a bitstream overrun as an I/O error.
            Err(Error::DecodeError(_) | Error::IoError(_)) => continue,
            Err(e) => return Err(format!("오디오를 풀지 못했습니다: {e}")),
        };
        let spec = *audio.spec();
        let pipe = match &mut pipe {
            Some((made_for, pipe)) if *made_for == spec => pipe,
            _ => {
                &mut pipe
                    .insert((spec, Pipe::new(spec.rate, spec.channels.count() as u16)?))
                    .1
            }
        };
        let needed = audio.capacity() * spec.channels.count();
        let buffer = match &mut interleaved {
            Some(buffer) if buffer.capacity() >= needed => buffer,
            _ => interleaved.insert(SampleBuffer::new(audio.capacity() as u64, spec)),
        };
        buffer.copy_interleaved_ref(audio);
        mono.clear();
        pipe.push(buffer.samples(), &mut mono);
        written += mono.len() as u64;
        if written > MAX_SAMPLES {
            return Err(TOO_LONG.into());
        }
        for sample in &mono {
            // As the recorder writes them (writer.rs).
            wav.write_sample((sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
                .map_err(|e| format!("녹음 파일을 쓰지 못했습니다: {e}"))?;
        }
    }
    wav.finalize()
        .map_err(|e| format!("녹음 파일을 쓰지 못했습니다: {e}"))?;
    if told != Some(100) {
        progress(100);
    }
    if written == 0 {
        return Err("파일에서 소리를 읽지 못했습니다".into());
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 44.1 kHz stereo WAV of `secs` seconds: a 440 Hz tone on the left, silence on the right.
    fn tone(path: &Path, secs: u32) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..44_100 * secs {
            let t = i as f32 / 44_100.0;
            w.write_sample(((t * 440.0 * std::f32::consts::TAU).sin() * 16_000.0) as i16)
                .unwrap();
            w.write_sample(0i16).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn a_stereo_wav_becomes_16_khz_mono_of_the_same_length() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, out) = (tmp.path().join("회의.wav"), tmp.path().join("mic.wav"));
        tone(&src, 3);
        let mut told = Vec::new();
        let samples = decode(&src, &out, |p| told.push(p)).unwrap();
        // How much of the file is read: rising, each value once, to the end.
        assert!(told.windows(2).all(|w| w[0] < w[1]), "{told:?}");
        assert_eq!((told.first(), told.last()), (Some(&0), Some(&100)));
        assert!(told.len() > 2, "{told:?}");
        // The resampler holds back less than one 1024-frame chunk at the end.
        assert!((47_000..=48_000).contains(&samples), "{samples}");
        let reader = hound::WavReader::open(&out).unwrap();
        assert_eq!(reader.spec(), SPEC);
        assert_eq!(u64::from(reader.duration()), samples);
        // The tone is there, at half its level: the silent right channel is averaged in.
        let peak = reader
            .into_samples::<i16>()
            .map(|s| s.unwrap().unsigned_abs())
            .max()
            .unwrap();
        assert!((7_000..=9_000).contains(&peak), "{peak}");
    }

    #[test]
    fn a_file_that_is_not_audio_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, out) = (tmp.path().join("notes.mp3"), tmp.path().join("mic.wav"));
        std::fs::write(&src, "회의 메모입니다").unwrap();
        assert_eq!(decode(&src, &out, |_| {}).unwrap_err(), UNSUPPORTED);
    }

    #[test]
    fn the_meeting_starts_its_length_before_the_file_was_saved() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
        let saved = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(4_000);
        let at = |modified| started(modified, now, 60_000).unix_timestamp();
        assert_eq!(at(Some(saved)), 3_940);
        // Unknown, or later than now (a clock off): it ended now.
        assert_eq!(at(None), 9_940);
        assert_eq!(at(Some(now + std::time::Duration::from_secs(60))), 9_940);
    }

    #[test]
    fn an_upload_is_a_recorded_meeting_with_its_audio() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("meetings")).unwrap();
        let src = tmp.path().join("주간 회의.wav");
        tone(&src, 2);
        let meta = import(&store, &src, true, |_| {}).unwrap();
        assert_eq!(meta.status, crate::store::Status::Recorded);
        assert!(meta.summarize_local);
        assert!(
            (1_950..=2_000).contains(&meta.duration_ms),
            "{}",
            meta.duration_ms
        );
        let dir = store.dir_for(&meta.id).unwrap();
        assert!(dir.join("mic.wav").is_file());
        assert!(!dir.join("system.wav").exists());
        assert_eq!(
            crate::audio::mix::recording_len(&dir).unwrap() / 16,
            meta.duration_ms
        );
        assert_eq!(store.list(), [meta]);
        // A failed upload leaves nothing behind.
        std::fs::write(&src, "x").unwrap();
        assert!(import(&store, &src, false, |_| {}).is_err());
        assert_eq!(
            std::fs::read_dir(tmp.path().join("meetings"))
                .unwrap()
                .count(),
            1
        );
    }
}
