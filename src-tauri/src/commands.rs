// `async` keeps file and keychain I/O off the main (UI) thread.
use crate::QUIT_CONFIRMED;
use crate::audio::{self, AudioDevices, Recorder, RecordingState};
use crate::queue::{Progress, Queue};
use crate::settings::{self, LoadedSettings, SecretKind, Settings, SettingsStore};
use crate::slack::{self, Channel, Slack};
use crate::store::{Meta, Status, Store, one_line};
use crate::summarize::{self, SummaryRoute};
use crate::transcribe::engine::Segment;
use crate::transcribe::model::{self, Cancelled, ModelFile, ModelState, Models};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_dialog::DialogExt;

#[tauri::command(async)]
pub fn get_settings(settings: State<'_, SettingsStore>) -> Result<LoadedSettings, String> {
    settings.load()
}

/// A new model or backend may let a waiting meeting start, and a first LLM connection queues the
/// `transcribed` meetings for their summaries.
#[tauri::command(async)]
pub fn save_settings(
    settings: State<'_, SettingsStore>,
    queue: State<'_, Queue>,
    value: Settings,
) -> Result<Settings, String> {
    settings.save(local_default_kept(value, queue.builtin_ready()))?;
    queue.enqueue_pending();
    Ok(settings.load()?.settings)
}

/// SUM-08: new meetings start with "로컬로 요약" only while something local can summarize them:
/// the summary model, or a saved local connection (plan 5.4). Else it is stored off, so it never
/// stays on unseen (the settings screen hides its switch then).
fn local_default_kept(mut value: Settings, builtin_ready: bool) -> Settings {
    value.summarize_local_by_default &= summarize::local_available(&value, builtin_ready);
    value
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
    /// A send of it is out, maybe from a review screen since closed (7.1).
    sending: bool,
}

#[tauri::command(async)]
pub fn get_meeting(
    store: State<'_, Store>,
    slack: State<'_, Slack>,
    id: String,
) -> Result<MeetingDetail, String> {
    Ok(MeetingDetail {
        sending: slack.is_sending(&id),
        meta: store.get(&id)?,
        summary: store.summary(&id)?,
    })
}

/// `transcript.json`, for the processing and review screens.
#[tauri::command(async)]
pub fn get_transcript(store: State<'_, Store>, id: String) -> Result<Vec<Segment>, String> {
    store.transcript(&id)
}

#[tauri::command(async)]
pub fn delete_meeting(
    app: AppHandle,
    store: State<'_, Store>,
    recorder: State<'_, Recorder>,
    queue: State<'_, Queue>,
    id: String,
) -> Result<(), String> {
    // Delete step 2 (8.3): close the WAV files before the folder goes.
    recorder.stop(Some(&id));
    let deleted = store.delete_with(&id, || queue.cancel(&id));
    crate::refresh_tray(&app);
    deleted
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
/// Running transcriptions and local summaries stop inside `Recorder::start` (TRN-09, SUM-09).
#[tauri::command(async)]
pub fn start_recording(
    app: AppHandle,
    store: State<'_, Store>,
    settings: State<'_, SettingsStore>,
    recorder: State<'_, Recorder>,
) -> Result<RecordingState, String> {
    let settings = settings.load().map(|l| l.settings).unwrap_or_default();
    let started = recorder.start(
        &app,
        &store,
        settings.input_device,
        settings.output_device,
        settings.summarize_local_by_default,
    );
    crate::refresh_tray(&app);
    started
}

/// REC-12: the recording file at `path` (the file picker's) as a new `recorded` meeting, queued
/// like one that just finished recording. "로컬로 요약" starts as for a recording (SUM-08). On a
/// blocking thread: an hour of audio takes seconds to decode.
#[tauri::command]
pub async fn import_recording(app: AppHandle, path: String) -> Result<Meta, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = app
            .state::<SettingsStore>()
            .load()
            .map(|l| l.settings)
            .unwrap_or_default();
        let meta = audio::import::import(
            &app.state::<Store>(),
            Path::new(&path),
            settings.summarize_local_by_default,
        )?;
        app.state::<Queue>().enqueue(&meta.id, meta.status);
        Ok(meta)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// For the settings' audio tab. Device calls can block inside the OS audio API, so this runs on
/// a blocking thread rather than an async worker.
#[tauri::command]
pub async fn list_audio_devices() -> Result<AudioDevices, String> {
    tauri::async_runtime::spawn_blocking(audio::devices)
        .await
        .map_err(|e| e.to_string())?
}

/// The audio tab's checks open devices of their own, so never beside a recording's.
fn recording(app: &AppHandle) -> bool {
    app.state::<Recorder>().state().is_some()
}

/// ONB-03: plays a 1 kHz tone and listens to the system audio meanwhile (architecture 4.4); true
/// when it was heard. `device`: the output device the tab shows, saved or not (Windows only). On
/// a blocking thread: a permission prompt holds the opening.
#[tauri::command]
pub async fn test_system_audio(app: AppHandle, device: Option<String>) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        audio::check::system_audio(device, || recording(&app))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// ONB-03: listens to the microphone the tab shows (`device`, saved or not) for 5 s, sending
/// `audio-test://level` (0..100). True when sound came in.
#[tauri::command]
pub async fn test_microphone(app: AppHandle, device: Option<String>) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        audio::check::microphone(
            device,
            || recording(&app),
            |level| {
                let _ = app.emit("audio-test://level", level);
            },
        )
    })
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

/// Settings 일반 "로그 폴더 열기": the folder of `app.log` in the file manager. The path is
/// resolved here, so the webview names no path and gets no opener scope. Created first: it is
/// missing when app.log could not be opened at startup.
#[tauri::command(async)]
pub fn open_log_dir(app: AppHandle) -> Result<(), String> {
    let opened = app
        .path()
        .app_log_dir()
        .map_err(|e| e.to_string())
        .and_then(|dir| {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            tauri_plugin_opener::open_path(&dir, None::<&str>).map_err(|e| e.to_string())
        });
    opened.map_err(|e| format!("로그 폴더를 열지 못했습니다: {e}"))
}

/// Queued and running meetings, for the first render before `meeting://progress` arrives.
#[tauri::command]
pub fn queue_state(queue: State<'_, Queue>) -> Vec<Progress> {
    queue.snapshot()
}

/// DX12: what the four model commands act on; `None` is `All`, as for the callers before scopes.
#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The chosen Whisper model and the VAD model, then the summary model while the built-in LLM
    /// is the default (E3). A cloud or other local default leaves it out (plan 5.2).
    All,
    /// The Whisper and VAD models only, for the 전사 tab.
    Transcription,
    Summary,
}

fn scoped(scope: Option<Scope>, settings: &Settings) -> Result<Vec<&'static ModelFile>, String> {
    if scope == Some(Scope::Summary) {
        return Ok(vec![&model::SUMMARY]);
    }
    let mut files = model::needed(&settings.model)?.to_vec();
    if scope != Some(Scope::Transcription) && settings.default_connection().is_none() {
        files.push(&model::SUMMARY);
    }
    Ok(files)
}

/// ENG18: while llama-server runs it holds the summary model open, and Windows can neither replace
/// nor delete an open file. `running`: `Queue::engine_running`.
/// ponytail: checked once before the download or import, so one racing a summary's engine start
/// can still meet an open file; that fails the download or import with an OS error, nothing worse.
/// Check again before the rename if that shows up.
pub(crate) fn not_in_use(files: &[&ModelFile], running: bool) -> Result<(), String> {
    if running && files.iter().any(|f| f.file == model::SUMMARY.file) {
        return Err("요약 중에는 요약 모델을 바꿀 수 없습니다".into());
    }
    Ok(())
}

/// E3: a file in place lets waiting meetings start without the rest of the chain; the summary
/// model also queues the `transcribed` ones (`enqueue_pending` pumps too).
// ponytail: after a download this lists the meetings on an async worker; move it to
// `spawn_blocking` if the list ever gets slow.
pub(crate) fn arrived<R: Runtime>(queue: &Queue<R>, f: &ModelFile) {
    if f.file == model::SUMMARY.file {
        queue.enqueue_pending();
    } else {
        queue.pump();
    }
}

/// The scope's files: installed, partly downloaded, downloading.
#[tauri::command(async)]
pub fn model_status(
    settings: State<'_, SettingsStore>,
    models: State<'_, Models>,
    scope: Option<Scope>,
) -> Result<ModelState, String> {
    let settings = settings.load().map(|l| l.settings).unwrap_or_default();
    Ok(models.state(&scoped(scope, &settings)?))
}

/// ONB-01, E3: downloads the scope's missing files one by one from `model_base_url` (a mirror if
/// set), sending `model://download`. Resumes a `.part` left by an earlier try.
#[tauri::command]
pub async fn download_models(
    app: AppHandle,
    settings: State<'_, SettingsStore>,
    models: State<'_, Models>,
    queue: State<'_, Queue>,
    scope: Option<Scope>,
) -> Result<(), String> {
    let settings = settings.load().map(|l| l.settings).unwrap_or_default();
    let files = scoped(scope, &settings)?;
    not_in_use(&files, queue.engine_running())?;
    let installed = app.clone();
    let result = models
        .download(
            &settings.model_base_url,
            &files,
            move |progress| {
                let _ = app.emit("model://download", progress);
            },
            move |f| arrived(&installed.state::<Queue>(), f),
        )
        .await;
    // Also for a file installed as a cancel landed, and for files all there already.
    if files.iter().any(|f| f.file == model::SUMMARY.file) && models.installed(&model::SUMMARY) {
        queue.enqueue_pending();
    } else {
        queue.pump();
    }
    result
}

/// Stops the download and the rest of its chain; what arrived stays for resuming. A narrower
/// scope cancels only its files: one in flight stops the download all the same, ones still
/// ahead just leave the chain. `null` when there was nothing of the scope to cancel.
#[tauri::command(async)]
pub fn cancel_model_download(
    settings: State<'_, SettingsStore>,
    models: State<'_, Models>,
    scope: Option<Scope>,
) -> Result<Option<Cancelled>, String> {
    let settings = settings.load().map(|l| l.settings).unwrap_or_default();
    let only = match scope {
        None | Some(Scope::All) => None,
        _ => Some(scoped(scope, &settings)?),
    };
    Ok(models.cancel(only.as_deref()))
}

/// ONB-02: installs a model file of the scope the recorder picked, if its size and sha256 match.
#[tauri::command(async)]
pub fn import_model(
    settings: State<'_, SettingsStore>,
    models: State<'_, Models>,
    queue: State<'_, Queue>,
    path: String,
    scope: Option<Scope>,
) -> Result<ModelState, String> {
    let settings = settings.load().map(|l| l.settings).unwrap_or_default();
    let files = scoped(scope, &settings)?;
    not_in_use(&files, queue.engine_running())?;
    arrived(&queue, models.import(Path::new(&path), &files)?);
    Ok(models.state(&files))
}

/// Where this meeting's summary goes (SUM-07) and whether "로컬로 요약" can still change (SUM-08).
#[tauri::command(async)]
pub fn summary_route(
    store: State<'_, Store>,
    settings: State<'_, SettingsStore>,
    models: State<'_, Models>,
    id: String,
) -> Result<SummaryRoute, String> {
    let settings = settings.load()?.settings;
    let ready = models.installed(&model::SUMMARY);
    Ok(SummaryRoute::of(&settings, &store.get(&id)?, ready))
}

/// SUM-08 "로컬로 요약", until summarizing starts.
#[tauri::command(async)]
pub fn set_summarize_local(
    store: State<'_, Store>,
    settings: State<'_, SettingsStore>,
    queue: State<'_, Queue>,
    id: String,
    local: bool,
) -> Result<SummaryRoute, String> {
    summarize_locally(&store, &settings.load()?.settings, &queue, &id, local)
}

/// `set_summarize_local`'s work: the switch, and a meeting waiting for its route queued again, as
/// it may have one now (plan 5.4).
pub(crate) fn summarize_locally<R: Runtime>(
    store: &Store,
    settings: &Settings,
    queue: &Queue<R>,
    id: &str,
    local: bool,
) -> Result<SummaryRoute, String> {
    let ready = queue.builtin_ready();
    if local && !summarize::local_available(settings, ready) {
        return Err("로컬 LLM 연결이 없습니다".into());
    }
    let meta = store.set_summarize_local(id, local)?;
    if meta.wait.is_some() {
        queue.enqueue_pending();
    }
    Ok(SummaryRoute::of(settings, &meta, ready))
}

/// One entry of `list_connections` (plan 5.4): a connection a screen may offer, the built-in route
/// as "이 PC". No secret.
#[derive(Serialize, Debug, PartialEq)]
pub struct ConnectionItem {
    name: String,
    label: String,
    /// Where it sends a transcript, as SUM-07 names it. Empty for the built-in route.
    host: String,
    is_local: bool,
    builtin: bool,
}

fn connection_items(settings: &Settings) -> Vec<ConnectionItem> {
    let host = |url: &str| {
        let url = url.trim();
        reqwest::Url::parse(url)
            .ok()
            .and_then(|u| {
                let host = u.host_str()?;
                Some(
                    u.port()
                        .map_or(host.to_string(), |port| format!("{host}:{port}")),
                )
            })
            .unwrap_or_else(|| url.to_string())
    };
    settings
        .connections()
        .into_iter()
        .map(|c| {
            let builtin = c.name == settings::BUILTIN;
            ConnectionItem {
                label: if builtin {
                    settings::BUILTIN_LABEL.into()
                } else {
                    c.name.clone()
                },
                host: host(&c.base_url),
                is_local: c.is_local,
                builtin,
                name: c.name,
            }
        })
        .collect()
}

/// The saved connections and the built-in route (plan 5.4). `get_settings` never carries the
/// built-in route; screens that pick a connection read this.
#[tauri::command(async)]
pub fn list_connections(settings: State<'_, SettingsStore>) -> Result<Vec<ConnectionItem>, String> {
    Ok(connection_items(&settings.load()?.settings))
}

/// REV-01: the edited title and summary body, in `review` only.
#[tauri::command(async)]
pub fn save_summary(
    store: State<'_, Store>,
    id: String,
    title: String,
    body: String,
) -> Result<Meta, String> {
    store.save_summary(&id, &title, &body)
}

/// REV-03 "전송 안 함": `review` → `archived`.
#[tauri::command(async)]
pub fn archive_meeting(
    app: AppHandle,
    store: State<'_, Store>,
    id: String,
) -> Result<Meta, String> {
    let archived = store.set_status(&id, Status::Archived);
    crate::refresh_tray(&app);
    archived
}

/// REV-02: the characters of the Slack message block for this title and body, header included,
/// footer not. `offset_minutes`: the local UTC offset at the meeting's start (`slack::markdown`).
#[tauri::command(async)]
pub fn slack_length(
    store: State<'_, Store>,
    settings: State<'_, SettingsStore>,
    id: String,
    title: String,
    body: String,
    offset_minutes: i32,
) -> Result<usize, String> {
    let recorder = settings.load()?.settings.recorder_name;
    let meta = store.get(&id)?;
    let text = slack::markdown(&one_line(&title), &meta, offset_minutes, &recorder, &body);
    Ok(text.chars().count())
}

/// SND-01..06: the reviewed summary to `channel`, as the signed-in recorder. Only the send button
/// calls this (SND-05). `slack://finished` (the id) tells the list and a review screen opened
/// meanwhile that it ended, sent or not.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn send_to_slack(
    app: AppHandle,
    store: State<'_, Store>,
    settings: State<'_, SettingsStore>,
    slack: State<'_, Slack>,
    id: String,
    channel: Channel,
    title: String,
    body: String,
    offset_minutes: i32,
) -> Result<Meta, String> {
    let sent = slack
        .send(
            &store,
            &settings,
            &id,
            &channel,
            &title,
            &body,
            offset_minutes,
        )
        .await;
    crate::refresh_tray(&app);
    if let Err(e) = app.emit("slack://finished", &id) {
        log::warn!("전송 종료 이벤트 전송 실패: {e}");
    }
    sent
}

/// ONB-04: Slack sign-in in the browser. The workspace's name, or `None` when cancelled.
#[tauri::command]
pub async fn slack_sign_in(
    settings: State<'_, SettingsStore>,
    slack: State<'_, Slack>,
) -> Result<Option<String>, String> {
    slack
        .sign_in(&settings, |url| {
            tauri_plugin_opener::open_url(url, None::<&str>)
                .map_err(|e| format!("브라우저를 열지 못했습니다: {e}"))
        })
        .await
}

#[tauri::command]
pub fn slack_cancel_sign_in(slack: State<'_, Slack>) {
    slack.cancel_sign_in();
}

#[tauri::command]
pub async fn slack_sign_out(
    settings: State<'_, SettingsStore>,
    slack: State<'_, Slack>,
) -> Result<(), String> {
    slack.sign_out(&settings).await
}

/// SND-04: the channels the signed-in recorder is in, for the send bar.
#[tauri::command]
pub async fn slack_channels(
    settings: State<'_, SettingsStore>,
    slack: State<'_, Slack>,
) -> Result<Vec<Channel>, String> {
    slack.channels(&settings).await
}

/// SUM-06 "요약 다시 실행" after `summarize_failed`, and ENG15's change of connection for a meeting
/// waiting with its transcript, with the connection the recorder picked (`list_connections`'s
/// name, the built-in route's too).
#[tauri::command(async)]
pub fn retry_summary(
    store: State<'_, Store>,
    settings: State<'_, SettingsStore>,
    queue: State<'_, Queue>,
    id: String,
    connection: String,
) -> Result<Meta, String> {
    let settings = settings.load()?.settings;
    let chosen = retry_connection(&settings, &connection, queue.builtin_ready())?;
    let meta = store.retry_summary(&id, &chosen)?;
    queue.enqueue(&id, meta.status);
    Ok(meta)
}

/// What `retry_summary` pins: a connection the meeting's route then resolves to (plan 5.4), so a
/// retry never ends as a wait, nor in `summarize_failed` waiting for nothing.
fn retry_connection(
    settings: &Settings,
    name: &str,
    builtin_ready: bool,
) -> Result<settings::LlmConnection, String> {
    let chosen = settings.connections().into_iter().find(|c| c.name == name);
    let chosen = chosen.ok_or_else(|| {
        format!("LLM 연결 '{name}'을(를) 찾을 수 없습니다. 다른 연결을 골라 주세요")
    })?;
    if chosen.name == settings::BUILTIN && !builtin_ready {
        return Err(
            "이 PC 요약 모델이 아직 없습니다. 설정 → 요약(LLM)에서 요약 모델을 먼저 받아 주세요"
                .into(),
        );
    }
    Ok(chosen)
}

/// REV-05: saves the summary and the transcript as one `.md` file. The save dialog opens here, so
/// the webview never names a path to write. `None` when the recorder cancels.
#[tauri::command]
pub async fn export_markdown(
    app: AppHandle,
    store: State<'_, Store>,
    id: String,
) -> Result<Option<String>, String> {
    let meta = store.get(&id)?;
    let title = if meta.title.trim().is_empty() {
        id.clone()
    } else {
        meta.title.trim().to_string()
    };
    let text = summarize::markdown(&title, &store.summary(&id)?, &store.transcript(&id)?);
    let name = file_name(&title);
    // Blocks until the recorder answers, so not on an async worker.
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(format!("{name}.md"))
            .add_filter("Markdown", &["md"])
            .blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("파일을 저장하지 못했습니다: {e}"))?;
    Ok(Some(path.display().to_string()))
}

/// The save dialog's suggested name: path separators, characters Windows refuses in a file name,
/// and control characters become `_`.
fn file_name(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_control() || r#"\/:*?"<>|"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DX12, E3: transcription first, the summary model only for the built-in default.
    fn cloud(name: &str, base_url: &str) -> settings::LlmConnection {
        settings::LlmConnection {
            name: name.into(),
            base_url: base_url.into(),
            model: "m".into(),
            context_tokens: 128_000,
            is_local: false,
            timeout_secs: 300,
        }
    }

    #[test]
    fn scopes_name_their_files_in_order() {
        let files = |scope, default_llm: Option<&str>| {
            let settings = Settings {
                llm_connections: vec![cloud("클라우드", "https://api.example.com/v1")],
                default_llm: default_llm.map(String::from),
                ..Settings::default()
            };
            scoped(scope, &settings)
                .unwrap()
                .iter()
                .map(|f| f.file)
                .collect::<Vec<_>>()
        };
        let transcription = model::needed(&Settings::default().model)
            .unwrap()
            .map(|f| f.file);
        let summary = model::SUMMARY.file;
        assert_eq!(files(None, None), [&transcription[..], &[summary]].concat());
        assert_eq!(files(Some(Scope::All), Some("클라우드")), transcription);
        assert_eq!(files(Some(Scope::Transcription), None), transcription);
        assert_eq!(files(Some(Scope::Summary), Some("클라우드")), [summary]);
        // The built-in default however it is spelt (`Settings::default_connection`).
        for builtin in [Some("builtin"), Some("지운 연결")] {
            assert_eq!(
                files(None, builtin),
                [&transcription[..], &[summary]].concat()
            );
        }
        assert_eq!(
            serde_json::from_str::<[Scope; 3]>(r#"["all", "transcription", "summary"]"#).unwrap(),
            [Scope::All, Scope::Transcription, Scope::Summary]
        );
    }

    #[test]
    fn the_summary_model_cannot_change_while_the_engine_runs() {
        let transcription = model::needed(&Settings::default().model).unwrap();
        let all = [&transcription[..], &[&model::SUMMARY]].concat();
        assert_eq!(
            not_in_use(&all, true).unwrap_err(),
            "요약 중에는 요약 모델을 바꿀 수 없습니다"
        );
        assert!(not_in_use(&[&model::SUMMARY], true).is_err());
        assert!(not_in_use(&all, false).is_ok());
        // The transcription files are not llama-server's.
        assert!(not_in_use(&transcription, true).is_ok());
    }

    /// SUM-08: the local default is stored only while something local can summarize; a
    /// connection under a reserved name does not count (d6).
    #[test]
    fn the_local_default_is_stored_only_while_something_local_can_summarize() {
        let kept = |connections: Vec<settings::LlmConnection>, ready: bool| {
            let value = Settings {
                llm_connections: connections,
                summarize_local_by_default: true,
                ..Settings::default()
            };
            local_default_kept(value, ready).summarize_local_by_default
        };
        let local = |name: &str| settings::LlmConnection {
            is_local: true,
            ..cloud(name, "http://localhost:11434/v1")
        };
        assert!(kept(vec![], true));
        assert!(kept(vec![local("Ollama")], false));
        assert!(!kept(
            vec![cloud("클라우드", "https://api.example.com/v1")],
            false
        ));
        assert!(!kept(vec![local("이 PC")], false));
        // Off stays off.
        let off = Settings {
            summarize_local_by_default: false,
            ..Settings::default()
        };
        assert!(!local_default_kept(off, true).summarize_local_by_default);
    }

    /// Plan 5.4: the screens' list ends with the built-in route as "이 PC"; `get_settings` and the
    /// file `save_settings` writes never hold it.
    #[test]
    fn list_connections_offers_this_pc_which_settings_never_hold() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(tmp.path().join("settings.json"));
        store
            .save(Settings {
                llm_connections: vec![cloud("회사 클라우드", " https://api.example.com:8443/v1 ")],
                ..Settings::default()
            })
            .unwrap();
        let loaded = store.load().unwrap();
        let item = |name: &str, label: &str, host: &str, local| ConnectionItem {
            name: name.into(),
            label: label.into(),
            host: host.into(),
            is_local: local,
            builtin: local,
        };
        assert_eq!(
            connection_items(&loaded.settings),
            [
                item(
                    "회사 클라우드",
                    "회사 클라우드",
                    "api.example.com:8443",
                    false
                ),
                item("builtin", "이 PC", "", true),
            ]
        );
        let sent = serde_json::to_string(&loaded).unwrap();
        let file = std::fs::read_to_string(tmp.path().join("settings.json")).unwrap();
        for text in [sent, file] {
            assert!(
                !text.contains(settings::BUILTIN) && !text.contains("이 PC"),
                "{text}"
            );
        }
        assert_eq!(loaded.settings.default_llm, None);
    }

    /// Plan 5.4: a retry pins only a connection the meeting can be summarized with now, so it
    /// never ends waiting (in `summarize_failed`, for a model nothing will queue it for).
    #[test]
    fn a_retry_takes_only_a_connection_that_can_summarize_now() {
        let settings = Settings {
            llm_connections: vec![cloud("회사 클라우드", "https://api.example.com/v1")],
            ..Settings::default()
        };
        let picked = |name, ready| retry_connection(&settings, name, ready);
        assert_eq!(
            picked("회사 클라우드", false).unwrap().name,
            "회사 클라우드"
        );
        assert_eq!(picked(settings::BUILTIN, true), Ok(settings::builtin()));
        let modelless = picked(settings::BUILTIN, false).unwrap_err();
        assert!(
            modelless.contains("요약 모델을 먼저 받아 주세요"),
            "{modelless}"
        );
        let gone = picked("지운 연결", true).unwrap_err();
        assert!(
            gone.contains("찾을 수 없습니다") && gone.contains("다른 연결"),
            "{gone}"
        );
    }

    #[test]
    fn export_file_name_cannot_leave_the_folder_or_upset_windows() {
        assert_eq!(file_name("주간 회의"), "주간 회의");
        assert_eq!(
            file_name("../a\\b:c*d?\"e<f>g|h\u{7}"),
            ".._a_b_c_d__e_f_g_h_"
        );
    }
}
