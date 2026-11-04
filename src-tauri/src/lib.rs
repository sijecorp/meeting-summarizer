mod audio;
mod commands;
mod grounding;
mod queue;
mod settings;
mod shape;
mod sidecar;
mod slack;
mod store;
mod summarize;
mod transcribe;

/// This executable as a child process, before the app builds: a transcription worker
/// (architecture 5.3) or the summary engine's supervisor (6.6) runs and exits inside. Returns for
/// the app.
pub fn worker_main() {
    sidecar::supervisor_main();
    transcribe::engine::worker_main();
}

/// Plan 5.1-7: opts this process (`None`) or a child out of EcoQoS, so Windows does not move its
/// work to efficiency cores at low clocks to save power. The priority stays. `Err`: the error code.
#[cfg(windows)]
pub(crate) fn no_ecoqos(child: Option<&std::process::Child>) -> Result<(), i32> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
        ProcessPowerThrottling, SetProcessInformation,
    };
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    // SAFETY: our own pseudo handle, or the child's while `child` is borrowed; `state` outlives
    // the call that reads it.
    let set = unsafe {
        let process = match child {
            Some(child) => child.as_raw_handle(),
            None => GetCurrentProcess(),
        };
        SetProcessInformation(
            process,
            ProcessPowerThrottling,
            (&raw const state).cast(),
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    };
    match set {
        0 => Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)),
        _ => Ok(()),
    }
}

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
#[cfg(target_os = "macos")]
use tauri::menu::{PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, RunEvent, Runtime, WindowEvent};
use tauri_plugin_notification::{Notification, NotificationExt};

/// Set by `confirm_quit`. After that, `ExitRequested` must let the exit through even if a
/// meeting still looks busy (its pause failed), or quitting would loop forever (8.5).
pub(crate) static QUIT_CONFIRMED: AtomicBool = AtomicBool::new(false);

/// `ExitRequested`: hold the exit for a confirmation only while work runs and the recorder
/// has not confirmed yet.
fn must_confirm_exit(quit_confirmed: bool, store: Option<&store::Store>) -> bool {
    !quit_confirmed && store.is_some_and(|s| !s.busy_ids().is_empty())
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// APP-03: an OS notification, sent only while the main window is hidden, minimized or not
/// focused; otherwise the screen itself tells the recorder. A click opens `meeting`. The body is
/// an event and at most a meeting title, never transcript or summary text.
pub(crate) fn notify<R: Runtime>(app: &AppHandle<R>, body: String, meeting: &str) {
    let (handle, meeting) = (app.clone(), meeting.to_string());
    // The window is asked on the main thread, so no caller ever waits on it.
    let _ = app.run_on_main_thread(move || {
        let Some(notification) = handle.try_state::<Notification<R>>() else {
            return;
        };
        let seen = handle.get_webview_window("main").is_some_and(|w| {
            w.is_visible().unwrap_or(false)
                && w.is_focused().unwrap_or(false)
                && !w.is_minimized().unwrap_or(false)
        });
        if seen {
            return;
        }
        // productName is the bundle's ASCII name; the title is the app's Korean one.
        let shown = notification
            .builder()
            .title("회의 요약")
            .body(body)
            .extra("meeting", meeting)
            .show();
        if let Err(e) = shown {
            log::warn!("알림을 보내지 못했습니다: {e}");
        }
    });
}

/// macOS (architecture 11.1): the main window gained focus, so the recorder is looking at the app
/// and its notifications are stale. Clearing them also ends the threads that wait on them.
#[cfg(target_os = "macos")]
fn clears_notifications(window: &str, event: &WindowEvent) -> bool {
    window == "main" && matches!(event, WindowEvent::Focused(true))
}

/// This app's notifications out of Notification Center. Main thread only. Without a bundle
/// identifier (a bare binary, the tests) there is no notification center, and asking for one
/// would panic, which aborts a release build.
#[cfg(target_os = "macos")]
fn clear_notifications() {
    if objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .is_some()
    {
        #[allow(deprecated)]
        objc2_foundation::NSUserNotificationCenter::defaultUserNotificationCenter()
            .removeAllDeliveredNotifications();
    }
}

const TRAY: &str = "main";
/// Architecture 10.3 colors: `danger`, `accent` (a muted `mainBlue`), `warning`.
const RECORDING_DOT: [u8; 3] = [0xf6, 0x59, 0x59];
const PROCESSING_DOT: [u8; 3] = [0x49, 0x50, 0xb0];
const UNSENT_DOT: [u8; 3] = [0xff, 0x9b, 0x3f];

/// APP-04: the tray's dot and tooltip. Recording beats processing beats unsent. While recording,
/// processing is not counted: a recording holds it (TRN-09), like the header chip.
fn tray_state(recording: bool, processing: usize, unsent: usize) -> (Option<[u8; 3]>, String) {
    let dot = if recording {
        Some(RECORDING_DOT)
    } else if processing > 0 {
        Some(PROCESSING_DOT)
    } else if unsent > 0 {
        Some(UNSENT_DOT)
    } else {
        None
    };
    let mut tooltip = String::from("회의 요약");
    if recording {
        tooltip += " · 녹음 중";
    } else if processing > 0 {
        tooltip += &format!(" · 처리 중 {processing}");
    }
    if unsent > 0 {
        tooltip += &format!(" · 미전송 {unsent}");
    }
    (dot, tooltip)
}

/// The icon with a `color` dot in its bottom-right corner, 40% of its width across: the macOS menu
/// bar draws the icon at about 22 pt, so a smaller dot is hard to see. A white rim keeps it apart
/// from the icon on light and dark bars alike.
fn with_dot(rgba: &[u8], width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let size = width * 2 / 5;
    let r = size as f32 / 2.0;
    let (cx, cy) = (width as f32 - r, height as f32 - r);
    for y in height.saturating_sub(size)..height {
        for x in width.saturating_sub(size)..width {
            let d = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy);
            if d <= r {
                let [red, green, blue] = if d <= r * 0.75 { color } else { [255; 3] };
                let at = ((y * width + x) * 4) as usize;
                out[at..at + 4].copy_from_slice(&[red, green, blue, 255]);
            }
        }
    }
    out
}

/// APP-04: redraws the tray from the meetings (one `list`), the recorder and the queue. Called
/// where those change; nothing polls. The counts are read on a blocking-pool thread, neither the
/// main thread nor the caller's: a caller may be waited on by a lock holder (`Recorder::start`
/// joins an ended writer under its lock, and the writer's last step lands here). Only the icon is
/// set on the main thread.
pub(crate) fn refresh_tray<R: Runtime>(app: &AppHandle<R>) {
    // No tray in the queue's tests on the mock runtime.
    if app.tray_by_id(TRAY).is_none() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // One at a time, each reading the state as it is then: the last to run draws the newest.
        static ORDER: Mutex<()> = Mutex::new(());
        let _order = ORDER.lock().unwrap_or_else(|e| e.into_inner());
        let recording = app
            .try_state::<audio::Recorder>()
            .is_some_and(|r| r.state().is_some());
        let processing = app
            .try_state::<queue::Queue<R>>()
            .map_or(0, |q| q.snapshot().len());
        // Unsent: waiting in review. `sent` and `archived` are not (design doc, APP-04).
        let unsent = app.try_state::<store::Store>().map_or(0, |s| {
            s.list()
                .iter()
                .filter(|m| m.status == store::Status::Review)
                .filter(|m| m.schema_version <= store::SCHEMA_VERSION)
                .count()
        });
        let (dot, tooltip) = tray_state(recording, processing, unsent);
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(tray) = handle.tray_by_id(TRAY) else {
                return;
            };
            let icon = handle.default_window_icon().map(|base| match dot {
                Some(color) => Image::new_owned(
                    with_dot(base.rgba(), base.width(), base.height(), color),
                    base.width(),
                    base.height(),
                ),
                None => base.clone(),
            });
            let _ = tray.set_icon(icon);
            let _ = tray.set_tooltip(Some(tooltip));
        });
    });
}

/// macOS only. Our own menu instead of the default one: the default "Quit" skips
/// `ExitRequested` (architecture 8.5). The Edit menu keeps copy/paste shortcuts working in text
/// fields. Windows gets no menu bar; the frontend sends Ctrl+Q to `request_quit` instead.
#[cfg(target_os = "macos")]
fn app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let quit = MenuItem::with_id(app, "quit", "종료", true, Some("CmdOrCtrl+Q"))?;
    let edit = Submenu::with_items(
        app,
        "편집",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    Menu::with_items(
        app,
        &[
            &Submenu::with_items(app, "회의 요약", true, &[&quit])?,
            &edit,
        ],
    )
}

/// The app log (architecture 8.1): stdout, so `tauri dev` shows it, and with `file` also
/// `app.log` in `app_log_dir()`. 5 MB keeps months of these few lines per meeting and still
/// attaches to a Slack message; two rotated files cap the folder near 15 MB. Info in dev too: the
/// app has no `debug!` lines, so Debug would only add tao, wry and reqwest noise. The plugin's
/// JS `log` command is not registered: the webview does not log.
fn attach_log<R: Runtime>(app: &AppHandle<R>, file: bool) -> Result<(), String> {
    use tauri_plugin_log::{Builder, RotationStrategy, Target, TargetKind};
    let mut targets = vec![Target::new(TargetKind::Stdout)];
    if file {
        targets.push(Target::new(TargetKind::LogDir {
            file_name: Some("app".into()),
        }));
    }
    let (_, level, logger) = Builder::new()
        .targets(targets)
        .max_file_size(5_000_000)
        .rotation_strategy(RotationStrategy::KeepSome(2))
        .level(log::LevelFilter::Info)
        .split(app)
        .map_err(|e| e.to_string())?;
    tauri_plugin_log::attach_logger(level, logger).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Release builds abort on panic and Windows release has no stderr, so a crash would leave no
    // trace. Workers never get here (`worker_main` exits first): their panics stay on stderr,
    // which is their `worker.log` (5.3).
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("{info}");
        default_hook(info);
    }));
    let builder = tauri::Builder::default()
        // Must be first: a second launch exits here and focuses the running instance (APP-06).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app)
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init());
    #[cfg(target_os = "macos")]
    let builder = builder.menu(app_menu);
    builder
        .setup(|app| {
            // First, so the rest is logged. Here, not as a plugin: a plugin's setup error stops the
            // app, and an app.log that cannot be opened or rotated (another process holds it, a
            // read-only profile) must not. Plugins set up before this, so a second launch has
            // exited without touching the running app's log.
            if let Err(e) = attach_log(app.handle(), true)
                && attach_log(app.handle(), false).is_ok()
            {
                log::error!("app.log를 열 수 없어 stdout에만 씁니다: {e}");
            }
            #[cfg(windows)]
            if let Err(code) = no_ecoqos(None) {
                log::warn!("EcoQoS를 끄지 못했습니다: {code}");
            }
            // Recordings and models stay out of the Windows roaming profile (8.1).
            let local = app.path().app_local_data_dir()?;
            let store = store::Store::new(local.join("meetings"))?;
            // Single instance guarantees this process owns the meetings folder.
            store.recover();
            app.manage(store);
            app.manage(audio::Recorder::default());
            let settings =
                settings::SettingsStore::new(app.path().app_data_dir()?.join("settings.json"));
            if let Err(e) = settings.reset_gpu_failed_once(&app.package_info().version.to_string())
            {
                log::warn!("gpu_failed 초기화 실패: {e}");
            }
            app.manage(settings);
            app.manage(transcribe::model::Models::new(local.join("models")));
            app.manage(queue::Queue::new(
                app.handle().clone(),
                std::env::current_exe()?,
            ));
            app.manage(slack::Slack::default());
            app.manage(shape::Shape::default());
            app.manage(audio::mix::Exports::default());
            // APP-03: a click on a notification shows the app on that meeting. Registered before
            // any is shown: the plugin reports clicks only on ones shown after a handler exists.
            let handle = app.handle().clone();
            app.notification().on_action(move |performed| {
                if performed.action_id() != "tap" {
                    return;
                }
                show_main_window(&handle);
                let meeting = performed
                    .notification()
                    .and_then(|n| n.extra().get("meeting"))
                    .and_then(|id| id.as_str());
                if let Some(id) = meeting {
                    let _ = handle.emit("app://open-meeting", id);
                }
            })?;
            // TRN-01: what recovery left `recorded` or `paused` goes back in line, oldest first,
            // and `transcribed` too once an LLM connection exists.
            app.state::<queue::Queue>().enqueue_pending();

            let tray_menu = Menu::with_items(
                app,
                &[
                    &MenuItem::with_id(app, "open", "열기", true, None::<&str>)?,
                    &MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?,
                ],
            )?;
            let mut tray = TrayIconBuilder::with_id(TRAY)
                .tooltip("회의 요약")
                .menu(&tray_menu);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            refresh_tray(app.handle());

            // Last, once every command's state is managed: the page may call them as it loads. Built
            // here (`create: false`) so Windows can go without its frame, the app header holding the
            // window controls there. macOS keeps its traffic lights, over the header
            // (`titleBarStyle: Overlay`, design-system 6.1).
            let window = tauri::WebviewWindowBuilder::from_config(
                app.handle(),
                &app.config().app.windows[0],
            )?;
            #[cfg(windows)]
            let window = window.decorations(false);
            window.build()?;
            Ok(())
        })
        // Handles both the app menu and the tray menu.
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        // The app lives in the tray: closing the window only hides it (APP-01).
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
            if let WindowEvent::Focused(true) = event {
                shape::refit(window);
            }
            #[cfg(target_os = "macos")]
            if clears_notifications(window.label(), event) {
                clear_notifications();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::reset_settings,
            commands::set_secret,
            commands::has_secret,
            commands::delete_secret,
            commands::list_meetings,
            commands::get_meeting,
            commands::get_transcript,
            commands::delete_meeting,
            commands::resume_meeting,
            commands::pause_meeting,
            commands::transcribe_languages,
            commands::set_meeting_language,
            commands::retranscribe,
            commands::start_recording,
            commands::stop_recording,
            commands::recording_state,
            commands::import_recording,
            commands::list_audio_devices,
            commands::test_system_audio,
            commands::test_microphone,
            commands::confirm_quit,
            commands::request_quit,
            commands::queue_state,
            commands::model_status,
            commands::download_models,
            commands::cancel_model_download,
            commands::import_model,
            commands::summary_route,
            commands::set_summarize_local,
            commands::save_summary,
            commands::archive_meeting,
            commands::retry_summary,
            commands::list_connections,
            commands::export_markdown,
            commands::export_recording,
            commands::slack_length,
            commands::send_to_slack,
            commands::slack_sign_in,
            commands::slack_cancel_sign_in,
            commands::slack_sign_out,
            commands::slack_channels,
            commands::open_log_dir,
            shape::set_window_shape,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            // Tray / app-menu quit: ask first if work is running; `confirm_quit` finishes it.
            RunEvent::ExitRequested { api, .. } => {
                let store = app.try_state::<store::Store>();
                if must_confirm_exit(QUIT_CONFIRMED.load(Ordering::SeqCst), store.as_deref()) {
                    api.prevent_exit();
                    show_main_window(app);
                    let _ = app.emit("app://quit-requested", ());
                }
            }
            // Every exit path, including Dock quit and OS shutdown, which skip ExitRequested.
            // The recording goes first: its WAV files are finalized and it becomes `recorded`.
            RunEvent::Exit => {
                // Workers first: child processes would outlive the app.
                if let Some(queue) = app.try_state::<queue::Queue>() {
                    queue.shutdown();
                }
                if let Some(recorder) = app.try_state::<audio::Recorder>() {
                    recorder.stop(None);
                }
                if let Some(store) = app.try_state::<store::Store>() {
                    store.pause_busy();
                }
                if let Some(exports) = app.try_state::<audio::mix::Exports>() {
                    exports.cancel_all();
                }
            }
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => show_main_window(app),
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tray_shows_the_most_urgent_state_and_tells_them_all() {
        assert_eq!(tray_state(false, 0, 0), (None, "회의 요약".into()));
        assert_eq!(
            tray_state(true, 2, 3),
            (Some(RECORDING_DOT), "회의 요약 · 녹음 중 · 미전송 3".into())
        );
        assert_eq!(
            tray_state(false, 2, 3),
            (
                Some(PROCESSING_DOT),
                "회의 요약 · 처리 중 2 · 미전송 3".into()
            )
        );
        assert_eq!(
            tray_state(false, 0, 1),
            (Some(UNSENT_DOT), "회의 요약 · 미전송 1".into())
        );
    }

    #[test]
    fn the_dot_covers_the_bottom_right_corner_only() {
        // A transparent 10x10 icon: the dot is 4 px across, with a white rim.
        let (w, h) = (10, 10);
        let out = with_dot(&vec![0; (w * h * 4) as usize], w, h, UNSENT_DOT);
        let px = |x: u32, y: u32| {
            let at = ((y * w + x) * 4) as usize;
            [out[at], out[at + 1], out[at + 2], out[at + 3]]
        };
        assert_eq!(px(8, 8), [0xff, 0x9b, 0x3f, 255]);
        assert_eq!(px(7, 7), [0xff, 0x9b, 0x3f, 255]);
        assert_eq!(px(6, 8), [255, 255, 255, 255]);
        // Outside the circle and outside the corner, the icon is untouched.
        assert_eq!(px(6, 6), [0; 4]);
        assert_eq!(px(0, 0), [0; 4]);
        assert_eq!(px(9, 0), [0; 4]);
        let painted = out.chunks(4).filter(|p| p[3] == 255).count();
        assert!((10..=16).contains(&painted), "{painted}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_the_main_window_gaining_focus_clears_notifications() {
        assert!(clears_notifications("main", &WindowEvent::Focused(true)));
        assert!(!clears_notifications("main", &WindowEvent::Focused(false)));
        assert!(!clears_notifications("main", &WindowEvent::Destroyed));
        assert!(!clears_notifications("other", &WindowEvent::Focused(true)));
        // The test binary has no bundle identifier, like a bare binary: nothing to clear, no panic.
        clear_notifications();
    }

    #[cfg(windows)]
    #[test]
    fn ecoqos_reads_back_as_off() {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, GetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
            ProcessPowerThrottling,
        };
        assert_eq!(no_ecoqos(None), Ok(()));
        let mut state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ..Default::default()
        };
        // SAFETY: our own pseudo handle; `state` is the size given and outlives the call.
        let read = unsafe {
            GetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                (&raw mut state).cast(),
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
        };
        // The product only sets it: a Windows that cannot read it back (ERROR_INVALID_PARAMETER on
        // older builds) has nothing more to check.
        if read == 0 {
            eprintln!("GetProcessInformation: {}", std::io::Error::last_os_error());
            return;
        }
        // Execution speed is controlled by us, and not throttled.
        let speed = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
        assert_eq!(
            (state.ControlMask & speed, state.StateMask & speed),
            (speed, 0)
        );
    }

    #[test]
    fn confirmed_quit_is_not_blocked_by_a_meeting_that_cannot_be_paused() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store::Store::new(tmp.path().join("meetings")).unwrap();
        let id = store
            .create(
                "녹음 중인 회의",
                false,
                crate::transcribe::lang::default_list(),
            )
            .unwrap()
            .id;
        // meta.json can no longer be rewritten: the temp file path is taken by a folder.
        std::fs::create_dir(tmp.path().join("meetings").join(&id).join("meta.json.tmp")).unwrap();

        assert!(must_confirm_exit(false, Some(&store)));
        store.pause_busy();
        assert_eq!(store.busy_ids(), [id]);
        assert!(must_confirm_exit(false, Some(&store)));
        assert!(!must_confirm_exit(true, Some(&store)));
    }
}
