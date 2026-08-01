/**
 * Typed bridge for engine status + control.
 * Rust owns the source of truth; this is the window/overlay surface.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invokeErrorMessage } from "./errors";
import {
  EMPTY_SETTINGS,
  normalizeStatus,
  OFF_STATUS,
  type AppSettings,
  type DeviceDto,
  type DownloadProgress,
  type ModelsInstallReport,
  type ModelsStatus,
  type PreflightReport,
  type StatusPicture,
} from "./types";

/** Pull once (overlay mount, window focus, etc.). */
export async function getStatus(): Promise<StatusPicture> {
  try {
    const raw = await invoke<StatusPicture>("get_status");
    return normalizeStatus(raw);
  } catch {
    return { ...OFF_STATUS };
  }
}

/** Check whether required STT/TTS models exist under `~/.boris`. */
export async function preflightCheck(): Promise<PreflightReport> {
  try {
    return await invoke<PreflightReport>("preflight_check");
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

/**
 * Subscribe to status pushes from Rust (`emit("status", …)`).
 * Returns an unsubscribe function.
 */
export async function onStatus(
  handler: (picture: StatusPicture) => void,
): Promise<() => void> {
  let unlisten: UnlistenFn | undefined;
  try {
    unlisten = await listen<StatusPicture>("status", (e) => {
      handler(normalizeStatus(e.payload));
    });
  } catch {
    return () => {};
  }
  return () => {
    unlisten?.();
  };
}

export async function startEngine(
  apiKey = "",
  model?: string,
): Promise<void> {
  try {
    await invoke("start_engine", {
      apiKey,
      model: model?.trim() ? model.trim() : null,
    });
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

export async function stopEngine(): Promise<void> {
  try {
    await invoke("stop_engine");
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

export async function listInputDevices(): Promise<DeviceDto[]> {
  try {
    return await invoke<DeviceDto[]>("list_input_devices");
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

export async function listOutputDevices(): Promise<DeviceDto[]> {
  try {
    return await invoke<DeviceDto[]>("list_output_devices");
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

export async function switchInput(deviceId: string): Promise<void> {
  try {
    await invoke("switch_input", { deviceId });
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

export async function switchOutput(deviceId: string): Promise<void> {
  try {
    await invoke("switch_output", { deviceId });
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

/** Whether Parakeet + Supertone are present under `~/.boris/models`. */
export async function getModelsStatus(): Promise<ModelsStatus> {
  try {
    return await invoke<ModelsStatus>("models_status");
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

/**
 * Download missing models (blocks until finished).
 * Subscribe with {@link onModelsProgress} for per-file updates.
 */
export async function downloadModels(): Promise<ModelsInstallReport> {
  try {
    return await invoke<ModelsInstallReport>("download_models");
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

/** Subscribe to `models-progress` emits during {@link downloadModels}. */
export async function onModelsProgress(
  handler: (progress: DownloadProgress) => void,
): Promise<() => void> {
  let unlisten: UnlistenFn | undefined;
  try {
    unlisten = await listen<DownloadProgress>("models-progress", (e) => {
      handler(e.payload);
    });
  } catch {
    return () => {};
  }
  return () => {
    unlisten?.();
  };
}

/** Load OpenRouter key + model from `~/.boris/settings.json`. */
export async function getSettings(): Promise<AppSettings> {
  try {
    const raw = await invoke<Partial<AppSettings>>("get_settings");
    return {
      openrouter_api_key: raw?.openrouter_api_key ?? "",
      openrouter_model: raw?.openrouter_model ?? "",
    };
  } catch {
    return { ...EMPTY_SETTINGS };
  }
}

/** Persist OpenRouter key + model (never log the key). */
export async function saveSettings(settings: AppSettings): Promise<void> {
  try {
    await invoke("save_app_settings", {
      settings: {
        openrouter_api_key: settings.openrouter_api_key ?? "",
        openrouter_model: settings.openrouter_model ?? "",
      },
    });
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}
