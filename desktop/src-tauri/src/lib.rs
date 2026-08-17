//! Boris desktop host (Tauri v2).
//!
//! # Host vs pipeline
//!
//! | Layer | Crate / module | Owns |
//! |-------|----------------|------|
//! | **Host** (this crate) | `commands`, `orchestrator`, `tray`, `overlay_win`, `logging` | Windows, tray, IPC, engine lifecycle, status mirror, app updater plugins |
//! | **Pipeline** | `boris_pipeline` | Voice engine, STT/TTS, agent turns, `~/.boris` paths/settings/download |
//!
//! Keep this crate thin: no wake/VAD policy, no tool execution, no model math.
//! The React UI talks only through [`commands`] (stable invoke names) and
//! status / models-progress events — see `desktop/src/bridge`.
//!
//! # Module map
//!
//! - [`commands`] — `#[tauri::command]` handlers + event name constants
//! - [`orchestrator`] — `AppState`: spawn engine, Start/Stop, device prefs
//! - [`tray`] — system tray (show console / quit / overlay lock)
//! - [`overlay_win`] — always-on-top click-through island
//! - [`logging`] — file + stderr tracing, panic hook, frontend log sink

mod artifacts;
mod autostart;
mod commands;
mod logging;
mod orchestrator;
mod overlay_win;
mod tray;
mod updater;

use orchestrator::AppState;
use tauri::{Emitter, Manager, WindowEvent};

/// Process entry used by `main.rs` (and mobile entry when enabled).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init_tracing();
    tracing::info!("starting Tauri app");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::preflight_check,
            commands::start_engine,
            commands::stop_engine,
            commands::wake_liveness_status,
            commands::start_wake_enroll,
            commands::clear_wake_profile,
            commands::list_input_devices,
            commands::list_output_devices,
            commands::switch_input,
            commands::switch_output,
            commands::models_status,
            commands::download_models,
            commands::get_settings,
            commands::save_app_settings,
            commands::get_log_path,
            commands::frontend_log,
            commands::list_session_artifacts,
            commands::get_session_artifact,
            updater::check_app_update,
        ])
        .setup(setup_app)
        // Closing the console hides it; only tray "Quit Boris" exits the app.
        .on_window_event(on_main_window_event)
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// One-time shell setup after the Tauri runtime is ready.
fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("app setup begin");

    // `--autostart` (Windows logon): stay in the tray. Hide before the first
    // paint so the console never flashes. Normal launches show immediately
    // (tauri.conf keeps main `visible: false` to make this race-free).
    let silent = autostart::launched_from_windows_startup();
    if let Some(main) = app.get_webview_window("main") {
        if silent {
            let _ = main.hide();
            let _ = main.set_skip_taskbar(true);
            tracing::info!("windows startup launch — main window hidden");
        } else {
            let _ = main.show();
        }
        tracing::info!(
            silent,
            visible = main.is_visible().unwrap_or(false),
            "main window present"
        );
    } else {
        tracing::warn!("main window missing at setup");
    }

    // Do not create the overlay HWND here. Packaged WebView2 on Windows
    // flashes a decorated transparent frame during window build — that is
    // the empty window on launch. The island is spawned on first show.
    // Seed persisted overlay prefs off the UI thread so first-run migration
    // cannot freeze launch. Cache only — no hide/show/resize.
    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let settings =
            match tauri::async_runtime::spawn_blocking(boris_pipeline::load_settings).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "deferred boot settings load failed");
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "deferred boot settings join failed");
                    return;
                }
            };

        // Refresh the Run-key path after updates; drop a stale entry if unset.
        if let Err(e) = autostart::apply(settings.start_with_windows) {
            tracing::warn!(error = %e, "apply start-with-windows at boot");
        }

        if overlay_win::overlay_prefs_dirty() {
            tracing::debug!("boot settings load skipped — newer prefs already applied");
        } else {
            let _ = overlay_win::apply_preferences(&handle, &settings);
        }

        if !silent {
            return;
        }
        if !settings.start_with_windows {
            // Leftover Run key — do not stay silent; show the console.
            tracing::info!("--autostart but start_with_windows is off — showing main window");
            if let Some(main) = handle.get_webview_window("main") {
                let _ = main.set_skip_taskbar(false);
                let _ = main.show();
            }
            return;
        }

        let boot = handle.clone();
        let saved = settings.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            commands::start_engine_with_settings(&boot, &saved)
        })
        .await
        {
            Ok(Ok(())) => tracing::info!("silent start: engine on"),
            Ok(Err(e)) => tracing::warn!(error = %e, "silent start: engine failed"),
            Err(e) => tracing::warn!(error = %e, "silent start: engine join failed"),
        }
    });

    // Tray keeps control after the main console is closed/hidden.
    if let Err(e) = tray::setup_tray(app.handle()) {
        tracing::error!(error = %e, "failed to create system tray");
    }

    if let Some(state) = app.try_state::<AppState>() {
        let _ = app.emit(commands::EVENT_STATUS, state.status());
    }

    tracing::info!(
        log_hint = %logging::log_path_hint(),
        "app setup complete — logs written under ~/.boris/logs"
    );
    Ok(())
}

/// Hide the main console on close instead of exiting (tray owns full quit).
fn on_main_window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != "main" {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
        tracing::info!("main window hidden to tray");
    }
}
