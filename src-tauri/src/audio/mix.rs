//! Gating mix (architecture 4.5, TRN-02). While the system track carries sound, the mic track is
//! turned down, so a remote voice the speakers leak back into the mic is not transcribed twice.
//! Both WAV files are read a window at a time: a 4 h recording is never loaded whole.

use super::SPEC;
use crate::store::{self, Status, Store};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// 20 ms at 16 kHz.
const FRAME: usize = 320;
/// A system frame louder than -45 dBFS RMS turns the mic down. [S4]
const SYSTEM_ACTIVE_RMS: f32 = 0.005_623_4;
/// -20 dB. [S4]
const DUCKED_GAIN: f32 = 0.1;
/// One-pole time constants in samples: 5 ms to turn down, 300 ms to come back. [S4]
const ATTACK_SAMPLES: f32 = 5.0 * 16.0;
const RELEASE_SAMPLES: f32 = 300.0 * 16.0;
/// The soft limiter is linear up to here, then bends toward full scale without reaching it.
const KNEE: f32 = 0.5;

/// The mic gain, smoothed across frames.
pub struct Gate {
    gain: f32,
    attack: f32,
    release: f32,
}

impl Gate {
    pub fn new() -> Self {
        Self {
            gain: 1.0,
            attack: (-1.0 / ATTACK_SAMPLES).exp(),
            release: (-1.0 / RELEASE_SAMPLES).exp(),
        }
    }

    /// `mic = limit(mic × gain + system)`, in place. Both slices have the same length.
    pub fn mix(&mut self, mic: &mut [f32], system: &[f32]) {
        for (mic, system) in mic.chunks_mut(FRAME).zip(system.chunks(FRAME)) {
            let power = system.iter().map(|s| s * s).sum::<f32>() / system.len() as f32;
            let target = if power.sqrt() > SYSTEM_ACTIVE_RMS {
                DUCKED_GAIN
            } else {
                1.0
            };
            for (m, s) in mic.iter_mut().zip(system) {
                let coef = if target < self.gain {
                    self.attack
                } else {
                    self.release
                };
                self.gain = target + (self.gain - target) * coef;
                *m = limit(*m * self.gain + s);
            }
        }
    }
}

/// Linear below the knee; above it a tanh curve that never passes full scale, so no clipping.
fn limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    x.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh())
}

fn open(path: &Path) -> Result<hound::WavReader<std::io::BufReader<std::fs::File>>, String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let reader =
        hound::WavReader::open(path).map_err(|e| format!("{name}을(를) 열 수 없습니다: {e}"))?;
    if reader.spec() != SPEC {
        return Err(format!("{name}이(가) 16kHz 모노 16-bit WAV가 아닙니다"));
    }
    Ok(reader)
}

/// The recording's length in samples: the shorter track. `meta.json`'s `duration_ms` is not used
/// (architecture 5.2): after a write error or a crash the WAV files hold less than that. An upload
/// has `mic.wav` alone (REC-12).
pub fn recording_len(dir: &Path) -> Result<u64, String> {
    let mic = open(&dir.join("mic.wav"))?.duration();
    let system = dir.join("system.wav");
    if !system.exists() {
        return Ok(mic.into());
    }
    Ok(mic.min(open(&system)?.duration()).into())
}

/// `len` samples from `start`, fewer at the end of the file.
fn read(path: &Path, start: u64, len: usize) -> Result<Vec<f32>, String> {
    let mut reader = open(path)?;
    let start = u32::try_from(start).map_err(|_| "녹음 위치가 너무 큽니다".to_string())?;
    if start >= reader.duration() {
        return Ok(Vec::new());
    }
    reader.seek(start).map_err(|e| e.to_string())?;
    reader
        .samples::<i16>()
        .take(len)
        .map(|s| {
            s.map(|s| f32::from(s) / 32_768.0)
                .map_err(|e| e.to_string())
        })
        .collect()
}

/// Both tracks for `[start, start + len)` samples, gated and mixed in memory (never written). An
/// upload's one track as it is.
pub fn mixed(dir: &Path, start: u64, len: usize) -> Result<Vec<f32>, String> {
    let mut mic = read(&dir.join("mic.wav"), start, len)?;
    let system = dir.join("system.wav");
    if !system.exists() {
        return Ok(mic);
    }
    let system = read(&system, start, len)?;
    let len = mic.len().min(system.len());
    mic.truncate(len);
    Gate::new().mix(&mut mic, &system[..len]);
    Ok(mic)
}

/// Plan 5.4-2: 10 minutes of the export at a time.
const EXPORT_WINDOW: u64 = 16_000 * 600;
const MB: u64 = 1_000_000;
/// Plan 5.4-4: how an export ends when its meeting is deleted meanwhile.
pub const DELETED: &str = "회의가 삭제되어 저장하지 못했습니다";

/// Plan 5.4-1: what can be saved. A recording is still being written; a newer app's meeting may
/// not be ours to read.
pub fn exportable(status: Status) -> Result<(), String> {
    match status {
        Status::Deleting => Err(DELETED.into()),
        Status::Recording | Status::Unknown => {
            Err("이 회의는 녹음 파일을 저장할 수 없습니다".into())
        }
        _ => Ok(()),
    }
}

/// Plan 5.4-4: the exports running now, by meeting. A delete stops its meeting's and waits for it,
/// so no WAV of the meeting is open when its folder goes (Windows would refuse).
#[derive(Default)]
pub struct Exports {
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
    ended: Condvar,
}

/// A meeting's place in `running`, given up when dropped.
struct Running<'a>(&'a Exports, &'a str);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.running.lock().unwrap().remove(self.1);
        self.0.ended.notify_all();
    }
}

impl Exports {
    /// `export_recording`: the meeting as one 16 kHz mono WAV at `to`, the sound transcription hears
    /// (`mixed`). Written under a temporary name in `to`'s folder, then renamed; nothing is left
    /// behind when it fails.
    pub fn export(&self, store: &Store, id: &str, to: &Path) -> Result<(), String> {
        self.export_with(store, id, to, mixed)
    }

    fn export_with(
        &self,
        store: &Store,
        id: &str,
        to: &Path,
        read: impl Fn(&Path, u64, usize) -> Result<Vec<f32>, String>,
    ) -> Result<(), String> {
        let dir = store.dir_for(id)?;
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut running = self.running.lock().unwrap();
            if running.contains_key(id) {
                return Err("이미 이 회의의 녹음 파일을 저장하고 있습니다".into());
            }
            running.insert(id.to_string(), stop.clone());
        }
        let _running = Running(self, id);
        // Checked once in `running`: a delete from now on stops it, and one before has written
        // `deleting` (or removed the meeting).
        exportable(
            store
                .with_lock(id, |m| m.status)
                .unwrap_or(Status::Deleting),
        )?;
        let len = recording_len(&dir)?;
        let need = len * 2 + 44;
        if let Some(folder) = to.parent()
            && let Ok(free) = super::free_space(folder)
        {
            room(need, free)?;
        }
        let mut temp = to.as_os_str().to_owned();
        temp.push(".part");
        let temp = PathBuf::from(temp);
        let written = write(&dir, &temp, len, &stop, read).and_then(|()| {
            store::rename(&temp, to).map_err(|e| format!("녹음 파일을 저장하지 못했습니다: {e}"))
        });
        if written.is_err() {
            let _ = fs::remove_file(&temp);
        }
        written
    }

    /// The delete's step 2 (architecture 8.3): stops the meeting's export and returns once it has
    /// ended.
    pub fn cancel(&self, id: &str) {
        let mut running = self.running.lock().unwrap();
        if let Some(stop) = running.get(id) {
            stop.store(true, Ordering::SeqCst);
        }
        while running.contains_key(id) {
            running = self.ended.wait(running).unwrap();
        }
    }
}

/// Plan 5.4-3: refused up front with the numbers, rather than a full disk halfway.
fn room(need: u64, free: u64) -> Result<(), String> {
    if free >= need {
        return Ok(());
    }
    Err(format!(
        "필요 약 {}MB, 남은 공간 {}MB. 다른 폴더를 골라 주세요",
        (need + MB / 2) / MB,
        free / MB
    ))
}

/// `len` samples of the meeting into `temp`, a window at a time, looking at `stop` before each.
fn write(
    dir: &Path,
    temp: &Path,
    len: u64,
    stop: &AtomicBool,
    read: impl Fn(&Path, u64, usize) -> Result<Vec<f32>, String>,
) -> Result<(), String> {
    let failed = |e: hound::Error| format!("녹음 파일을 쓰지 못했습니다: {e}");
    let mut wav = hound::WavWriter::create(temp, SPEC).map_err(failed)?;
    let mut start = 0;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err(DELETED.into());
        }
        if start >= len {
            break;
        }
        let samples = read(dir, start, (len - start).min(EXPORT_WINDOW) as usize)?;
        if samples.is_empty() {
            break;
        }
        for s in samples {
            // Saturates, and gives an upload's samples back exactly.
            wav.write_sample((s * 32_768.0) as i16).map_err(failed)?;
        }
        start += EXPORT_WINDOW;
    }
    wav.finalize().map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mic gain sample by sample: a constant 0.1 mic under a ±0.05 square wave (-26 dBFS)
    /// while `loud`, digital silence otherwise. Everything stays below the knee, so
    /// `gain = (out - system) / 0.1`.
    fn gains(loud: impl Fn(usize) -> bool, len: usize) -> Vec<f32> {
        let system: Vec<f32> = (0..len)
            .map(|i| match (loud(i), i % 2) {
                (false, _) => 0.0,
                (true, 0) => 0.05,
                (true, _) => -0.05,
            })
            .collect();
        let mut mic = vec![0.1; len];
        Gate::new().mix(&mut mic, &system);
        mic.iter()
            .zip(&system)
            .map(|(o, s)| (o - s) / 0.1)
            .collect()
    }

    #[test]
    fn quiet_system_leaves_the_mic_alone() {
        // -50 dBFS on the system track: below the threshold.
        let system = vec![0.003; 16_000];
        let mut mic: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
        let expected: Vec<f32> = mic.iter().zip(&system).map(|(m, s)| m + s).collect();
        Gate::new().mix(&mut mic, &system);
        assert_eq!(mic, expected);
    }

    #[test]
    fn loud_system_turns_the_mic_down_20_db_within_the_attack() {
        let g = gains(|_| true, 16_000);
        // One-pole: 63% of the way down after 5 ms (80 samples), all but 2% after 20 ms.
        let after_attack = 1.0 - 0.9 * 0.632;
        assert!((g[79] - after_attack).abs() < 0.01, "{}", g[79]);
        assert!(g[319] < 0.12, "{}", g[319]);
        assert!(g[16_000 - 1] < 0.101 && g[16_000 - 1] > 0.099);
    }

    #[test]
    fn mic_comes_back_over_the_release() {
        // 1 s loud, then quiet: 300 ms later the gain is 63% of the way back to 1.
        let g = gains(|i| i < 16_000, 48_000);
        let quiet_at = 16_000;
        let after_release = 0.1 + 0.9 * 0.632;
        let at_300ms = g[quiet_at + 4_800 - 1];
        assert!((at_300ms - after_release).abs() < 0.01, "{at_300ms}");
        assert!(g[48_000 - 1] > 0.99);
    }

    #[test]
    fn limiter_never_clips_and_keeps_quiet_signals() {
        assert_eq!(limit(0.3), 0.3);
        assert_eq!(limit(-0.5), -0.5);
        for x in [0.6, 1.0, 2.0, 10.0] {
            let y = limit(x);
            assert!(y > KNEE && y <= 1.0, "{x} -> {y}");
            assert_eq!(limit(-x), -y);
        }
        assert!(limit(0.9) < limit(1.2));
        // Full-scale on both tracks.
        let mut mic = vec![1.0; 640];
        Gate::new().mix(&mut mic, &[1.0; 640]);
        assert!(mic.iter().all(|&x| x <= 1.0));
    }

    fn wav(path: &Path, samples: impl Iterator<Item = i16>) {
        let mut w = hound::WavWriter::create(path, SPEC).unwrap();
        for s in samples {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn reads_only_the_window_and_uses_the_shorter_track() {
        let tmp = tempfile::tempdir().unwrap();
        // Mic counts up; the system track is silent and one second shorter.
        wav(
            &tmp.path().join("mic.wav"),
            (0..48_000).map(|i| (i % 1000) as i16),
        );
        wav(
            &tmp.path().join("system.wav"),
            std::iter::repeat_n(0, 32_000),
        );

        assert_eq!(recording_len(tmp.path()).unwrap(), 32_000);
        let window = mixed(tmp.path(), 16_000, 160_000).unwrap();
        assert_eq!(window.len(), 16_000);
        assert_eq!(window[5], 5.0 / 32_768.0);
        assert!(mixed(tmp.path(), 40_000, 100).unwrap().is_empty());
    }

    /// A meeting in `status` with `mic` (and `system`, when given) as its tracks.
    fn meeting(store: &Store, status: Status, mic: &[i16], system: Option<&[i16]>) -> String {
        let id = store
            .create("", false, crate::transcribe::lang::default_list())
            .unwrap()
            .id;
        let dir = store.dir_for(&id).unwrap();
        wav(&dir.join("mic.wav"), mic.iter().copied());
        if let Some(system) = system {
            wav(&dir.join("system.wav"), system.iter().copied());
        }
        store
            .update(&id, |m| {
                m.status = status;
                Ok(())
            })
            .unwrap();
        id
    }

    fn samples(path: &Path) -> Vec<i16> {
        let reader = hound::WavReader::open(path).unwrap();
        assert_eq!(reader.spec(), SPEC);
        reader.into_samples().map(Result::unwrap).collect()
    }

    /// Plan 5.4-2: what transcription hears, the shorter track long; an upload's one track as it is.
    #[test]
    fn exports_a_wav_of_the_mix_or_of_the_one_track() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("meetings")).unwrap();
        let exports = Exports::default();
        let mic: Vec<i16> = (0..20_000)
            .map(|i| ((i % 200) * 50 - 5_000) as i16)
            .collect();
        let system: Vec<i16> = (0..16_000)
            .map(|i| ((i % 40) * 400 - 8_000) as i16)
            .collect();

        let both = meeting(&store, Status::Sent, &mic, Some(&system));
        let to = tmp.path().join("회의.wav");
        exports.export(&store, &both, &to).unwrap();
        let expected: Vec<i16> = mixed(&store.dir_for(&both).unwrap(), 0, 16_000)
            .unwrap()
            .iter()
            .map(|s| (s * 32_768.0) as i16)
            .collect();
        assert_eq!(samples(&to), expected);

        let one = meeting(&store, Status::Recorded, &mic, None);
        exports.export(&store, &one, &to).unwrap();
        assert_eq!(samples(&to), mic);
        assert_eq!(
            fs::read_dir(tmp.path()).unwrap().count(),
            2,
            "a temp file was left"
        );
    }

    #[test]
    fn without_room_it_is_refused_with_the_numbers() {
        // An hour: 16 kHz × 2 bytes × 3,600 s.
        assert_eq!(
            room(115_200_044, 40_400_000).unwrap_err(),
            "필요 약 115MB, 남은 공간 40MB. 다른 폴더를 골라 주세요"
        );
        assert!(room(1_000, 1_000).is_ok());
    }

    /// Plan 5.4-1, 5.7-5: through the store's id check, and never while recording or deleting,
    /// nor twice at once.
    #[test]
    fn bad_ids_recordings_and_deleted_meetings_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("meetings")).unwrap();
        let exports = Exports::default();
        let to = tmp.path().join("out.wav");
        for id in ["../meetings", "", "a/b"] {
            assert!(
                exports
                    .export(&store, id, &to)
                    .unwrap_err()
                    .contains("잘못된 회의 id")
            );
        }
        let recording = meeting(&store, Status::Recording, &[0; 10], None);
        assert!(exports.export(&store, &recording, &to).is_err());
        let deleting = meeting(&store, Status::Deleting, &[0; 10], None);
        assert_eq!(exports.export(&store, &deleting, &to).unwrap_err(), DELETED);
        assert_eq!(
            exports
                .export(&store, "2026-01-01T0000-0000", &to)
                .unwrap_err(),
            DELETED
        );
        let busy = meeting(&store, Status::Review, &[0; 10], None);
        exports
            .running
            .lock()
            .unwrap()
            .insert(busy.clone(), Arc::default());
        assert!(exports.export(&store, &busy, &to).is_err());
        assert!(!to.exists());
    }

    /// Plan 5.4-4, 5.4-7: the delete stops the export between two windows, waits for it, and
    /// removes the meeting; the export says why and leaves no file.
    #[test]
    fn a_delete_during_an_export_stops_it_and_leaves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(tmp.path().join("meetings")).unwrap());
        let exports = Arc::new(Exports::default());
        let id = meeting(&store, Status::Review, &[1; 1_000], None);
        let to = tmp.path().join("out.wav");
        let delete = Mutex::new(None);
        // The first window starts the delete, and is read once it has asked the export to stop.
        let read = |dir: &Path, start, len| {
            let (s, e, i) = (store.clone(), exports.clone(), id.clone());
            *delete.lock().unwrap() = Some(std::thread::spawn(move || {
                s.delete_with(&i, || e.cancel(&i))
            }));
            while !exports.running.lock().unwrap()[&id].load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            mixed(dir, start, len)
        };

        let exported = exports.export_with(&store, &id, &to, read);

        assert_eq!(exported.unwrap_err(), DELETED);
        let deleted = delete.lock().unwrap().take().unwrap().join().unwrap();
        assert_eq!(deleted, Ok(()));
        assert!(!store.dir_for(&id).unwrap().exists());
        assert_eq!(
            fs::read_dir(tmp.path()).unwrap().count(),
            1,
            "only meetings/ is left"
        );
    }
}
