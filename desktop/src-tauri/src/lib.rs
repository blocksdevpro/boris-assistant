mod orchestrator;
mod overlay_win;
mod tray;

use boris_pipeline::{
    load_settings, save_settings, AppSettings, DeviceDto, DownloadProgress, ModelsInstallReport,
    ModelsStatus, PreflightReport, StatusPicture,
};
use orchestrator::AppState;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into())
                .add_directive("boris_pipeline=info".parse().unwrap())
                .add_directive("boris_audio=info".parse().unwrap())
                .add_directive("boris_sense=info".parse().unwrap())
                .add_directive("boris_desktop=info".parse().unwrap()),
        )
        .try_init();
}

#[tauri::command]
fn get_status(state: State<'_, AppState>) -> StatusPicture {
    state.status()
}

/// Model readiness gate for the UI (paths under `~/.boris`).
#[tauri::command]
fn preflight_check() -> PreflightReport {
    AppState::preflight()
}

#[tauri::command]
fn start_engine(
    app: AppHandle,
    state: State<'_, AppState>,
    api_key: String,
    model: Option<String>,
) -> Result<(), String> {
    let key = if api_key.trim().is_empty() {
        std::env::var("OPENROUTER_API_KEY").unwrap_or_default()
    } else {
        api_key
    };
    let model = model.or_else(|| std::env::var("OPENROUTER_MODEL").ok());

    state.start(key, model, move |picture| {
        let _ = app.emit("status", picture);
    })
}

#[tauri::command]
fn stop_engine(state: State<'_, AppState>) -> Result<(), String> {
    state.stop()
}

#[tauri::command]
fn list_input_devices() -> Vec<DeviceDto> {
    AppState::list_inputs()
}

#[tauri::command]
fn list_output_devices() -> Vec<DeviceDto> {
    AppState::list_outputs()
}

#[tauri::command]
fn switch_input(state: State<'_, AppState>, device_id: String) -> Result<(), String> {
    state.switch_input(device_id)
}

#[tauri::command]
fn switch_output(state: State<'_, AppState>, device_id: String) -> Result<(), String> {
    state.switch_output(device_id)
}

#[tauri::command]
fn models_status() -> ModelsStatus {
    boris_pipeline::models_status()
}

/// Download missing STT/TTS models into `~/.boris/models`.
///
/// Runs on a worker thread so the UI stays responsive for multi-hundred-MB
/// transfers. Emits `models-progress` ([`DownloadProgress`]) while running.
#[tauri::command]
async fn download_models(app: AppHandle) -> Result<ModelsInstallReport, String> {
    // Blocking reqwest must not run on the async/UI path — that freezes the
    // window ("Not Responding") for the entire install (~900 MB).
    tauri::async_runtime::spawn_blocking(move || {
        boris_pipeline::install_models(|progress: DownloadProgress| {
            let _ = app.emit("models-progress", &progress);
        })
    })
    .await
    .map_err(|e| format!("download task failed: {e}"))?
}

/// Restore OpenRouter key + model from `~/.boris/settings.json`.
#[tauri::command]
fn get_settings() -> Result<AppSettings, String> {
    load_settings()
}

/// Persist OpenRouter key + model to `~/.boris/settings.json` (never logged).
#[tauri::command]
fn save_app_settings(settings: AppSettings) -> Result<(), String> {
    save_settings(&settings)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            get_status,
            preflight_check,
            start_engine,
            stop_engine,
            list_input_devices,
            list_output_devices,
            switch_input,
            switch_output,
            models_status,
            download_models,
            get_settings,
            save_app_settings,
        ])
        .setup(|app| {
            // Overlay is `"create": false` in config — build with explicit transparent API.
            overlay_win::spawn_overlay_window(app.handle())?;

            // Tray keeps control after the main console is closed/hidden.
            if let Err(e) = tray::setup_tray(app.handle()) {
                tracing::error!(error = %e, "failed to create system tray");
            }

            if let Some(state) = app.try_state::<AppState>() {
                let _ = app.emit("status", state.status());
            }
            Ok(())
        })
        // Closing the console hides it; only tray "Quit Boris" exits the app.
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                tracing::info!("main window hidden to tray");
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
