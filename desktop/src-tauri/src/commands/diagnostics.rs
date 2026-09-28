use crate::logging;

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
