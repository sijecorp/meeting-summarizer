//! Gating mix (architecture 4.5, TRN-02). While the system track carries sound, the mic track is
//! turned down, so a remote voice the speakers leak back into the mic is not transcribed twice.
//! Both WAV files are read a window at a time: a 4 h recording is never loaded whole.

use super::SPEC;
use std::path::Path;

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
/// (architecture 5.2): after a write error or a crash the WAV files hold less than that.
pub fn recording_len(dir: &Path) -> Result<u64, String> {
    let mic = open(&dir.join("mic.wav"))?.duration();
    let system = open(&dir.join("system.wav"))?.duration();
    Ok(mic.min(system).into())
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

/// Both tracks for `[start, start + len)` samples, gated and mixed in memory (never written).
pub fn mixed(dir: &Path, start: u64, len: usize) -> Result<Vec<f32>, String> {
    let mut mic = read(&dir.join("mic.wav"), start, len)?;
    let system = read(&dir.join("system.wav"), start, len)?;
    let len = mic.len().min(system.len());
    mic.truncate(len);
    Gate::new().mix(&mut mic, &system[..len]);
    Ok(mic)
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
}
