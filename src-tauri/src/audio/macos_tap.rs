//! System audio on macOS: a global mono Core Audio process tap inside a private aggregate device.
//!
//! cpal 0.18.2 opens its own tap only for output devices without input channels. On a headset,
//! which also has a microphone, it silently opens that microphone instead (architecture 4.3).
//! So we create the tap ourselves and hand cpal an ordinary input device. Needs macOS 14.2+.

use objc2::AnyThread;
use objc2_core_audio::{
    AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap, AudioObjectID,
    CATapDescription, CATapMuteBehavior, kAudioAggregateDeviceIsPrivateKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioSubTapDriftCompensationKey,
    kAudioSubTapUIDKey,
};
use objc2_core_foundation::{CFArray, CFBoolean, CFDictionary, CFRetained, CFString, CFType};
use objc2_foundation::{NSArray, NSString};
use std::ffi::CStr;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};

static INSTANCE: AtomicU32 = AtomicU32::new(0);

/// Tap + aggregate device. Drop the cpal stream reading the aggregate before this:
/// dropping destroys the aggregate first, then the tap.
pub struct SystemTap {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    /// The aggregate device's UID, which is also its cpal `DeviceId`.
    pub uid: String,
}

impl SystemTap {
    pub fn new() -> Result<Self, String> {
        // Every process except none: all system output, mixed to mono. Unmuted keeps it audible.
        let desc = unsafe {
            CATapDescription::initMonoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &NSArray::new(),
            )
        };
        unsafe {
            desc.setMuteBehavior(CATapMuteBehavior::Unmuted);
            desc.setPrivate(true);
            desc.setName(&NSString::from_str("회의 요약 시스템 오디오"));
        }
        let mut tap: AudioObjectID = 0;
        let status = unsafe { AudioHardwareCreateProcessTap(Some(&desc), &mut tap) };
        if status != 0 {
            return Err(format!(
                "시스템 오디오 탭을 만들 수 없습니다 (OSStatus {status})"
            ));
        }

        let uid = format!(
            "com.sije.meeting-summarizer.tap.{}.{}",
            std::process::id(),
            INSTANCE.fetch_add(1, Ordering::Relaxed)
        );
        let tap_uid = unsafe { desc.UUID().UUIDString() }.to_string();
        let mut aggregate: AudioObjectID = 0;
        let status = unsafe {
            AudioHardwareCreateAggregateDevice(
                aggregate_description(&tap_uid, &uid).as_opaque(),
                NonNull::from(&mut aggregate),
            )
        };
        if status != 0 {
            unsafe { AudioHardwareDestroyProcessTap(tap) };
            return Err(format!(
                "시스템 오디오 장치를 만들 수 없습니다 (OSStatus {status})"
            ));
        }
        Ok(Self {
            tap,
            aggregate,
            uid,
        })
    }
}

impl Drop for SystemTap {
    fn drop(&mut self) {
        unsafe {
            AudioHardwareDestroyAggregateDevice(self.aggregate);
            AudioHardwareDestroyProcessTap(self.tap);
        }
    }
}

fn key(k: &CStr) -> CFRetained<CFString> {
    CFString::from_str(k.to_str().unwrap())
}

/// Private aggregate device holding only the tap, with drift compensation.
/// Same layout as cpal's `macos/loopback.rs` (cpal 0.18.2, Apache-2.0), which follows Apple's
/// "Capturing system audio with Core Audio taps" sample.
fn aggregate_description(tap_uid: &str, uid: &str) -> CFRetained<CFDictionary<CFString, CFType>> {
    let yes: &CFType = CFBoolean::new(true);
    let sub_tap = CFDictionary::<CFString, CFType>::from_slices(
        &[
            &key(kAudioSubTapUIDKey),
            &key(kAudioSubTapDriftCompensationKey),
        ],
        &[&CFString::from_str(tap_uid), yes],
    );
    CFDictionary::from_slices(
        &[
            &key(kAudioAggregateDeviceNameKey),
            &key(kAudioAggregateDeviceUIDKey),
            &key(kAudioAggregateDeviceTapListKey),
            &key(kAudioAggregateDeviceTapAutoStartKey),
            &key(kAudioAggregateDeviceIsPrivateKey),
        ],
        &[
            &CFString::from_str("회의 요약 시스템 오디오"),
            &CFString::from_str(uid),
            &CFArray::from_objects(&[&*sub_tap]),
            yes,
            yes,
        ],
    )
}
