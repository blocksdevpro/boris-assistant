/**
 * Stable Tauri IPC names for the desktop host.
 *
 * **Contract:** these string values must match Rust:
 * - commands → `desktop/src-tauri/src/commands.rs` (`#[tauri::command]` fn names)
 * - events   → `commands::EVENT_*` constants
 *
 * Rename only in an atomic host + bridge PR.
 */

/** `invoke` command names (Rust handler function names). */
export const COMMANDS = {
  getStatus: "get_status",
  preflightCheck: "preflight_check",
  startEngine: "start_engine",
  stopEngine: "stop_engine",
  listInputDevices: "list_input_devices",
  listOutputDevices: "list_output_devices",
  switchInput: "switch_input",
  switchOutput: "switch_output",
  modelsStatus: "models_status",
  downloadModels: "download_models",
  getSettings: "get_settings",
  saveAppSettings: "save_app_settings",
  getLogPath: "get_log_path",
  frontendLog: "frontend_log",
  listSessionArtifacts: "list_session_artifacts",
  getSessionArtifact: "get_session_artifact",
  checkAppUpdate: "check_app_update",
} as const;

/** Event names emitted by the host (`app.emit`). */
export const EVENTS = {
  /** Payload: `StatusPicture` */
  status: "status",
  /** Payload: `DownloadProgress` */
  modelsProgress: "models-progress",
} as const;

export type CommandName = (typeof COMMANDS)[keyof typeof COMMANDS];
export type EventName = (typeof EVENTS)[keyof typeof EVENTS];
