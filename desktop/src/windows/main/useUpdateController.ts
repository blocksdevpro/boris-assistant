import { useCallback, useEffect, useRef, useState } from "react";
import type { Update } from "@tauri-apps/plugin-updater";
import {
  appVersion,
  checkForUpdate,
  downloadAndInstallUpdate,
  type AvailableUpdate,
  type CheckResult,
  type UpdateProgress,
} from "@/lib/updater";
import { logger } from "@/lib/logger";
import type { UpdateChannel } from "@/bridge";
import type { UpdateUiState } from "./mainWindowShared";

/** Owns update checks, install progress, and the update banner state. */
export function useUpdateController(channel?: UpdateChannel) {
  const [appVer, setAppVer] = useState<string | null>(null);
  const [updateUi, setUpdateUi] = useState<UpdateUiState>("idle");
  const [availableUpdate, setAvailableUpdate] = useState<AvailableUpdate | null>(null);
  const [pendingUpdate, setPendingUpdate] = useState<Update | null>(null);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress | null>(null);
  const [updateError, setUpdateError] = useState<string | null>(null);
  const [updateBannerDismissed, setUpdateBannerDismissed] = useState(false);

  const installingUpdate = useRef(false);
  const checkInFlight = useRef<Promise<CheckResult> | null>(null);
  const checkInFlightChannel = useRef<UpdateChannel | null>(null);
  const updateChannel = useRef<UpdateChannel>("stable");
  updateChannel.current = channel ?? "stable";

  useEffect(() => {
    void appVersion().then((version) => {
      if (version) setAppVer(version);
    });
  }, []);

  const runUpdateCheck = useCallback(async (options?: { silent?: boolean }) => {
    const silent = options?.silent ?? false;
    if (installingUpdate.current) return;
    if (!silent) {
      setUpdateUi("checking");
      setUpdateError(null);
    }

    const selectedChannel = updateChannel.current;
    let pending = checkInFlight.current;
    if (!pending || checkInFlightChannel.current !== selectedChannel) {
      pending = checkForUpdate(selectedChannel);
      checkInFlight.current = pending;
      checkInFlightChannel.current = selectedChannel;
      void pending.finally(() => {
        if (checkInFlight.current === pending) {
          checkInFlight.current = null;
          checkInFlightChannel.current = null;
        }
      });
    }

    const result = await pending;
    if (installingUpdate.current) return;
    if (updateChannel.current !== selectedChannel) return;

    switch (result.status) {
      case "unavailable":
        if (!silent) setUpdateUi("idle");
        break;
      case "up_to_date":
        setAvailableUpdate(null);
        setPendingUpdate(null);
        setUpdateError(null);
        setAppVer(result.currentVersion);
        setUpdateUi("up_to_date");
        break;
      case "available":
        setAvailableUpdate(result.update);
        setPendingUpdate(result.raw);
        setUpdateError(null);
        setAppVer(result.update.currentVersion);
        setUpdateUi("available");
        setUpdateBannerDismissed(false);
        break;
      case "error":
        setUpdateError(result.message);
        if (!silent) setUpdateUi("error");
        break;
    }
  }, []);

  const didStartupUpdateCheck = useRef(false);
  useEffect(() => {
    if (!channel) return;
    if (!didStartupUpdateCheck.current) {
      didStartupUpdateCheck.current = true;
      const timer = window.setTimeout(() => {
        void runUpdateCheck({ silent: true });
      }, 2500);
      return () => window.clearTimeout(timer);
    }
    setUpdateBannerDismissed(false);
    void runUpdateCheck({ silent: true });
  }, [channel, runUpdateCheck]);

  const installAvailableUpdate = useCallback(async () => {
    if (!pendingUpdate || installingUpdate.current) return;
    installingUpdate.current = true;
    setUpdateUi("downloading");
    setUpdateProgress({ downloaded: 0, contentLength: null });
    setUpdateError(null);
    try {
      await downloadAndInstallUpdate(pendingUpdate, setUpdateProgress);
      // Windows exits during install; relaunch covers other platforms.
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      logger.error("update install failed", message);
      setUpdateError(message);
      setUpdateUi("error");
      installingUpdate.current = false;
    }
  }, [pendingUpdate]);

  return {
    appVer,
    updateUi,
    availableUpdate,
    updateProgress,
    updateError,
    updateBannerDismissed,
    setUpdateBannerDismissed,
    runUpdateCheck,
    installAvailableUpdate,
  };
}
