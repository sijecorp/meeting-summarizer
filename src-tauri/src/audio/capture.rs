//! Capture thread (architecture 4.1, 4.3, 4.4): opens the microphone and system-audio streams,
//! checks the devices every second and reopens a stream whose device failed or changed.
//!
//! Streams are only built here, never on an IPC or UI thread: the first-run permission prompt
//! blocks `build_input_stream` inside the HAL until the recorder answers it.

use super::writer::Source;
use super::{Emit, Event, Shared, Track, Warning, clock};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, DeviceId, FromSample, Host, SampleFormat, SizedSample, StreamConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// cpal cannot subscribe to default-device changes, so they are polled (REC-07).
const POLL: Duration = Duration::from_secs(1);

struct Input {
    track: Track,
    /// The device being recorded. For the macOS system track: the default output device the tap
    /// was made for.
    device: Option<DeviceId>,
    // Fields drop in order: the stream stops before its tap is destroyed.
    stream: Option<cpal::Stream>,
    #[cfg(target_os = "macos")]
    tap: Option<super::macos_tap::SystemTap>,
    /// Set by the stream's error callback.
    failed: Arc<AtomicBool>,
    opened_once: bool,
    last_message: String,
}

pub fn run(
    shared: &Shared,
    sources: Sender<Source>,
    input_device: Option<String>,
    output_device: Option<String>,
    emit: Emit,
) {
    // The macOS global tap records every output device, so a pinned one means nothing there.
    let pinned_output = if cfg!(target_os = "macos") {
        None
    } else {
        output_device.as_deref()
    };
    let host = cpal::default_host();
    let mut mic = Input::new(Track::Mic);
    let mut system = Input::new(Track::System);
    let mut next_poll = Instant::now();
    while !shared.stop.load(Ordering::SeqCst) {
        if Instant::now() >= next_poll {
            mic.refresh(&host, input_device.as_deref(), &sources, shared, &emit);
            system.refresh(&host, pinned_output, &sources, shared, &emit);
            next_poll = Instant::now() + POLL;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl Input {
    fn new(track: Track) -> Self {
        Self {
            track,
            device: None,
            stream: None,
            #[cfg(target_os = "macos")]
            tap: None,
            failed: Arc::default(),
            opened_once: false,
            last_message: String::new(),
        }
    }

    fn name(&self) -> &'static str {
        match self.track {
            Track::Mic => "마이크",
            Track::System => "시스템 오디오 출력 장치",
        }
    }

    /// Repeats of the same message are not sent again.
    fn warn(&mut self, emit: &Emit, message: String) {
        if message != self.last_message {
            self.last_message = message.clone();
            let kind = match self.track {
                Track::Mic => "mic_device",
                Track::System => "system_device",
            };
            emit(Event::Warning(Warning { kind, message }));
        }
    }

    /// Reopens the stream when it failed, when the wanted device changed, or when a device
    /// appeared after none. With no device the writer fills the track with silence (4.3).
    fn refresh(
        &mut self,
        host: &Host,
        pinned: Option<&str>,
        sources: &Sender<Source>,
        shared: &Shared,
        emit: &Emit,
    ) {
        let default = || match self.track {
            Track::Mic => host.default_input_device(),
            Track::System => host.default_output_device(),
        };
        let pinned_id = pinned.and_then(|p| p.parse::<DeviceId>().ok());
        let pinned_device = pinned_id.as_ref().and_then(|id| host.device_by_id(id));
        let pinned_missing = pinned_id.is_some() && pinned_device.is_none();
        let wanted = pinned_device.or_else(default);
        let wanted_id = wanted.as_ref().and_then(|d| d.id().ok());

        let healthy = self.stream.is_some() && !self.failed.load(Ordering::SeqCst);
        if healthy && wanted_id == self.device {
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
            return self.warn(emit, message);
        };
        match self.open(&device, sources, shared) {
            Ok(()) => {
                self.device = wanted_id;
                let label = device
                    .description()
                    .map(|d| d.name().to_string())
                    .unwrap_or_default();
                if pinned_missing {
                    let message = format!(
                        "설정한 {}를 찾을 수 없어 기본 장치({label})로 녹음합니다",
                        self.name()
                    );
                    self.warn(emit, message);
                } else if self.opened_once || !self.last_message.is_empty() {
                    // Also replaces a standing "not found" banner.
                    let message = format!("{}가 바뀌었습니다: {label}", self.name());
                    self.warn(emit, message);
                }
                self.opened_once = true;
            }
            Err(e) => {
                let message = format!(
                    "{}를 열 수 없습니다 ({e}). 이 트랙은 무음으로 녹음하며 다시 시도합니다",
                    self.name()
                );
                self.warn(emit, message);
            }
        }
    }

    fn open(
        &mut self,
        device: &Device,
        sources: &Sender<Source>,
        shared: &Shared,
    ) -> Result<(), String> {
        self.failed = Arc::default();
        #[cfg(target_os = "macos")]
        if self.track == Track::System {
            // A fresh tap per output device: cheap, and safe even though a global tap follows the
            // default device by itself (4.3).
            let tap = super::macos_tap::SystemTap::new()?;
            // The private aggregate is visible to this process, so cpal opens it as a plain input.
            let aggregate = cpal::default_host()
                .device_by_id(&DeviceId::new(cpal::HostId::CoreAudio, &tap.uid))
                .ok_or("시스템 오디오 장치를 찾을 수 없습니다")?;
            self.stream = Some(build(
                &aggregate,
                false,
                self.track,
                &self.failed,
                sources,
                shared,
            )?);
            self.tap = Some(tap);
            return Ok(());
        }
        // Windows: an input stream on an output device is WASAPI loopback.
        let loopback = self.track == Track::System;
        self.stream = Some(build(
            device,
            loopback,
            self.track,
            &self.failed,
            sources,
            shared,
        )?);
        Ok(())
    }
}

fn build(
    device: &Device,
    loopback: bool,
    track: Track,
    failed: &Arc<AtomicBool>,
    sources: &Sender<Source>,
    shared: &Shared,
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
    // Until the stream runs: a permission prompt may hold building or starting it (4.4).
    shared
        .opening_since_ms
        .store(clock::now().awake_ms.max(1), Ordering::SeqCst);
    let opened = (|| {
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
    })();
    shared.opening_since_ms.store(0, Ordering::SeqCst);
    opened
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
