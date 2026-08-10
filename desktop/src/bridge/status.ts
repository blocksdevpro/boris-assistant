/**
 * Typed bridge for engine status + control.
 *
 * # Host vs pipeline
 *
 * - **Host** (`boris-desktop`): owns these invoke/event names, mirrors status,
 *   starts/stops the engine process, lists devices, loads settings.
 * - **Pipeline** (`boris_pipeline`): owns voice policy, model install, real
 *   `StatusPicture` production, and `~/.boris` persistence internals.
 *
 * The UI never imports pipeline crates — only this module + [`./types`].
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { logger } from "@/lib/logger";
import { invokeErrorMessage } from "./errors";
import { COMMANDS, EVENTS } from "./ipc";
import {
  EMPTY_SETTINGS,
  normalizeSettings,
  normalizeStatus,
  OFF_STATUS,
  settingsToWire,
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
    const raw = await invoke<StatusPicture>(COMMANDS.getStatus);
    return normalizeStatus(raw);
  } catch (e) {
    logger.warn("get_status failed", invokeErrorMessage(e));
    return { ...OFF_STATUS };
  }
}

/** Check whether required STT/TTS models exist under `~/.boris`. */
export async function preflightCheck(): Promise<PreflightReport> {
  try {
    return await invoke<PreflightReport>(COMMANDS.preflightCheck);
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
    unlisten = await listen<StatusPicture>(EVENTS.status, (e) => {
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
    // CamelCase keys → Tauri renames to snake_case for Rust args.
    await invoke(COMMANDS.startEngine, {
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
    await invoke(COMMANDS.stopEngine);
    logger.info("stopEngine ok");
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("stopEngine failed", msg);
    throw new Error(msg);
  }
}

export async function listInputDevices(): Promise<DeviceDto[]> {
  try {
    return await invoke<DeviceDto[]>(COMMANDS.listInputDevices);
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("list_input_devices failed", msg);
    throw new Error(msg);
  }
}

export async function listOutputDevices(): Promise<DeviceDto[]> {
  try {
    return await invoke<DeviceDto[]>(COMMANDS.listOutputDevices);
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("list_output_devices failed", msg);
    throw new Error(msg);
  }
}

export async function switchInput(deviceId: string): Promise<void> {
  logger.info("switchInput", { deviceId });
  try {
    await invoke(COMMANDS.switchInput, { deviceId });
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("switchInput failed", msg);
    throw new Error(msg);
  }
}

export async function switchOutput(deviceId: string): Promise<void> {
  logger.info("switchOutput", { deviceId });
  try {
    await invoke(COMMANDS.switchOutput, { deviceId });
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("switchOutput failed", msg);
    throw new Error(msg);
  }
}

/** Whether Parakeet + Supertone are present under `~/.boris/models`. */
export async function getModelsStatus(): Promise<ModelsStatus> {
  try {
    return await invoke<ModelsStatus>(COMMANDS.modelsStatus);
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
    const report = await invoke<ModelsInstallReport>(COMMANDS.downloadModels);
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
    unlisten = await listen<DownloadProgress>(EVENTS.modelsProgress, (e) => {
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

/** Load OpenRouter key + models/providers from `~/.boris/config.toml` + `auth.json`. */
export async function getSettings(): Promise<AppSettings> {
  try {
    const raw = await invoke<Partial<AppSettings>>(COMMANDS.getSettings);
    return normalizeSettings(raw);
  } catch (e) {
    logger.warn("get_settings failed", invokeErrorMessage(e));
    return { ...EMPTY_SETTINGS };
  }
}

/** Persist OpenRouter key + models/providers (never log the key). */
export async function saveSettings(settings: AppSettings): Promise<void> {
  const wire = settingsToWire(settings);
  try {
    await invoke(COMMANDS.saveAppSettings, { settings: wire });
    logger.info("saveSettings ok", {
      hasKey: Boolean(wire.openrouter_api_key?.trim()),
      model: wire.openrouter_model || null,
      fastModel: wire.openrouter_fast_model || null,
      modelProvider: wire.openrouter_model_provider || null,
      fastProvider: wire.openrouter_fast_provider || null,
      pinProvider: wire.openrouter_pin_provider ?? false,
    });
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("saveSettings failed", msg);
    throw new Error(msg);
  }
}
