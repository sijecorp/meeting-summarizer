// `async` keeps file and keychain I/O off the main (UI) thread.
use crate::QUIT_CONFIRMED;
use crate::audio::{self, AudioDevices, Recorder, RecordingState};
use crate::queue::{Progress, Queue};
use crate::settings::{self, LoadedSettings, SecretKind, Settings, SettingsStore};
use crate::store::{Meta, Status, Store};
use crate::transcribe::model::{self, ModelState, Models};
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};

#[tauri::command(async)]
pub fn get_settings(settings: State<'_, SettingsStore>) -> Result<LoadedSettings, String> {
    settings.load()
}

/// A new model or backend may let a waiting meeting start.
#[tauri::command(async)]
pub fn save_settings(
    settings: State<'_, SettingsStore>,
    queue: State<'_, Queue>,
    value: Settings,
) -> Result<(), String> {
    settings.save(value)?;
    queue.pump();
    Ok(())
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
    queue: State<'_, Queue>,
    id: String,
) -> Result<(), String> {
    // Delete step 2 (8.3): close the WAV files before the folder goes.
    recorder.stop(Some(&id));
    store.delete_with(&id, || queue.cancel(&id))
}

/// "처리 계속" for an interrupted recording, or another try after `transcribe_failed`. Either
/// way the meeting joins the processing queue (TRN-01).
#[tauri::command(async)]
pub fn resume_meeting(
    store: State<'_, Store>,
    queue: State<'_, Queue>,
    id: String,
) -> Result<Meta, String> {
    let meta = if store.is_interrupted(&id) {
        store.continue_interrupted(&id)?
    } else {
        store.get(&id)?
    };
    if !matches!(
        meta.status,
        Status::Recorded | Status::Paused | Status::TranscribeFailed
    ) {
        return Err("다시 처리할 수 있는 회의가 아닙니다".into());
    }
    queue.enqueue(&id, meta.status);
    Ok(meta)
}

/// Pinned devices come from settings; unreadable settings must not block a recording.
/// Running transcriptions stop inside `Recorder::start` (TRN-09).
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
    queue: State<'_, Queue>,
) -> Result<(), String> {
    QUIT_CONFIRMED.store(true, Ordering::SeqCst);
    // Workers first: child processes would outlive the app.
    queue.shutdown();
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

/// Queued and running meetings, for the first render before `meeting://progress` arrives.
#[tauri::command]
pub fn queue_state(queue: State<'_, Queue>) -> Vec<Progress> {
    queue.snapshot()
}

/// The chosen Whisper model and the VAD model: installed, partly downloaded, downloading.
#[tauri::command(async)]
pub fn model_status(
    settings: State<'_, SettingsStore>,
    models: State<'_, Models>,
) -> Result<ModelState, String> {
    let settings = settings.load().map(|l| l.settings).unwrap_or_default();
    models.state(&settings.model)
}

/// ONB-01: downloads the missing model files from `model_base_url` (a mirror if set), sending
/// `model://download`. Resumes a `.part` left by an earlier try. Queued meetings start after.
#[tauri::command]
pub async fn download_models(
    app: AppHandle,
    settings: State<'_, SettingsStore>,
    models: State<'_, Models>,
    queue: State<'_, Queue>,
) -> Result<(), String> {
    let settings = settings.load().map(|l| l.settings).unwrap_or_default();
    let files = model::needed(&settings.model)?;
    models
        .download(&settings.model_base_url, files, move |progress| {
            let _ = app.emit("model://download", progress);
        })
        .await?;
    queue.pump();
    Ok(())
}

/// Stops the download; what arrived stays for resuming.
#[tauri::command]
pub fn cancel_model_download(models: State<'_, Models>) {
    models.cancel();
}

/// ONB-02: installs a model file the recorder picked, if its size and sha256 match.
#[tauri::command(async)]
pub fn import_model(
    settings: State<'_, SettingsStore>,
    models: State<'_, Models>,
    queue: State<'_, Queue>,
    path: String,
) -> Result<ModelState, String> {
    let settings = settings.load().map(|l| l.settings).unwrap_or_default();
    models.import(Path::new(&path), &model::needed(&settings.model)?)?;
    queue.pump();
    models.state(&settings.model)
}
