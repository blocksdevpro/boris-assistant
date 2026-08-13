/**
 * App auto-update helpers (Tauri updater plugin).
 *
 * Checks GitHub Releases (`latest.json` on the selected channel), downloads
 * signed installers, then relaunches. No-ops cleanly outside the Tauri
 * webview (browser fixtures).
 */

import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { Update, type DownloadEvent } from "@tauri-apps/plugin-updater";
import { COMMANDS } from "@/bridge/ipc";
import {
  normalizeUpdateChannel,
  type UpdateChannel,
} from "@/bridge/types";
import { isTauriRuntime } from "@/lib/runtime";
import { logger } from "@/lib/logger";

/** GitHub `/releases/latest` — never includes pre-releases. */
export const STABLE_UPDATE_ENDPOINT =
  "https://github.com/blocksdevpro/boris-assistant/releases/latest/download/latest.json";

/** Long-lived pre-release tag `beta` (overwrite `latest.json` on each beta). */
export const BETA_UPDATE_ENDPOINT =
  "https://github.com/blocksdevpro/boris-assistant/releases/download/beta/latest.json";

export function endpointForChannel(
  channel: string | null | undefined,
): string {
  return normalizeUpdateChannel(channel) === "beta"
    ? BETA_UPDATE_ENDPOINT
    : STABLE_UPDATE_ENDPOINT;
}

/** Shape returned by `check_app_update` — matches the plugin `Update` constructor. */
type PluginUpdateMeta = {
  rid: number;
  currentVersion: string;
  version: string;
  date?: string | null;
  body?: string | null;
  rawJson: Record<string, unknown>;
};

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
 * Poll the GitHub Releases feed for `channel`.
 * Returns `unavailable` when not running under Tauri (dev browser preview).
 */
export async function checkForUpdate(
  channel: string | null | undefined = "stable",
): Promise<CheckResult> {
  if (!isTauriRuntime()) {
    return { status: "unavailable" };
  }

  const selected: UpdateChannel = normalizeUpdateChannel(channel);
  try {
    const currentVersion = (await getVersion().catch(() => "unknown")) ?? "unknown";
    const metadata = await invoke<PluginUpdateMeta | null>(COMMANDS.checkAppUpdate, {
      channel: selected,
    });
    if (!metadata) {
      logger.info("updater: already on latest", { currentVersion, channel: selected });
      return { status: "up_to_date", currentVersion };
    }

    const update = new Update({
      rid: metadata.rid,
      currentVersion: metadata.currentVersion,
      version: metadata.version,
      date: metadata.date ?? undefined,
      body: metadata.body ?? undefined,
      rawJson: metadata.rawJson ?? {},
    });

    const available: AvailableUpdate = {
      version: update.version,
      currentVersion: update.currentVersion,
      body: update.body ?? null,
      date: update.date ?? null,
    };
    logger.info("updater: update available", { ...available, channel: selected });
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
