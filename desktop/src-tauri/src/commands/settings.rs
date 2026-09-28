use boris_pipeline::{load_settings, save_settings, AppSettings};
use tauri::{AppHandle, Manager};

use crate::{autostart, orchestrator::AppState, overlay_win};

/// Restore API keys + models/providers from `~/.boris/config.toml` + `auth.json`.
///
/// Off the UI thread: first-run migration + disk reads must not freeze launch.
#[tauri::command]
pub async fn get_settings() -> Result<AppSettings, String> {
    let result = tauri::async_runtime::spawn_blocking(load_settings)
        .await
        .map_err(|e| {
            let msg = format!("get_settings task failed: {e}");
            tracing::error!(error = %msg, "get_settings join failed");
            msg
        })?;

    match result {
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
///
/// Off the UI thread: config merge + disk write used to freeze the window on
/// every settings keystroke, and again on Start (which saves first).
#[tauri::command]
pub async fn save_app_settings(app: AppHandle, settings: AppSettings) -> Result<(), String> {
    tracing::info!(
        has_openrouter_key = !settings.openrouter_api_key.trim().is_empty(),
        has_exa_key = !settings.exa_api_key.trim().is_empty(),
        model = %settings.openrouter_model,
        fast_model = %settings.openrouter_fast_model,
        model_provider = %settings.openrouter_model_provider,
        fast_provider = %settings.openrouter_fast_provider,
        pin_provider = settings.openrouter_pin_provider,
        start_with_windows = settings.start_with_windows,
        update_channel = %settings.update_channel,
        "save_app_settings"
    );
    // Win any in-flight boot settings load so stale disk prefs cannot overwrite.
    overlay_win::mark_overlay_prefs_dirty();
    let to_disk = settings.clone();
    tauri::async_runtime::spawn_blocking(move || {
        save_settings(&to_disk).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| {
        let msg = format!("save_app_settings task failed: {e}");
        tracing::error!(error = %msg, "save_app_settings join failed");
        msg
    })?
    .map_err(|e| {
        tracing::error!(error = %e, "save_app_settings failed");
        e
    })?;

    let want_autostart = settings.start_with_windows;
    tauri::async_runtime::spawn_blocking(move || autostart::apply(want_autostart))
        .await
        .map_err(|e| format!("start-with-windows task failed: {e}"))?
        .map_err(|e| {
            tracing::error!(error = %e, "start-with-windows apply failed");
            e
        })?;

    // Cache + notify only. Do not resize/move/sync the overlay on every
    // save — SetWindowPos on a visible transparent WebView2 flashes a
    // decorated empty pane on Windows 11. Geometry applies when scale or
    // position actually change; visibility syncs only if the user just
    // turned the island on.
    if overlay_win::apply_preferences(&app, &settings) {
        let picture = app.state::<AppState>().status();
        overlay_win::sync_visibility(&app, &picture);
    }
    Ok(())
}
