/**
 * Desktop IPC bridge — the only UI entrypoint into the Tauri host.
 *
 * Import from `@/bridge` in windows/components. Do not `invoke` host commands
 * ad-hoc except for logger plumbing (`frontend_log` / `get_log_path`). Put
 * command implementations under `commands/`, grouped by their IPC topic.
 */

export { invokeErrorMessage } from "./errors";
export { COMMANDS, EVENTS, type CommandName, type EventName } from "./ipc";
export {
  getStatus,
  onStatus,
  preflightCheck,
  startEngine,
  stopEngine,
  submitInput,
  cancelInput,
  wakeLivenessStatus,
  startWakeEnroll,
  clearWakeProfile,
  type StartEngineOptions,
} from "./commands/engine";
export {
  listInputDevices,
  listOutputDevices,
  switchInput,
  switchOutput,
} from "./commands/devices";
export {
  downloadModels,
  getModelsStatus,
  onModelsProgress,
} from "./commands/models";
export { getSettings, saveSettings } from "./commands/settings";
export { getDebugCapture, setDebugCapture, clearDebugCapture, type DebugEvent, type DebugSnapshot } from "./commands/diagnostics";
export {
  getSessionArtifact,
  listSessionArtifacts,
} from "./commands/artifacts";
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
  type LivenessStatus,
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
  type WakeEnrollPeek,
  type InputPeek,
} from "./types";
