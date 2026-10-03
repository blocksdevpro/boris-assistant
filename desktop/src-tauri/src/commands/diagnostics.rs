use crate::logging;
use crate::orchestrator::AppState;
use tauri::State;

/// Path hint for the log directory / active file (for UI debug copy).
#[tauri::command]
pub async fn get_log_path() -> String {
    logging::log_path_hint()
}

/// Accept frontend / webview log lines into the same file as Rust.
///
/// Levels: `error` | `warn` | `info` | `debug` (anything else → info).
/// Async so a burst of UI logs cannot stall the Windows message pump.
#[tauri::command]
pub async fn frontend_log(level: String, message: String, context: Option<String>) {
    logging::write_frontend_log(&level, &message, context.as_deref());
}

/// Read only events newer than `after`; the UI polls while its developer view is open.
#[tauri::command]
pub fn get_debug_capture(state: State<'_, AppState>, after: u64) -> boris_pipeline::DebugSnapshot {
    state.debug_snapshot(after)
}

#[tauri::command]
pub fn set_debug_capture(state: State<'_, AppState>, enabled: bool) {
    state.set_debug_capture(enabled);
}

#[tauri::command]
pub fn clear_debug_capture(state: State<'_, AppState>) {
    state.clear_debug_capture();
}
