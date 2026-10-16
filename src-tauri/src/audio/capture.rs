//! Capture thread (architecture 4.1, 4.3, 4.4): opens the microphone and system-audio streams,
//! checks the devices every second and reopens a stream whose device failed or changed.
//!
//! Streams are only built here, never on an IPC or UI thread: the first-run permission prompt
//! blocks `build_input_stream` inside the HAL until the recorder answers it.

use super::writer::Source;
use super::{Emit, Event, Shared, TAP_UID_PREFIX, Track, Warning, clock};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, DeviceId, FromSample, Host, SampleFormat, SizedSample, StreamConfig};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// cpal cannot subscribe to default-device changes, so they are polled (REC-07).
const POLL: Duration = Duration::from_secs(1);
/// A device that failed to open this many times in a row is retried only every `BACKOFF`.
/// The retries before that come with the 1 s poll, inside REC-07's 2 s.
const FAILS_BEFORE_BACKOFF: u32 = 3;
const BACKOFF: Duration = Duration::from_secs(5);

/// When to try again to open a device that failed to open. On macOS every try creates and
/// destroys a tap, so a device that keeps failing is not retried every second.
#[derive(Default)]
struct Retry {
    device: Option<DeviceId>,
    failures: u32,
    not_before: Option<Instant>,
}

impl Retry {
    fn waiting(&self, wanted: &Option<DeviceId>, now: Instant) -> bool {
        self.device == *wanted && self.not_before.is_some_and(|t| now < t)
    }

    fn failed(&mut self, wanted: Option<DeviceId>, now: Instant) {
        self.failures = if self.device == wanted {
            self.failures + 1
        } else {
            1
        };
        self.device = wanted;
        self.not_before = (self.failures >= FAILS_BEFORE_BACKOFF).then(|| now + BACKOFF);
    }
}

pub(super) struct Input {
    track: Track,
    /// Device id from settings. Ignored for the macOS system track (see `run`).
    pinned: Option<String>,
    /// The device being recorded. For the macOS system track: the default output device the tap
    /// was made for.
    device: Option<DeviceId>,
    // Fields drop in order: the stream stops before its tap is destroyed.
    stream: Option<cpal::Stream>,
    #[cfg(target_os = "macos")]
    tap: Option<super::macos_tap::SystemTap>,
    /// Set by the stream's error callback.
    failed: Arc<AtomicBool>,
    /// The device opened last, to tell a reconnect from a change.
    last_opened: Option<DeviceId>,
    retry: Retry,
    last_message: String,
}

/// A device for the settings' audio tab. `id` is the `DeviceId` string that `Input::refresh`
/// parses back when the device is pinned.
#[derive(Serialize)]
pub struct AudioDevice {
    id: String,
    name: String,
}

#[derive(Serialize)]
pub struct AudioDevices {
    inputs: Vec<AudioDevice>,
    outputs: Vec<AudioDevice>,
}

/// Can block inside the OS audio API: call it on a blocking thread, never the IPC thread.
pub fn devices() -> Result<AudioDevices, String> {
    let host = cpal::default_host();
    let inputs = host.input_devices().map_err(|e| e.to_string())?;
    // Only Windows pins an output device (see `run`). An output error must not hide the mics.
    let outputs = if cfg!(target_os = "macos") {
        Vec::new()
    } else {
        host.output_devices().map(listed).unwrap_or_default()
    };
    Ok(AudioDevices {
        inputs: listed(inputs),
        outputs,
    })
}

fn listed(devices: impl Iterator<Item = Device>) -> Vec<AudioDevice> {
    devices
        .filter_map(|device| {
            let id = device.id().ok().filter(|id| !is_own_tap(id))?;
            Some(AudioDevice {
                id: id.to_string(),
                name: device.description().ok()?.name().to_string(),
            })
        })
        .collect()
}

/// Our private aggregate device (macos_tap.rs) shows up as an input while recording.
fn is_own_tap(id: &DeviceId) -> bool {
    id.id().starts_with(TAP_UID_PREFIX)
}

pub fn run(
    shared: &Shared,
    sources: Sender<Source>,
    input_device: Option<String>,
    output_device: Option<String>,
    emit: Emit,
) {
    // The macOS global tap records every output device, so a pinned one means nothing there.
    let output_device = output_device.filter(|_| !cfg!(target_os = "macos"));
    let host = cpal::default_host();
    let mut inputs = [
        Input::new(Track::Mic, input_device),
        Input::new(Track::System, output_device),
    ];
    let mut next_poll = Instant::now();
    loop {
        if Instant::now() >= next_poll {
            for input in &mut inputs {
                input.refresh(&host, &sources, shared, &emit);
                // Opening can block on a permission prompt for a long time. A recording that
                // ended meanwhile gets no new stream and no new tap.
                if shared.stop.load(Ordering::SeqCst) {
                    return;
                }
            }
            next_poll = Instant::now() + POLL;
        }
        std::thread::sleep(Duration::from_millis(100));
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
    }
}

impl Input {
    pub(super) fn new(track: Track, pinned: Option<String>) -> Self {
        Self {
            track,
            pinned,
            device: None,
            stream: None,
            #[cfg(target_os = "macos")]
            tap: None,
            failed: Arc::default(),
            last_opened: None,
            retry: Retry::default(),
            last_message: String::new(),
        }
    }

    fn name(&self) -> &'static str {
        match self.track {
            Track::Mic => "마이크",
            Track::System => "시스템 오디오 출력 장치",
        }
    }

    /// Repeats of the same message are not sent again, and nothing is sent after the stop.
    fn warn(&mut self, emit: &Emit, shared: &Shared, message: String) {
        if shared.stop.load(Ordering::SeqCst) || message == self.last_message {
            return;
        }
        self.last_message = message.clone();
        let kind = match self.track {
            Track::Mic => "mic_device",
            Track::System => "system_device",
        };
        emit(Event::Warning(Warning { kind, message }));
    }

    /// Reopens the stream when it failed, when the wanted device changed, or when a device
    /// appeared after none. With no device the writer fills the track with silence (4.3).
    fn refresh(&mut self, host: &Host, sources: &Sender<Source>, shared: &Shared, emit: &Emit) {
        let default = || match self.track {
            Track::Mic => host.default_input_device(),
            Track::System => host.default_output_device(),
        };
        let pinned_id = self
            .pinned
            .as_deref()
            .and_then(|p| p.parse::<DeviceId>().ok());
        let pinned_device = pinned_id.as_ref().and_then(|id| host.device_by_id(id));
        let pinned_missing = pinned_id.is_some() && pinned_device.is_none();
        let wanted = pinned_device.or_else(default);
        let wanted_id = wanted.as_ref().and_then(|d| d.id().ok());

        let healthy = self.stream.is_some() && !self.failed.load(Ordering::SeqCst);
        if (healthy && wanted_id == self.device) || self.retry.waiting(&wanted_id, Instant::now()) {
            return;
        }
        self.stream = None;
        #[cfg(target_os = "macos")]
        {
            self.tap = None;
        }
        self.device = None;
        let Some(device) = wanted else {
            let message = format!(
                "{}를 찾을 수 없습니다. 연결될 때까지 이 트랙은 무음으로 녹음합니다",
                self.name()
            );
            return self.warn(emit, shared, message);
        };
        if let Err(e) = self.open(&device, sources, shared) {
            self.retry.failed(wanted_id, Instant::now());
            let message = format!(
                "{}를 열 수 없습니다 ({e}). 이 트랙은 무음으로 녹음하며 다시 시도합니다",
                self.name()
            );
            return self.warn(emit, shared, message);
        }
        self.retry = Retry::default();
        let label = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_default();
        let name = self.name();
        let message = if pinned_missing {
            Some(format!(
                "설정한 {name}를 찾을 수 없어 기본 장치({label})로 녹음합니다"
            ))
        } else if self.last_opened.is_some() && self.last_opened == wanted_id {
            Some(format!("{name}를 다시 연결했습니다: {label}"))
        } else if self.last_opened.is_some() {
            Some(format!("{name}가 바뀌었습니다: {label}"))
        } else if !self.last_message.is_empty() {
            // Replaces a standing "not found" or "cannot open" banner.
            Some(format!("{name}를 연결했습니다: {label}"))
        } else {
            None
        };
        self.device = wanted_id.clone();
        self.last_opened = wanted_id;
        if let Some(message) = message {
            self.warn(emit, shared, message);
        }
    }

    fn open(
        &mut self,
        device: &Device,
        sources: &Sender<Source>,
        shared: &Shared,
    ) -> Result<(), String> {
        self.failed = Arc::default();
        // Until the stream runs: a permission prompt can hold creating the tap, or building or
        // starting the stream (4.4).
        shared
            .opening_since_ms
            .store(clock::now().awake_ms.max(1), Ordering::SeqCst);
        let opened = self.open_stream(device, sources);
        shared.opening_since_ms.store(0, Ordering::SeqCst);
        opened
    }

    /// Also used by the settings' audio checks (check.rs).
    pub(super) fn open_stream(
        &mut self,
        device: &Device,
        sources: &Sender<Source>,
    ) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        if self.track == Track::System {
            // A fresh tap per output device: cheap, and safe even though a global tap follows the
            // default device by itself (4.3).
            let tap = super::macos_tap::SystemTap::new()?;
            // The private aggregate is visible to this process, so cpal opens it as a plain input.
            let aggregate = cpal::default_host()
                .device_by_id(&DeviceId::new(cpal::HostId::CoreAudio, &tap.uid))
                .ok_or("시스템 오디오 장치를 찾을 수 없습니다")?;
            self.stream = Some(build(&aggregate, false, self.track, &self.failed, sources)?);
            self.tap = Some(tap);
            return Ok(());
        }
        // Windows: an input stream on an output device is WASAPI loopback.
        let loopback = self.track == Track::System;
        self.stream = Some(build(device, loopback, self.track, &self.failed, sources)?);
        Ok(())
    }
}

fn build(
    device: &Device,
    loopback: bool,
    track: Track,
    failed: &Arc<AtomicBool>,
    sources: &Sender<Source>,
) -> Result<cpal::Stream, String> {
    let config = if loopback {
        device.default_output_config()
    } else {
        device.default_input_config()
    }
    .map_err(|e| e.to_string())?;
    let (rate, channels) = (config.sample_rate(), config.channels());
    // 2 s of audio. The writer drains every 20 ms; this only fills up if it stalls.
    let (producer, samples) = rtrb::RingBuffer::new(rate as usize * usize::from(channels) * 2);
    let failed = failed.clone();
    let stream_config = config.config();
    let stream = match config.sample_format() {
        SampleFormat::F32 => stream::<f32>(device, stream_config, producer, failed),
        SampleFormat::I16 => stream::<i16>(device, stream_config, producer, failed),
        SampleFormat::I32 => stream::<i32>(device, stream_config, producer, failed),
        other => Err(format!("지원하지 않는 샘플 형식입니다: {other}")),
    }?;
    sources
        .send(Source {
            track,
            samples,
            rate,
            channels,
        })
        .map_err(|_| "녹음이 이미 끝났습니다")?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

fn stream<T: SizedSample>(
    device: &Device,
    config: StreamConfig,
    mut producer: rtrb::Producer<f32>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                // Real-time thread: no allocation, lock or I/O. Whole frames only, so the
                // writer's downmix never splits one; if the writer stalls, the rest is dropped
                // and the clock fills the gap.
                let slots = producer.slots();
                let n = data.len().min(slots - slots % channels);
                if let Ok(chunk) = producer.write_chunk_uninit(n) {
                    chunk.fill_from_iter(data[..n].iter().map(|s| s.to_sample::<f32>()));
                }
            },
            move |e| {
                // Rerouted streams and glitches keep running; anything else needs a reopen.
                use cpal::ErrorKind::{DeviceChanged, RealtimeDenied, Xrun};
                if !matches!(e.kind(), DeviceChanged | Xrun | RealtimeDenied) {
                    failed.store(true, Ordering::SeqCst);
                }
            },
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_that_keeps_failing_is_retried_less_often() {
        let id = |name: &str| Some(DeviceId::new(cpal::default_host().id(), name));
        let t0 = Instant::now();
        let mut retry = Retry::default();
        for n in 1..FAILS_BEFORE_BACKOFF {
            retry.failed(id("a"), t0);
            assert!(!retry.waiting(&id("a"), t0 + POLL), "retry {n}: next poll");
        }
        retry.failed(id("a"), t0);
        assert!(retry.waiting(&id("a"), t0 + POLL));
        assert!(!retry.waiting(&id("a"), t0 + BACKOFF));
        // A newly wanted device (a headset was plugged in) is tried at once.
        assert!(!retry.waiting(&id("b"), t0 + POLL));
        retry.failed(id("b"), t0);
        assert!(!retry.waiting(&id("b"), t0 + POLL), "its count starts over");
    }

    #[test]
    fn device_list_leaves_out_our_own_tap_and_ids_round_trip() {
        let host = cpal::default_host().id();
        let tap = DeviceId::new(host, format!("{TAP_UID_PREFIX}123.0"));
        let mic = DeviceId::new(host, "BuiltInMicrophoneDevice");
        assert!(is_own_tap(&tap));
        assert!(!is_own_tap(&mic));
        // What the audio tab stores is what `Input::refresh` parses, even with ':' in the UID.
        let usb = DeviceId::new(host, "AppleUSBAudioEngine:Vendor:Product:1234:1");
        for id in [mic, usb] {
            assert_eq!(id.to_string().parse::<DeviceId>().ok(), Some(id));
        }
    }
}
