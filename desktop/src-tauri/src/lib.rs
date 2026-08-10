//! Boris desktop host (Tauri v2).
//!
//! # Host vs pipeline
//!
//! | Layer | Crate / module | Owns |
//! |-------|----------------|------|
//! | **Host** (this crate) | `commands`, `orchestrator`, `tray`, `overlay_win`, `logging` | Windows, tray, IPC, engine lifecycle, status mirror |
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

mod commands;
mod logging;
mod orchestrator;
mod overlay_win;
mod tray;

use orchestrator::AppState;
use tauri::{Emitter, Manager, WindowEvent};

/// Process entry used by `main.rs` (and mobile entry when enabled).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init_tracing();
    tracing::info!("starting Tauri app");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::preflight_check,
            commands::start_engine,
            commands::stop_engine,
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

    // Overlay is `"create": false` in config — build with explicit transparent API.
    match overlay_win::spawn_overlay_window(app.handle()) {
        Ok(()) => tracing::info!("overlay window spawned"),
        Err(e) => {
            tracing::error!(error = %e, "failed to spawn overlay window");
            return Err(e.into());
        }
    }

    // Tray keeps control after the main console is closed/hidden.
    if let Err(e) = tray::setup_tray(app.handle()) {
        tracing::error!(error = %e, "failed to create system tray");
    }

    if let Some(state) = app.try_state::<AppState>() {
        let _ = app.emit(commands::EVENT_STATUS, state.status());
    }

    if let Some(main) = app.get_webview_window("main") {
        tracing::info!(visible = main.is_visible().unwrap_or(false), "main window present");
    } else {
        tracing::warn!("main window missing at setup");
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
