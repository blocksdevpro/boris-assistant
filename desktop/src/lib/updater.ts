/**
 * App auto-update helpers (Tauri updater plugin).
 *
 * Checks GitHub Releases (`latest.json`), downloads signed installers, then
 * relaunches. No-ops cleanly outside the Tauri webview (browser fixtures).
 */

import { check, type Update, type DownloadEvent } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import { isTauriRuntime } from "@/lib/runtime";
import { logger } from "@/lib/logger";

export type UpdateProgress = {
  /** Bytes received so far for the current download. */
  downloaded: number;
  /** Total size when known. */
  contentLength: number | null;
};

export type AvailableUpdate = {
  version: string;
  currentVersion: string;
  body: string | null;
  date: string | null;
};

export type CheckResult =
  | { status: "unavailable" }
  | { status: "up_to_date"; currentVersion: string }
  | { status: "available"; update: AvailableUpdate; raw: Update }
  | { status: "error"; message: string };

function invokeErrorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) {
    const m = (e as { message?: unknown }).message;
    if (typeof m === "string" && m.trim()) return m;
  }
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

/** Current packaged app version, or `null` outside Tauri. */
export async function appVersion(): Promise<string | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await getVersion();
  } catch (e) {
    logger.warn("getVersion failed", invokeErrorMessage(e));
    return null;
  }
}

/**
 * Poll the configured updater endpoint.
 * Returns `unavailable` when not running under Tauri (dev browser preview).
 */
export async function checkForUpdate(): Promise<CheckResult> {
  if (!isTauriRuntime()) {
    return { status: "unavailable" };
  }

  try {
    const currentVersion = (await getVersion().catch(() => "unknown")) ?? "unknown";
    const update = await check();
    if (!update) {
      logger.info("updater: already on latest", { currentVersion });
      return { status: "up_to_date", currentVersion };
    }

    const available: AvailableUpdate = {
      version: update.version,
      currentVersion: update.currentVersion,
      body: update.body ?? null,
      date: update.date ?? null,
    };
    logger.info("updater: update available", available);
    return { status: "available", update: available, raw: update };
  } catch (e) {
    const message = invokeErrorMessage(e);
    logger.warn("updater: check failed", message);
    return { status: "error", message };
  }
}

/**
 * Download + install a pending update, then relaunch.
 * On Windows the process exits during install (installer limitation).
 */
export async function downloadAndInstallUpdate(
  update: Update,
  onProgress?: (progress: UpdateProgress) => void,
): Promise<void> {
  let downloaded = 0;
  let contentLength: number | null = null;

  await update.downloadAndInstall((event: DownloadEvent) => {
    switch (event.event) {
      case "Started":
        contentLength = event.data.contentLength ?? null;
        downloaded = 0;
        onProgress?.({ downloaded, contentLength });
        break;
      case "Progress":
        downloaded += event.data.chunkLength;
        onProgress?.({ downloaded, contentLength });
        break;
      case "Finished":
        onProgress?.({ downloaded, contentLength });
        break;
    }
  });

  logger.info("updater: install finished, relaunching");
  // Windows may already have exited the process during install.
  try {
    await relaunch();
  } catch (e) {
    logger.warn("updater: relaunch failed (may already be exiting)", invokeErrorMessage(e));
  }
}
