/**
 * Desktop IPC bridge — the only UI entrypoint into the Tauri host.
 *
 * Import from `@/bridge` in windows/components. Do not `invoke` host commands
 * ad-hoc except for logger plumbing (`frontend_log` / `get_log_path`).
 */

export { invokeErrorMessage } from "./errors";
export { COMMANDS, EVENTS, type CommandName, type EventName } from "./ipc";
export {
  downloadModels,
  getModelsStatus,
  getSessionArtifact,
  getSettings,
  getStatus,
  listInputDevices,
  listOutputDevices,
  listSessionArtifacts,
  onModelsProgress,
  onStatus,
  preflightCheck,
  saveSettings,
  startEngine,
  stopEngine,
  switchInput,
  switchOutput,
  type StartEngineOptions,
} from "./status";
export { useStatus } from "./useStatus";
export {
  EMPTY_SETTINGS,
  formatContextMeter,
  MODEL_PRESETS,
  normalizeSettings,
  normalizeStatus,
  normalizeUpdateChannel,
  OFF_STATUS,
  PROVIDER_PRESETS,
  settingsToWire,
  type AppSettings,
  type UpdateChannel,
  type ArtifactCard,
  type ArtifactListItem,
  type ArtifactPeek,
  type DeviceDto,
  type DeviceHealth,
  type DownloadFileStatus,
  type DownloadProgress,
  type EngineState,
  type ModelComponent,
  type ModelsInstallReport,
  type ModelsStatus,
  type Phase,
  type PreflightReport,
  type StatusPicture,
} from "./types";
