use boris_pipeline::{AppSettings, DeviceDto};
use tauri::{AppHandle, Emitter, Manager, State};

use super::EVENT_STATUS;
use crate::{orchestrator::AppState, overlay_win};

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

/// Start from saved prefs (Windows silent logon). Applies device prefs first.
pub(crate) fn start_engine_with_settings(
    app: &AppHandle,
    settings: &AppSettings,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !settings.input_device.trim().is_empty() {
        if let Err(e) = state.switch_input(settings.input_device.clone()) {
            tracing::warn!(error = %e, "silent start: switch_input failed");
        }
    }
    if !settings.output_device.trim().is_empty() {
        if let Err(e) = state.switch_output(settings.output_device.clone()) {
            tracing::warn!(error = %e, "silent start: switch_output failed");
        }
    }

    let key_from_env = settings.openrouter_api_key.trim().is_empty();
    let key = if key_from_env {
        std::env::var("OPENROUTER_API_KEY").unwrap_or_default()
    } else {
        settings.openrouter_api_key.clone()
    };
    let model =
        nonempty_opt(&settings.openrouter_model).or_else(|| std::env::var("OPENROUTER_MODEL").ok());
    let fast_model = nonempty_opt(&settings.openrouter_fast_model);
    let model_provider = nonempty_opt(&settings.openrouter_model_provider);
    let fast_provider = nonempty_opt(&settings.openrouter_fast_provider);
    let pin_provider = Some(settings.openrouter_pin_provider);

    tracing::info!(
        key_source = if key_from_env { "env" } else { "settings" },
        key_present = !key.trim().is_empty(),
        model = ?model.as_deref(),
        "silent start_engine"
    );

    let app_for_status = app.clone();
    state.start(
        key,
        model,
        fast_model,
        model_provider,
        fast_provider,
        pin_provider,
        move |picture| {
            overlay_win::sync_visibility(&app_for_status, &picture);
            let _ = app_for_status.emit(EVENT_STATUS, picture);
        },
    )
}

fn nonempty_opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Stop and join the engine thread (may take a moment while audio shuts down).
#[tauri::command]
pub async fn stop_engine(app: AppHandle) -> Result<(), String> {
    tracing::info!("stop_engine command");
    let app_for_overlay = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.stop()?;
        state.with_status(|picture| {
            overlay_win::sync_visibility(&app_for_overlay, &picture);
            let _ = app_for_overlay.emit(EVENT_STATUS, picture);
        });
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
pub fn wake_liveness_status() -> boris_pipeline::LivenessStatus {
    boris_pipeline::liveness_status()
}

#[tauri::command]
pub fn start_wake_enroll(state: State<'_, AppState>, takes: Option<u32>) -> Result<(), String> {
    state.start_wake_enroll(takes.unwrap_or(4))
}

#[tauri::command]
pub fn clear_wake_profile(state: State<'_, AppState>) -> Result<(), String> {
    state.clear_wake_profile()
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
pub async fn switch_input(app: AppHandle, device_id: String) -> Result<(), String> {
    tracing::info!(%device_id, "switch_input command");
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.switch_input(device_id)
    })
    .await
    .map_err(|e| format!("switch_input task failed: {e}"))?
    .map_err(|e| {
        tracing::error!(error = %e, "switch_input failed");
        e
    })
}

#[tauri::command]
pub async fn submit_input(app: AppHandle, id: String, value: String) -> Result<(), String> {
    tracing::info!(%id, chars = value.chars().count(), "submit_input command");
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.submit_input(id, value)
    })
    .await
    .map_err(|e| format!("submit_input task failed: {e}"))?
}

#[tauri::command]
pub async fn cancel_input(app: AppHandle, id: String) -> Result<(), String> {
    tracing::info!(%id, "cancel_input command");
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.cancel_input(id)
    })
    .await
    .map_err(|e| format!("cancel_input task failed: {e}"))?
}

#[tauri::command]
pub async fn switch_output(app: AppHandle, device_id: String) -> Result<(), String> {
    tracing::info!(%device_id, "switch_output command");
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.switch_output(device_id)
    })
    .await
    .map_err(|e| format!("switch_output task failed: {e}"))?
    .map_err(|e| {
        tracing::error!(error = %e, "switch_output failed");
        e
    })
}
