import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { logger } from "@/lib/logger";
import { invokeErrorMessage } from "../errors";
import { COMMANDS, EVENTS } from "../ipc";
import {
  normalizeStatus,
  OFF_STATUS,
  type LivenessStatus,
  type PreflightReport,
  type StatusPicture,
} from "../types";

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

export async function submitInput(id: string, value: string): Promise<void> {
  try {
    await invoke(COMMANDS.submitInput, { id, value });
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("submit_input failed", msg);
    throw new Error(msg);
  }
}

export async function cancelInput(id: string): Promise<void> {
  try {
    await invoke(COMMANDS.cancelInput, { id });
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("cancel_input failed", msg);
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

export async function wakeLivenessStatus(): Promise<LivenessStatus> {
  try {
    return await invoke<LivenessStatus>(COMMANDS.wakeLivenessStatus);
  } catch (e) {
    logger.warn("wake_liveness_status failed", invokeErrorMessage(e));
    return { enrolled: false, takes: 0 };
  }
}

export async function startWakeEnroll(takes = 4): Promise<void> {
  await invoke(COMMANDS.startWakeEnroll, { takes });
}

export async function clearWakeProfile(): Promise<void> {
  await invoke(COMMANDS.clearWakeProfile);
}
