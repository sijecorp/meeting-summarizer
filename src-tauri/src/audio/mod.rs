//! Recording (architecture 4). One recording at a time: a capture thread owns the device
//! streams, a writer thread turns them into `mic.wav` and `system.wav`.

mod capture;
mod clock;
#[cfg(target_os = "macos")]
mod macos_tap;
pub mod mix;
mod writer;

use crate::queue::Queue;
use crate::store::Store;
use serde::Serialize;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use tauri::{AppHandle, Emitter, Manager};

pub const SPEC: hound::WavSpec = hound::WavSpec {
    channels: 1,
    sample_rate: clock::RATE as u32,
    bits_per_sample: 16,
    sample_format: hound::SampleFormat::Int,
};
/// Both tracks: 16 kHz × 2 bytes × 2 (architecture 4.6).
const BYTES_PER_SEC: u64 = 64_000;
/// Below this, warn before recording how many minutes fit (REC-11).
const START_WARNING_BYTES: u64 = 1_500_000_000;
/// Starts the UID, and so the cpal device id, of our private aggregate device (macos_tap.rs).
const TAP_UID_PREFIX: &str = "com.sije.meeting-summarizer.tap.";

pub use capture::{AudioDevices, devices};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Mic,
    System,
}

/// `recording://level`, every 40 ms. RMS per track, 0..1, and the spectrum of the last 64 ms
/// per track (`writer::Spectrum`).
#[derive(Serialize, Clone, Debug)]
pub struct Level {
    pub elapsed_ms: u64,
    pub mic: f32,
    pub system: f32,
    pub mic_bands: [u8; writer::BANDS],
    pub system_bands: [u8; writer::BANDS],
    /// A stream has been opening for over a second: likely the first-run permission prompt (4.4).
    pub waiting_permission: bool,
}

/// `recording://warning`. A newer warning of the same kind replaces the older one.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Warning {
    pub kind: &'static str,
    pub message: String,
}

/// `recording://stopped`. `message` says why when the recording ended by itself.
#[derive(Serialize, Clone, Debug)]
pub struct Stopped {
    pub id: String,
    pub message: Option<String>,
}

pub enum Event {
    Level(Level),
    Warning(Warning),
    Stopped(Stopped),
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// Shared by `Recorder`, the capture thread and the writer thread.
#[derive(Default)]
pub struct Shared {
    pub stop: AtomicBool,
    /// Reference-clock ms when the capture thread started building a stream; 0 when not building.
    pub opening_since_ms: AtomicU64,
}

/// Free space on the volume holding `path`, for this user.
pub fn free_space(path: &Path) -> std::io::Result<u64> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(u64::from(stat.f_bavail) * stat.f_frsize)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut free = 0u64;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                path.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(free)
    }
}

/// What the recording screen needs when it opens: the meeting and the warnings still standing.
#[derive(Serialize, Clone, Debug)]
pub struct RecordingState {
    pub id: String,
    pub warnings: Vec<Warning>,
}

struct Active {
    id: String,
    shared: Arc<Shared>,
    writer: JoinHandle<()>,
    warnings: Arc<Mutex<Vec<Warning>>>,
}

#[derive(Default)]
pub struct Recorder {
    active: Mutex<Option<Active>>,
    /// Held by `stop` until the writer has finished, so a second stop (quit, delete) racing the
    /// first waits for `recorded` instead of writing over a recording still being closed, and a
    /// new `start` waits too.
    finishing: Mutex<()>,
}

impl Recorder {
    /// REC-01. Creates the meeting and both WAV files, then starts the threads. Opening the
    /// devices happens on the capture thread, so this returns before any permission prompt.
    /// `summarize_local` is the meeting's "로컬로 요약" to begin with (SUM-08).
    pub fn start(
        &self,
        app: &AppHandle,
        store: &Store,
        input_device: Option<String>,
        output_device: Option<String>,
        summarize_local: bool,
    ) -> Result<RecordingState, String> {
        // Waits for a `stop` still closing the last recording: its writer tells the queue the
        // recording is over, and that must come before this one says it has begun.
        let _finishing = self.finishing.lock().unwrap();
        let mut active = self.active.lock().unwrap();
        if active
            .as_ref()
            .is_some_and(|a| !a.shared.stop.load(Ordering::SeqCst))
        {
            return Err("이미 녹음 중입니다".into());
        }
        // A recording that ended by itself (4 hours, disk, sleep): let its writer finish first.
        if let Some(ended) = active.take()
            && ended.writer.join().is_err()
        {
            eprintln!("녹음 기록 스레드가 비정상 종료했습니다 ({})", ended.id);
        }
        let id = store.create("", summarize_local)?.id;
        let dir = store.dir_for(&id)?;
        // A second handle to each file lets the writer fsync after every 5 s flush.
        let open = |name: &str| {
            let created = || -> hound::Result<_> {
                let file = File::create(dir.join(name))?;
                let sync = file.try_clone()?;
                Ok((hound::WavWriter::new(BufWriter::new(file), SPEC)?, sync))
            };
            created().map_err(|e| format!("{name}을(를) 만들 수 없습니다: {e}"))
        };
        let (mic, system) = match open("mic.wav").and_then(|mic| Ok((mic, open("system.wav")?))) {
            Ok(files) => files,
            Err(e) => {
                let _ = store.delete(&id);
                return Err(e);
            }
        };

        let warnings = Arc::new(Mutex::new(Vec::new()));
        if let Ok(free) = free_space(&dir)
            && free < START_WARNING_BYTES
        {
            warnings.lock().unwrap().push(Warning {
                kind: "disk",
                message: format!(
                    "디스크 여유 공간이 부족합니다. 남은 공간으로 최대 약 {}분 녹음할 수 있습니다",
                    free / BYTES_PER_SEC / 60
                ),
            });
        }
        let emit = emitter(app.clone(), warnings.clone());
        let shared = Arc::new(Shared::default());
        let (sources, received) = mpsc::channel();
        let writer = writer::Writer::new(
            id.clone(),
            dir,
            mic,
            system,
            received,
            shared.clone(),
            emit.clone(),
            clock::now(),
        );

        // TRN-09: running transcriptions stop now. The writer thread ends the pause when it
        // finishes this recording.
        let queue = app.try_state::<Queue>();
        if let Some(queue) = &queue {
            queue.recording_started();
        }
        let handle = app.clone();
        let spawned = thread::Builder::new()
            .name("recording-writer".into())
            .spawn(move || {
                // Windows ties this to the calling thread, which lives as long as the recording.
                let _awake = keepawake::Builder::default()
                    .idle(true)
                    .sleep(true)
                    .reason("회의 녹음 중")
                    .app_name("회의 요약")
                    .app_reverse_domain("com.sije.meeting-summarizer")
                    .create()
                    .inspect_err(|e| eprintln!("절전 방지 실패: {e}"));
                writer::run(writer, &handle.state::<Store>());
                // TRN-01: the meeting is `recorded` now. Queue it, and let the transcriptions
                // the recording stopped go on (TRN-09).
                if let Some(queue) = handle.try_state::<Queue>() {
                    queue.recording_finished();
                }
            });
        // A meeting that never started recording is not kept.
        let writer = match spawned {
            Ok(writer) => writer,
            Err(e) => {
                // The unspawned closure dropped the WAV writers, so the files are closed.
                let _ = store.delete(&id);
                if let Some(queue) = &queue {
                    queue.recording_finished();
                }
                return Err(format!("녹음을 시작할 수 없습니다: {e}"));
            }
        };
        let capture_shared = shared.clone();
        let capture = thread::Builder::new()
            .name("recording-capture".into())
            .spawn(move || {
                capture::run(&capture_shared, sources, input_device, output_device, emit)
            });
        if let Err(e) = capture {
            // Let the writer close the files first, then remove the empty meeting.
            shared.stop.store(true, Ordering::SeqCst);
            let _ = writer.join();
            let _ = store.delete(&id);
            return Err(format!("녹음을 시작할 수 없습니다: {e}"));
        }

        let state = RecordingState {
            id: id.clone(),
            warnings: warnings.lock().unwrap().clone(),
        };
        *active = Some(Active {
            id,
            shared,
            writer,
            warnings,
        });
        Ok(state)
    }

    /// Stops the recording (only if it records `only`, when given) and waits until both WAV
    /// files are closed and the meeting is `recorded`. Returns the meeting id, also when the
    /// recording had already ended by itself.
    pub fn stop(&self, only: Option<&str>) -> Option<String> {
        let _finishing = self.finishing.lock().unwrap();
        let active = self
            .active
            .lock()
            .unwrap()
            .take_if(|a| only.is_none_or(|id| a.id == id))?;
        active.shared.stop.store(true, Ordering::SeqCst);
        if active.writer.join().is_err() {
            eprintln!("녹음 기록 스레드가 비정상 종료했습니다 ({})", active.id);
        }
        Some(active.id)
    }

    /// The recording in progress, if any.
    pub fn state(&self) -> Option<RecordingState> {
        let active = self.active.lock().unwrap();
        let active = active
            .as_ref()
            .filter(|a| !a.shared.stop.load(Ordering::SeqCst))?;
        Some(RecordingState {
            id: active.id.clone(),
            warnings: active.warnings.lock().unwrap().clone(),
        })
    }
}

/// Sends events to the webview and remembers standing warnings for `Recorder::state`.
fn emitter(app: AppHandle, warnings: Arc<Mutex<Vec<Warning>>>) -> Emit {
    Arc::new(move |event| {
        let sent = match event {
            Event::Level(level) => app.emit("recording://level", level),
            Event::Warning(warning) => {
                let mut standing = warnings.lock().unwrap();
                standing.retain(|w| w.kind != warning.kind);
                standing.push(warning.clone());
                app.emit("recording://warning", warning)
            }
            Event::Stopped(stopped) => app.emit("recording://stopped", stopped),
        };
        if let Err(e) = sent {
            eprintln!("녹음 이벤트 전송 실패: {e}");
        }
    })
}
