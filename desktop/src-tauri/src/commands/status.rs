use boris_pipeline::{PreflightReport, StatusPicture};
use tauri::State;

use crate::orchestrator::AppState;

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
