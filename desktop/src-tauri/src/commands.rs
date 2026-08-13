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
//!
//! # Threading
//!
//! Sync `#[tauri::command]` handlers can run on the UI/main thread on Windows.
//! Any work that may take more than a few ms (engine join, model status, cpal
//! device enum, large disk copies) must use `async` + `spawn_blocking` so the
//! window does not freeze as "Not Responding".

use boris_pipeline::{
    load_settings, save_settings, AppSettings, DeviceDto, DownloadProgress, ModelsInstallReport,
    ModelsStatus, PreflightReport, StatusPicture,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::logging;
use crate::orchestrator::AppState;
use crate::overlay_win;

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

/// Catalog of visual cards in the active voice session.
#[tauri::command]
pub async fn list_session_artifacts() -> Result<Vec<boris_pipeline::ArtifactListItem>, String> {
    tauri::async_runtime::spawn_blocking(crate::artifacts::list_current)
        .await
        .map_err(|e| format!("list_session_artifacts join: {e}"))?
}

/// Body + meta for one session card (`id` omitted → current).
#[tauri::command]
pub async fn get_session_artifact(
    id: Option<String>,
) -> Result<boris_pipeline::ArtifactCard, String> {
    tauri::async_runtime::spawn_blocking(move || crate::artifacts::get_current(id))
        .await
        .map_err(|e| format!("get_session_artifact join: {e}"))?
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
///
/// Off the UI thread: preflight may copy bootstrap assets in a dev checkout.
#[tauri::command]
pub async fn preflight_check() -> PreflightReport {
    let report = tauri::async_runtime::spawn_blocking(AppState::preflight)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "preflight_check join failed");
            PreflightReport {
                parakeet_ready: false,
                supertone_ready: false,
                boris_home: String::new(),
                parakeet_dir: String::new(),
                supertone_onnx_dir: String::new(),
                supertone_voices_dir: String::new(),
                ok: false,
                messages: vec![format!("preflight task failed: {e}")],
            }
        });
    tracing::debug!(
        ok = report.ok,
        messages = ?report.messages,
        "preflight_check"
    );
    report
}

/// Start (or rebuild) the voice engine.
///
/// Engine spawn / fingerprint rebuild can join the previous thread — always
/// off the UI thread.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_engine(
    app: AppHandle,
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

    let app_for_status = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.start(
            key,
            model,
            fast_model,
            model_provider,
            fast_provider,
            pin_provider,
            move |picture| {
                // Cached prefs only — never load_settings on the status hot path.
                overlay_win::sync_visibility(&app_for_status, &picture);
                let _ = app_for_status.emit(EVENT_STATUS, picture);
            },
        )
    })
    .await
    .map_err(|e| {
        let msg = format!("start_engine task failed: {e}");
        tracing::error!(error = %msg, "start_engine join failed");
        msg
    })?;

    result.map_err(|e| {
        tracing::error!(error = %e, "start_engine failed");
        e
    })?;
    tracing::info!("start_engine ok");
    Ok(())
}

/// Stop and join the engine thread (may take a moment while audio shuts down).
#[tauri::command]
pub async fn stop_engine(app: AppHandle) -> Result<(), String> {
    tracing::info!("stop_engine command");
    let app_for_overlay = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.stop()?;
        let picture = state.status();
        overlay_win::sync_visibility(&app_for_overlay, &picture);
        let _ = app_for_overlay.emit(EVENT_STATUS, picture);
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| {
        let msg = format!("stop_engine task failed: {e}");
        tracing::error!(error = %msg, "stop_engine join failed");
        msg
    })?;

    result.map_err(|e| {
        tracing::error!(error = %e, "stop_engine failed");
        e
    })
}

#[tauri::command]
pub async fn list_input_devices() -> Vec<DeviceDto> {
    let list = tauri::async_runtime::spawn_blocking(AppState::list_inputs)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "list_input_devices join failed");
            Vec::new()
        });
    tracing::debug!(count = list.len(), "list_input_devices");
    list
}

#[tauri::command]
pub async fn list_output_devices() -> Vec<DeviceDto> {
    let list = tauri::async_runtime::spawn_blocking(AppState::list_outputs)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "list_output_devices join failed");
            Vec::new()
        });
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

/// Model readiness under `~/.boris/models` (metadata only — see pipeline docs).
#[tauri::command]
pub async fn models_status() -> ModelsStatus {
    let status = tauri::async_runtime::spawn_blocking(boris_pipeline::models_status)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "models_status join failed");
            ModelsStatus {
                home: String::new(),
                models_dir: String::new(),
                parakeet_ready: false,
                parakeet_dir: String::new(),
                supertone_ready: false,
                supertone_onnx_dir: String::new(),
                supertone_voices_dir: String::new(),
                missing: vec![format!("status task failed: {e}")],
                base_url_override: None,
            }
        });
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
        let msg = e.to_string();
        tracing::error!(error = %msg, "download_models install failed");
        msg
    })?;

    tracing::info!(
        ok = report.ok,
        downloaded = report.files_downloaded,
        failed = report.files_failed,
        "download_models finished"
    );
    Ok(report)
}

/// Restore API keys + models/providers from `~/.boris/config.toml` + `auth.json`.
#[tauri::command]
pub fn get_settings() -> Result<AppSettings, String> {
    match load_settings() {
        Ok(s) => {
            // Keep status-path overlay cache aligned if setup loaded defaults only.
            overlay_win::remember_overlay_prefs(&s);
            tracing::debug!(
                has_openrouter_key = !s.openrouter_api_key.trim().is_empty(),
                has_exa_key = !s.exa_api_key.trim().is_empty(),
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
            Err(e.to_string())
        }
    }
}

/// Persist prefs to `config.toml` and secrets to `auth.json` (never log key values).
#[tauri::command]
pub fn save_app_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<(), String> {
    tracing::info!(
        has_openrouter_key = !settings.openrouter_api_key.trim().is_empty(),
        has_exa_key = !settings.exa_api_key.trim().is_empty(),
        model = %settings.openrouter_model,
        fast_model = %settings.openrouter_fast_model,
        model_provider = %settings.openrouter_model_provider,
        fast_provider = %settings.openrouter_fast_provider,
        pin_provider = settings.openrouter_pin_provider,
        update_channel = %settings.update_channel,
        "save_app_settings"
    );
    save_settings(&settings).map_err(|e| {
        tracing::error!(error = %e, "save_app_settings failed");
        e.to_string()
    })?;
    // apply_preferences also refreshes the overlay-prefs cache used by status.
    overlay_win::apply_preferences(&app, &settings);
    overlay_win::sync_visibility(&app, &state.status());
    Ok(())
}
