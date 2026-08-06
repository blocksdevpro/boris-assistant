/**
 * Typed bridge for engine status + control.
 * Rust owns the source of truth; this is the window/overlay surface.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { logger } from "@/lib/logger";
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
  } catch (e) {
    logger.warn("get_status failed", invokeErrorMessage(e));
    return { ...OFF_STATUS };
  }
}

/** Check whether required STT/TTS models exist under `~/.boris`. */
export async function preflightCheck(): Promise<PreflightReport> {
  try {
    return await invoke<PreflightReport>("preflight_check");
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("preflight_check failed", msg);
    throw new Error(msg);
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
  } catch (e) {
    logger.warn("onStatus listen failed", invokeErrorMessage(e));
    return () => {};
  }
  return () => {
    unlisten?.();
  };
}

export type StartEngineOptions = {
  apiKey?: string;
  /** Strong / primary OpenRouter model id. */
  model?: string;
  /** Fast model for simple turns. */
  fastModel?: string;
  /**
   * OpenRouter model-provider slug(s) for the strong model
   * (e.g. `coreweave` or `coreweave,baseten`) — not the API brand.
   */
  modelProvider?: string;
  /** Model-provider for the fast model. */
  fastProvider?: string;
  /** Hard-pin to preferred providers (no fallbacks). */
  pinProvider?: boolean;
};

export async function startEngine(
  apiKeyOrOpts: string | StartEngineOptions = "",
  model?: string,
): Promise<void> {
  const opts: StartEngineOptions =
    typeof apiKeyOrOpts === "string"
      ? { apiKey: apiKeyOrOpts, model }
      : apiKeyOrOpts;

  const apiKey = opts.apiKey ?? "";
  const modelId = opts.model?.trim() || null;
  const fastModel = opts.fastModel?.trim() || null;
  const modelProvider = opts.modelProvider?.trim() || null;
  const fastProvider = opts.fastProvider?.trim() || null;
  const pinProvider = opts.pinProvider ?? null;

  logger.info("startEngine", {
    hasKey: Boolean(apiKey.trim()),
    model: modelId,
    fastModel,
    modelProvider,
    fastProvider,
    pinProvider,
  });
  try {
    await invoke("start_engine", {
      apiKey,
      model: modelId,
      fastModel,
      modelProvider,
      fastProvider,
      pinProvider,
    });
    logger.info("startEngine ok");
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("startEngine failed", msg);
    throw new Error(msg);
  }
}

export async function stopEngine(): Promise<void> {
  logger.info("stopEngine");
  try {
    await invoke("stop_engine");
    logger.info("stopEngine ok");
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("stopEngine failed", msg);
    throw new Error(msg);
  }
}

export async function listInputDevices(): Promise<DeviceDto[]> {
  try {
    return await invoke<DeviceDto[]>("list_input_devices");
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("list_input_devices failed", msg);
    throw new Error(msg);
  }
}

export async function listOutputDevices(): Promise<DeviceDto[]> {
  try {
    return await invoke<DeviceDto[]>("list_output_devices");
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("list_output_devices failed", msg);
    throw new Error(msg);
  }
}

export async function switchInput(deviceId: string): Promise<void> {
  logger.info("switchInput", { deviceId });
  try {
    await invoke("switch_input", { deviceId });
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("switchInput failed", msg);
    throw new Error(msg);
  }
}

export async function switchOutput(deviceId: string): Promise<void> {
  logger.info("switchOutput", { deviceId });
  try {
    await invoke("switch_output", { deviceId });
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("switchOutput failed", msg);
    throw new Error(msg);
  }
}

/** Whether Parakeet + Supertone are present under `~/.boris/models`. */
export async function getModelsStatus(): Promise<ModelsStatus> {
  try {
    return await invoke<ModelsStatus>("models_status");
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("models_status failed", msg);
    throw new Error(msg);
  }
}

/**
 * Download missing models (blocks until finished).
 * Subscribe with {@link onModelsProgress} for per-file updates.
 */
export async function downloadModels(): Promise<ModelsInstallReport> {
  logger.info("downloadModels start");
  try {
    const report = await invoke<ModelsInstallReport>("download_models");
    logger.info("downloadModels done", {
      ok: report.ok,
      files_downloaded: report.files_downloaded,
      files_failed: report.files_failed,
    });
    return report;
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("downloadModels failed", msg);
    throw new Error(msg);
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
  } catch (e) {
    logger.warn("onModelsProgress listen failed", invokeErrorMessage(e));
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
  } catch (e) {
    logger.warn("get_settings failed", invokeErrorMessage(e));
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
    logger.info("saveSettings ok", {
      hasKey: Boolean(settings.openrouter_api_key?.trim()),
      model: settings.openrouter_model || null,
    });
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("saveSettings failed", msg);
    throw new Error(msg);
  }
}
