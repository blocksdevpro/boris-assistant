import { useCallback, useEffect, useRef, useState } from "react";
import {
  EMPTY_SETTINGS,
  getSettings,
  saveSettings,
  type AppSettings,
} from "@/bridge";
import { logger } from "@/lib/logger";
import type { SaveState } from "../mainWindowShared";

/** Owns settings loading and the debounced save lifecycle for the main window. */
export function useSettingsController(
  onError: (message: string | null) => void,
) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savedIndicatorTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const flushSave = useCallback(
    async (next: AppSettings) => {
      try {
        await saveSettings(next);
        setSaveState("saved");
        if (savedIndicatorTimer.current) {
          clearTimeout(savedIndicatorTimer.current);
        }
        savedIndicatorTimer.current = setTimeout(() => setSaveState("idle"), 1800);
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        logger.error("saveSettings failed", message);
        onError(message);
        setSaveState("error");
      }
    },
    [onError],
  );

  const patchSettings = useCallback(
    (patch: Partial<AppSettings>) => {
      const base = settingsRef.current ?? { ...EMPTY_SETTINGS };
      const next = { ...base, ...patch };
      setSettings(next);
      setSaveState("saving");
      onError(null);
      settingsRef.current = next;
      if (saveTimer.current) clearTimeout(saveTimer.current);
      if (savedIndicatorTimer.current) clearTimeout(savedIndicatorTimer.current);
      saveTimer.current = setTimeout(() => {
        void flushSave(next);
      }, 320);
    },
    [flushSave, onError],
  );

  useEffect(() => {
    return () => {
      if (saveTimer.current) clearTimeout(saveTimer.current);
      if (savedIndicatorTimer.current) clearTimeout(savedIndicatorTimer.current);
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    void getSettings().then((loaded) => {
      if (cancelled) return;
      setSettings(loaded);
    }).catch(() => {
      if (!cancelled) setSettings({ ...EMPTY_SETTINGS });
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const cancelPendingSave = useCallback(() => {
    if (!saveTimer.current) return;
    clearTimeout(saveTimer.current);
    saveTimer.current = null;
  }, []);

  return { settings, settingsRef, saveState, patchSettings, cancelPendingSave };
}
