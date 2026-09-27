use boris_pipeline::{DownloadProgress, ModelsInstallReport, ModelsStatus};
use tauri::{AppHandle, Emitter};

use super::EVENT_MODELS_PROGRESS;

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
