import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { logger } from "@/lib/logger";
import { invokeErrorMessage } from "../errors";
import { COMMANDS, EVENTS } from "../ipc";
import type { DownloadProgress, ModelsInstallReport, ModelsStatus } from "../types";
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

/** Load API keys + models/providers from `~/.boris/config.toml` + `auth.json`. */
