// `async` keeps file and keychain I/O off the main (UI) thread.
use crate::QUIT_CONFIRMED;
use crate::audio::{self, AudioDevices, Recorder, RecordingState};
use crate::settings::{self, LoadedSettings, SecretKind, Settings, SettingsStore};
use crate::store::{Meta, Status, Store};
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, State};

#[tauri::command(async)]
pub fn get_settings(settings: State<'_, SettingsStore>) -> Result<LoadedSettings, String> {
    settings.load()
}

#[tauri::command(async)]
pub fn save_settings(settings: State<'_, SettingsStore>, value: Settings) -> Result<(), String> {
    settings.save(value)
}

/// Backs up an unreadable settings.json and starts from defaults.
#[tauri::command(async)]
pub fn reset_settings(settings: State<'_, SettingsStore>) -> Result<LoadedSettings, String> {
    settings.reset()
}

#[tauri::command(async)]
pub fn set_secret(kind: SecretKind, name: String, value: String) -> Result<(), String> {
    settings::set_secret(kind, &name, &value)
}

#[tauri::command(async)]
pub fn has_secret(kind: SecretKind, name: String) -> Result<bool, String> {
    settings::has_secret(kind, &name)
}

#[tauri::command(async)]
pub fn delete_secret(kind: SecretKind, name: String) -> Result<(), String> {
    settings::delete_secret(kind, &name)
}

/// A listed meeting. `interrupted`: left in `recording` by a crash, offered as
/// "처리 계속 / 삭제" (STO-03).
#[derive(Serialize)]
pub struct MeetingItem {
    #[serde(flatten)]
    meta: Meta,
    interrupted: bool,
}

#[tauri::command(async)]
pub fn list_meetings(store: State<'_, Store>) -> Result<Vec<MeetingItem>, String> {
    Ok(store
        .list()
        .into_iter()
        .map(|meta| MeetingItem {
            // A meeting deleted mid-recording may still be in the set.
            interrupted: meta.status == Status::Recording && store.is_interrupted(&meta.id),
            meta,
        })
        .collect())
}

#[derive(Serialize)]
pub struct MeetingDetail {
    meta: Meta,
    summary: String,
}

#[tauri::command(async)]
pub fn get_meeting(store: State<'_, Store>, id: String) -> Result<MeetingDetail, String> {
    Ok(MeetingDetail {
        meta: store.get(&id)?,
        summary: store.summary(&id)?,
    })
}

#[tauri::command(async)]
pub fn delete_meeting(
    store: State<'_, Store>,
    recorder: State<'_, Recorder>,
    id: String,
) -> Result<(), String> {
    // Delete step 2 (8.3): close the WAV files before the folder goes.
    recorder.stop(Some(&id));
    store.delete(&id)
}

/// "처리 계속" for an interrupted recording. `transcribe_failed` retries come with the queue (M3).
#[tauri::command(async)]
pub fn resume_meeting(store: State<'_, Store>, id: String) -> Result<Meta, String> {
    store.continue_interrupted(&id)
}

/// Pinned devices come from settings; unreadable settings must not block a recording.
#[tauri::command(async)]
pub fn start_recording(
    app: AppHandle,
    store: State<'_, Store>,
    settings: State<'_, SettingsStore>,
    recorder: State<'_, Recorder>,
) -> Result<RecordingState, String> {
    let settings = settings.load().map(|l| l.settings).unwrap_or_default();
    recorder.start(&app, &store, settings.input_device, settings.output_device)
}

/// For the settings' audio tab. Device calls can block inside the OS audio API, so this runs on
/// a blocking thread rather than an async worker.
#[tauri::command]
pub async fn list_audio_devices() -> Result<AudioDevices, String> {
    tauri::async_runtime::spawn_blocking(audio::devices)
        .await
        .map_err(|e| e.to_string())?
}

/// Waits until both WAV files are closed and the meeting is `recorded`.
#[tauri::command(async)]
pub fn stop_recording(recorder: State<'_, Recorder>) -> Option<String> {
    recorder.stop(None)
}

#[tauri::command(async)]
pub fn recording_state(recorder: State<'_, Recorder>) -> Option<RecordingState> {
    recorder.state()
}

/// The recorder confirmed quitting with work in progress (architecture 8.5).
/// Pausing is best effort: a meeting that cannot be written must not keep the app alive.
#[tauri::command(async)]
pub fn confirm_quit(
    app: AppHandle,
    store: State<'_, Store>,
    recorder: State<'_, Recorder>,
) -> Result<(), String> {
    QUIT_CONFIRMED.store(true, Ordering::SeqCst);
    // Closes the WAV files and writes `recorded` before anything else is paused.
    recorder.stop(None);
    store.pause_busy();
    app.exit(0);
    Ok(())
}

/// Windows Ctrl+Q (no menu bar there): same `ExitRequested` path as the tray "종료".
#[tauri::command]
pub fn request_quit(app: AppHandle) {
    app.exit(0);
}
