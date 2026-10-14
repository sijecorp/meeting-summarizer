mod audio;
mod commands;
mod settings;
mod store;

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
use tauri::Runtime;
use tauri::menu::{Menu, MenuItem};
#[cfg(target_os = "macos")]
use tauri::menu::{PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        // Must be first: a second launch exits here and focuses the running instance (APP-06).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app)
        }))
        .plugin(tauri_plugin_opener::init());
    #[cfg(target_os = "macos")]
    let builder = builder.menu(app_menu);
    builder
        .setup(|app| {
            // Recordings (and later models) stay out of the Windows roaming profile (8.1).
            let store = store::Store::new(app.path().app_local_data_dir()?.join("meetings"))?;
            // Single instance guarantees this process owns the meetings folder.
            store.recover();
            app.manage(store);
            app.manage(audio::Recorder::default());
            app.manage(settings::SettingsStore::new(
                app.path().app_data_dir()?.join("settings.json"),
            ));

            let tray_menu = Menu::with_items(
                app,
                &[
                    &MenuItem::with_id(app, "open", "열기", true, None::<&str>)?,
                    &MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?,
                ],
            )?;
            let mut tray = TrayIconBuilder::new().tooltip("회의 요약").menu(&tray_menu);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
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
            commands::delete_meeting,
            commands::resume_meeting,
            commands::start_recording,
            commands::stop_recording,
            commands::recording_state,
            commands::list_audio_devices,
            commands::confirm_quit,
            commands::request_quit,
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
                if let Some(recorder) = app.try_state::<audio::Recorder>() {
                    recorder.stop(None);
                }
                if let Some(store) = app.try_state::<store::Store>() {
                    store.pause_busy();
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
    fn confirmed_quit_is_not_blocked_by_a_meeting_that_cannot_be_paused() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store::Store::new(tmp.path().join("meetings")).unwrap();
        let id = store.create("녹음 중인 회의").unwrap().id;
        // meta.json can no longer be rewritten: the temp file path is taken by a folder.
        std::fs::create_dir(tmp.path().join("meetings").join(&id).join("meta.json.tmp")).unwrap();

        assert!(must_confirm_exit(false, Some(&store)));
        store.pause_busy();
        assert_eq!(store.busy_ids(), [id]);
        assert!(must_confirm_exit(false, Some(&store)));
        assert!(!must_confirm_exit(true, Some(&store)));
    }
}
