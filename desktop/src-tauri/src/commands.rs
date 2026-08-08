//! Tauri `invoke` handlers — the desktop IPC surface.
//!
//! # Responsibility
//!
//! Thin adapters from the React bridge (`desktop/src/bridge`) onto
//! [`crate::orchestrator::AppState`] and `boris_pipeline` data-plane helpers.
//! **No voice policy lives here** — start/stop/device/settings are forwarded.
//!
//! # Stability contract
//!
//! Command **names** and event **names** are part of the public IPC contract.
//! Keep them in sync with `desktop/src/bridge/ipc.ts`. Do not rename without
//! updating the TypeScript bridge in the same change.

use boris_pipeline::{
    load_settings, save_settings, AppSettings, DeviceDto, DownloadProgress, ModelsInstallReport,
    ModelsStatus, PreflightReport, StatusPicture,
};
use tauri::{AppHandle, Emitter, State};

use crate::logging;
use crate::orchestrator::AppState;

// ── Event names (emitted to the webview) ─────────────────────────────────────

/// Status snapshot push — payload: [`StatusPicture`].
pub const EVENT_STATUS: &str = "status";

/// Model download progress — payload: [`DownloadProgress`].
pub const EVENT_MODELS_PROGRESS: &str = "models-progress";

// ── Commands ─────────────────────────────────────────────────────────────────

/// Path hint for the log directory / active file (for UI debug copy).
#[tauri::command]
pub fn get_log_path() -> String {
    logging::log_path_hint()
}

/// Accept frontend / webview log lines into the same file as Rust.
///
/// Levels: `error` | `warn` | `info` | `debug` (anything else → info).
#[tauri::command]
pub fn frontend_log(level: String, message: String, context: Option<String>) {
    logging::write_frontend_log(&level, &message, context.as_deref());
}

#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> StatusPicture {
    state.status()
}

/// Model readiness gate for the UI (paths under `~/.boris`).
#[tauri::command]
pub fn preflight_check() -> PreflightReport {
    let report = AppState::preflight();
    tracing::debug!(
        ok = report.ok,
        messages = ?report.messages,
        "preflight_check"
    );
    report
}

#[tauri::command]
pub fn start_engine(
    app: AppHandle,
    state: State<'_, AppState>,
    api_key: String,
    model: Option<String>,
    fast_model: Option<String>,
    model_provider: Option<String>,
    fast_provider: Option<String>,
    pin_provider: Option<bool>,
) -> Result<(), String> {
    let key_from_env = api_key.trim().is_empty();
    let key = if key_from_env {
        std::env::var("OPENROUTER_API_KEY").unwrap_or_default()
    } else {
        api_key
    };
    // Env fallbacks also applied inside PipelineConfig; keep model env here for logs.
    let model = model.or_else(|| std::env::var("OPENROUTER_MODEL").ok());

    tracing::info!(
        key_source = if key_from_env { "env" } else { "ui" },
        key_present = !key.trim().is_empty(),
        model = ?model.as_deref(),
        fast_model = ?fast_model.as_deref(),
        model_provider = ?model_provider.as_deref(),
        fast_provider = ?fast_provider.as_deref(),
        pin_provider = ?pin_provider,
        "start_engine command"
    );

    state
        .start(
            key,
            model,
            fast_model,
            model_provider,
            fast_provider,
            pin_provider,
            move |picture| {
                let _ = app.emit(EVENT_STATUS, picture);
            },
        )
        .map_err(|e| {
            tracing::error!(error = %e, "start_engine failed");
            e
        })
        .map(|()| {
            tracing::info!("start_engine ok");
        })
}

#[tauri::command]
pub fn stop_engine(state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("stop_engine command");
    state.stop().map_err(|e| {
        tracing::error!(error = %e, "stop_engine failed");
        e
    })
}

#[tauri::command]
pub fn list_input_devices() -> Vec<DeviceDto> {
    let list = AppState::list_inputs();
    tracing::debug!(count = list.len(), "list_input_devices");
    list
}

#[tauri::command]
pub fn list_output_devices() -> Vec<DeviceDto> {
    let list = AppState::list_outputs();
    tracing::debug!(count = list.len(), "list_output_devices");
    list
}

#[tauri::command]
pub fn switch_input(state: State<'_, AppState>, device_id: String) -> Result<(), String> {
    tracing::info!(%device_id, "switch_input command");
    state.switch_input(device_id).map_err(|e| {
        tracing::error!(error = %e, "switch_input failed");
        e
    })
}

#[tauri::command]
pub fn switch_output(state: State<'_, AppState>, device_id: String) -> Result<(), String> {
    tracing::info!(%device_id, "switch_output command");
    state.switch_output(device_id).map_err(|e| {
        tracing::error!(error = %e, "switch_output failed");
        e
    })
}

#[tauri::command]
pub fn models_status() -> ModelsStatus {
    let status = boris_pipeline::models_status();
    tracing::debug!(
        parakeet = status.parakeet_ready,
        supertone = status.supertone_ready,
        "models_status"
    );
    status
}

/// Download missing STT/TTS models into `~/.boris/models`.
///
/// Runs on a worker thread so the UI stays responsive for multi-hundred-MB
/// transfers. Emits [`EVENT_MODELS_PROGRESS`] ([`DownloadProgress`]) while running.
#[tauri::command]
pub async fn download_models(app: AppHandle) -> Result<ModelsInstallReport, String> {
    tracing::info!("download_models started");
    // Blocking reqwest must not run on the async/UI path — that freezes the
    // window ("Not Responding") for the entire install (~900 MB).
    let report = tauri::async_runtime::spawn_blocking(move || {
        boris_pipeline::install_models(|progress: DownloadProgress| {
            let _ = app.emit(EVENT_MODELS_PROGRESS, &progress);
        })
    })
    .await
    .map_err(|e| {
        let msg = format!("download task failed: {e}");
        tracing::error!(error = %msg, "download_models join failed");
        msg
    })?
    .map_err(|e| {
        tracing::error!(error = %e, "download_models install failed");
        e
    })?;

    tracing::info!(
        ok = report.ok,
        downloaded = report.files_downloaded,
        failed = report.files_failed,
        "download_models finished"
    );
    Ok(report)
}

/// Restore OpenRouter key + models/providers from `~/.boris/config.toml` + `auth.json`.
#[tauri::command]
pub fn get_settings() -> Result<AppSettings, String> {
    match load_settings() {
        Ok(s) => {
            tracing::debug!(
                has_key = !s.openrouter_api_key.trim().is_empty(),
                model = %s.openrouter_model,
                fast_model = %s.openrouter_fast_model,
                model_provider = %s.openrouter_model_provider,
                fast_provider = %s.openrouter_fast_provider,
                "get_settings ok"
            );
            Ok(s)
        }
        Err(e) => {
            tracing::warn!(error = %e, "get_settings failed");
            Err(e)
        }
    }
}

/// Persist prefs to `config.toml` and key to `auth.json` (never log the key).
#[tauri::command]
pub fn save_app_settings(settings: AppSettings) -> Result<(), String> {
    tracing::info!(
        has_key = !settings.openrouter_api_key.trim().is_empty(),
        model = %settings.openrouter_model,
        fast_model = %settings.openrouter_fast_model,
        model_provider = %settings.openrouter_model_provider,
        fast_provider = %settings.openrouter_fast_provider,
        pin_provider = settings.openrouter_pin_provider,
        "save_app_settings"
    );
    save_settings(&settings).map_err(|e| {
        tracing::error!(error = %e, "save_app_settings failed");
        e
    })
}
