import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronLeft,
  LoaderCircle,
  Settings as SettingsIcon,
} from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import type { Update } from "@tauri-apps/plugin-updater";
import { TitleBar } from "@/components/TitleBar";
import {
  downloadModels,
  EMPTY_SETTINGS,
  formatContextMeter,
  getModelsStatus,
  getSettings,
  listInputDevices,
  listOutputDevices,
  onModelsProgress,
  saveSettings,
  startEngine,
  stopEngine,
  switchInput,
  switchOutput,
  useStatus,
  type AppSettings,
  type DeviceDto,
  type DownloadProgress,
  type ModelsStatus,
} from "@/bridge";
import { getLogPath, logger } from "@/lib/logger";
import { toneFor } from "@/lib/phaseVisual";
import {
  appVersion,
  checkForUpdate,
  downloadAndInstallUpdate,
  type AvailableUpdate,
  type CheckResult,
  type UpdateProgress,
} from "@/lib/updater";
import { HomeView } from "./HomeView";
import {
  CAPABILITY_OPTIONS,
  type ModelCheckState,
  type SaveState,
  type SettingsCategory,
  type UpdateUiState,
  type View,
} from "./mainWindowShared";
import { SettingsView } from "./settings";
import { TeachVoiceView } from "./TeachVoiceView";

/**
 * Main window — Home (run) + Settings (prefs).
 * Quiet, Apple-like surface. Motion stays on the overlay island.
 */
export function MainWindow() {
  const status = useStatus();
  const reduceMotion = useReducedMotion();
  const tone = useMemo(
    () => toneFor(status.phase, status.engine),
    [status.phase, status.engine],
  );

  const [view, setView] = useState<View>("home");
  const mainScrollRef = useRef<HTMLElement>(null);
  const [settingsCategory, setSettingsCategory] =
    useState<SettingsCategory>("general");
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [inputs, setInputs] = useState<DeviceDto[]>([]);
  const [outputs, setOutputs] = useState<DeviceDto[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [models, setModels] = useState<ModelsStatus | null>(null);
  const [modelCheckState, setModelCheckState] =
    useState<ModelCheckState>("loading");
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [installing, setInstalling] = useState(false);
  const [installProgress, setInstallProgress] =
    useState<DownloadProgress | null>(null);
  const [logPath, setLogPath] = useState("");
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const autoStarted = useRef(false);
  const [appVer, setAppVer] = useState<string | null>(null);
  const [updateUi, setUpdateUi] = useState<UpdateUiState>("idle");
  const [availableUpdate, setAvailableUpdate] =
    useState<AvailableUpdate | null>(null);
  const [pendingUpdate, setPendingUpdate] = useState<Update | null>(null);
  const [updateProgress, setUpdateProgress] = useState<UpdateProgress | null>(
    null,
  );
  const [updateError, setUpdateError] = useState<string | null>(null);
  const [updateBannerDismissed, setUpdateBannerDismissed] = useState(false);

  const engineOn = status.engine === "On" || status.engine === "Starting";
  const engineFault = status.engine === "Fault";
  const modelsReady = modelCheckState === "ready";
  const contextMeter = formatContextMeter(
    status.context_used,
    status.context_limit,
    status.context_estimated,
  );

  useEffect(() => {
    // Settings is scrollable; a new page should never inherit its old offset.
    mainScrollRef.current?.scrollTo({ top: 0 });
  }, [view]);

  const refreshDevices = useCallback(async () => {
    setError(null);
    try {
      const [ins, outs] = await Promise.all([
        listInputDevices(),
        listOutputDevices(),
      ]);
      setInputs(ins);
      setOutputs(outs);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("refreshDevices failed", msg);
      setError(msg);
    }
  }, []);

  const refreshModels = useCallback(async () => {
    setError(null);
    setModelCheckState("loading");
    try {
      const next = await getModelsStatus();
      setModels(next);
      setModelCheckState(
        next.parakeet_ready && next.supertone_ready ? "ready" : "missing",
      );
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("refreshModels failed", msg);
      setError(msg);
      setModelCheckState("error");
    }
  }, []);

  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savedIndicatorTimer = useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  const flushSave = useCallback(async (next: AppSettings) => {
    try {
      await saveSettings(next);
      setSaveState("saved");
      if (savedIndicatorTimer.current)
        clearTimeout(savedIndicatorTimer.current);
      savedIndicatorTimer.current = setTimeout(
        () => setSaveState("idle"),
        1800,
      );
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("saveSettings failed", msg);
      setError(msg);
      setSaveState("error");
    }
  }, []);

  const patchSettings = useCallback(
    (patch: Partial<AppSettings>) => {
      const base = settingsRef.current ?? { ...EMPTY_SETTINGS };
      const next = { ...base, ...patch };
      setSettings(next);
      setSaveState("saving");
      setError(null);
      settingsRef.current = next;
      if (saveTimer.current) clearTimeout(saveTimer.current);
      if (savedIndicatorTimer.current)
        clearTimeout(savedIndicatorTimer.current);
      saveTimer.current = setTimeout(() => {
        void flushSave(next);
      }, 320);
    },
    [flushSave],
  );

  useEffect(() => {
    return () => {
      if (saveTimer.current) clearTimeout(saveTimer.current);
      if (savedIndicatorTimer.current)
        clearTimeout(savedIndicatorTimer.current);
    };
  }, []);

  useEffect(() => {
    void refreshDevices();
    void refreshModels();
    void getLogPath().then((p) => {
      if (p) setLogPath(p);
    });
  }, [refreshDevices, refreshModels]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const s = await getSettings();
        if (cancelled) return;
        setSettings(s);
      } catch {
        if (!cancelled) setSettings({ ...EMPTY_SETTINGS });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const restoredInput = useRef(false);
  const restoredOutput = useRef(false);

  // Resolve packaged version once for Settings / About.
  useEffect(() => {
    void appVersion().then((v) => {
      if (v) setAppVer(v);
    });
  }, []);

  /** True while download+install is in progress (survives re-renders / checks). */
  const installingUpdateRef = useRef(false);
  const checkInFlight = useRef<Promise<CheckResult> | null>(null);
  const checkInFlightChannel = useRef<string | null>(null);

  const runUpdateCheck = useCallback(async (opts?: { silent?: boolean }) => {
    const silent = opts?.silent ?? false;
    if (installingUpdateRef.current) return;
    if (!silent) {
      setUpdateUi("checking");
      setUpdateError(null);
    }
    const channel = settingsRef.current?.update_channel ?? "stable";
    let pending = checkInFlight.current;
    if (!pending || checkInFlightChannel.current !== channel) {
      pending = checkForUpdate(channel);
      checkInFlight.current = pending;
      checkInFlightChannel.current = channel;
      void pending.finally(() => {
        if (checkInFlight.current === pending) {
          checkInFlight.current = null;
          checkInFlightChannel.current = null;
        }
      });
    }
    const result = await pending;
    // Don't clobber an in-flight install if one started while we were checking.
    if (installingUpdateRef.current) return;
    if ((settingsRef.current?.update_channel ?? "stable") !== channel) return;
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

  // Quiet check once settings are known; re-check when the channel changes.
  const didStartupUpdateCheck = useRef(false);
  const updateChannel = settings?.update_channel;
  useEffect(() => {
    if (!updateChannel) return;
    if (!didStartupUpdateCheck.current) {
      didStartupUpdateCheck.current = true;
      const t = window.setTimeout(() => {
        void runUpdateCheck({ silent: true });
      }, 2500);
      return () => window.clearTimeout(t);
    }
    setUpdateBannerDismissed(false);
    void runUpdateCheck({ silent: true });
  }, [updateChannel, runUpdateCheck]);

  const onInstallUpdate = useCallback(async () => {
    if (!pendingUpdate || installingUpdateRef.current) return;
    installingUpdateRef.current = true;
    setUpdateUi("downloading");
    setUpdateProgress({ downloaded: 0, contentLength: null });
    setUpdateError(null);
    try {
      await downloadAndInstallUpdate(pendingUpdate, setUpdateProgress);
      // Windows exits during install; relaunch covers other platforms.
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("update install failed", msg);
      setUpdateError(msg);
      setUpdateUi("error");
      installingUpdateRef.current = false;
    }
  }, [pendingUpdate]);

  // Restore each preferred device once. Unrelated settings edits must not
  // interrupt the active audio stream by switching the same device again.
  useEffect(() => {
    if (!settings) return;
    if (inputs.length === 0 && outputs.length === 0) return;

    const pick = (list: DeviceDto[], preferred: string) => {
      if (preferred && list.some((d) => d.id === preferred)) return preferred;
      return list.find((d) => d.is_default)?.id ?? list[0]?.id ?? "";
    };

    const inputId = pick(inputs, settings.input_device);
    const outputId = pick(outputs, settings.output_device);

    if (!restoredInput.current && inputs.length > 0) {
      restoredInput.current = true;
      if (settings.input_device && inputId === settings.input_device)
        void switchInput(inputId).catch((e) => {
          // switchInput() already logs the underlying failure; this is just
          // context that it happened during preferred-device restore, not a
          // user-initiated switch.
          logger.error("restore preferred input device failed", {
            deviceId: inputId,
            error: e instanceof Error ? e.message : String(e),
          });
        });
    }
    if (!restoredOutput.current && outputs.length > 0) {
      restoredOutput.current = true;
      if (settings.output_device && outputId === settings.output_device)
        void switchOutput(outputId).catch((e) => {
          logger.error("restore preferred output device failed", {
            deviceId: outputId,
            error: e instanceof Error ? e.message : String(e),
          });
        });
    }
  }, [settings?.input_device, settings?.output_device, inputs, outputs]);

  const onStart = async () => {
    const s = settingsRef.current ?? settings;
    if (!s) return;
    if (!s.openrouter_api_key.trim()) {
      setError("Add your OpenRouter API key before starting Boris.");
      setSettingsCategory("connections");
      setView("settings");
      return;
    }
    if (modelCheckState !== "ready") {
      setError(
        modelCheckState === "error"
          ? "Speech models could not be checked. Retry the check in Speech settings."
          : "Install the speech models before starting Boris.",
      );
      setSettingsCategory("speech");
      setView("settings");
      return;
    }
    if (saveTimer.current) {
      clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    setBusy(true);
    setError(null);
    logger.info("UI onStart", {
      hasOpenRouterKey: Boolean(s.openrouter_api_key.trim()),
      hasExaKey: Boolean(s.exa_api_key.trim()),
      model: s.openrouter_model || null,
      modelsReady,
    });
    try {
      await saveSettings(s);
      if (s.input_device) await switchInput(s.input_device);
      if (s.output_device) await switchOutput(s.output_device);
      await startEngine({
        apiKey: s.openrouter_api_key,
        model: s.openrouter_model || undefined,
        fastModel: s.openrouter_fast_model || undefined,
        modelProvider: s.openrouter_model_provider || undefined,
        fastProvider: s.openrouter_fast_provider || undefined,
        pinProvider: s.openrouter_pin_provider,
      });
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("UI onStart failed", msg);
      setError(msg);
    } finally {
      setBusy(false);
    }
  };

  // Start engine on launch when enabled (once).
  useEffect(() => {
    if (autoStarted.current || !settings?.start_engine_on_launch) return;
    if (!modelsReady || engineOn || busy) return;
    if (!settings.openrouter_api_key.trim()) return;
    autoStarted.current = true;
    void onStart();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentional one-shot
  }, [settings, modelsReady, engineOn, busy]);

  const onStop = async () => {
    setBusy(true);
    setError(null);
    try {
      await stopEngine();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const onInputChange = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      await switchInput(id);
      await patchSettings({ input_device: id });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const onOutputChange = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      await switchOutput(id);
      await patchSettings({ output_device: id });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const onInstallModels = async () => {
    setInstalling(true);
    setError(null);
    setInstallProgress(null);
    let unsub = () => {};
    try {
      unsub = await onModelsProgress((p) => setInstallProgress(p));
      const report = await downloadModels();
      await refreshModels();
      if (!report.ok) {
        setError(
          report.errors[0] ??
            `Install incomplete (failed ${report.files_failed} file(s))`,
        );
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      await refreshModels();
    } finally {
      unsub();
      setInstalling(false);
    }
  };

  const selectedInput =
    settings?.input_device && inputs.some((d) => d.id === settings.input_device)
      ? settings.input_device
      : (inputs.find((d) => d.is_default)?.id ?? inputs[0]?.id ?? "");

  const selectedOutput =
    settings?.output_device &&
    outputs.some((d) => d.id === settings.output_device)
      ? settings.output_device
      : (outputs.find((d) => d.is_default)?.id ?? outputs[0]?.id ?? "");

  const capability =
    CAPABILITY_OPTIONS.find((p) => p.id === settings?.capability_preset) ??
    CAPABILITY_OPTIONS[0]!;

  return (
    <div className="main-console flex h-screen flex-col overflow-hidden text-white antialiased">
      <TitleBar
        title={
          view === "settings"
            ? "Settings"
            : view === "teach"
              ? "Your voice"
              : "Boris"
        }
        leading={
          view === "settings" ? (
            <button
              type="button"
              onClick={() => setView("home")}
              className="main-back-button inline-flex h-8 items-center gap-0.5 rounded-[9px] px-2 text-[12px] font-medium text-white/62 transition-[color,background-color,transform] duration-150 hover:bg-white/[0.07] hover:text-white active:scale-[0.97] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/20"
            >
              <ChevronLeft className="size-4" strokeWidth={2} />
              Home
            </button>
          ) : view === "teach" ? (
            <button
              type="button"
              onClick={() => {
                setSettingsCategory("speech");
                setView("settings");
              }}
              className="main-back-button inline-flex h-8 items-center gap-0.5 rounded-[9px] px-2 text-[12px] font-medium text-white/62 transition-[color,background-color,transform] duration-150 hover:bg-white/[0.07] hover:text-white active:scale-[0.97] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/20"
            >
              <ChevronLeft className="size-4" strokeWidth={2} />
              Speech
            </button>
          ) : undefined
        }
        trailing={
          view === "home" ? (
            <button
              type="button"
              aria-label="Settings"
              onClick={() => setView("settings")}
              className="main-settings-button inline-flex size-8 items-center justify-center rounded-[9px] text-white/42 transition-[color,background-color,transform] duration-150 hover:bg-white/[0.07] hover:text-white active:scale-[0.94] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/20"
            >
              <SettingsIcon className="size-4" strokeWidth={1.75} />
            </button>
          ) : undefined
        }
      />

      <main ref={mainScrollRef} className="min-h-0 flex-1 overflow-y-auto">
        <AnimatePresence
          mode="wait"
          initial={false}
          onExitComplete={() => mainScrollRef.current?.scrollTo({ top: 0 })}
        >
          <motion.div
            key={view}
            className="min-h-full"
            initial={
              reduceMotion ? false : { opacity: 0, y: 10, scale: 0.995 }
            }
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={
              reduceMotion
                ? { opacity: 1 }
                : { opacity: 0, y: -6, scale: 0.997 }
            }
            transition={
              reduceMotion
                ? { duration: 0 }
                : { duration: 0.3, ease: [0.22, 1, 0.36, 1] }
            }
          >
            {view === "home" ? (
              <HomeView
                status={status}
                tone={tone}
                contextMeter={contextMeter}
                engineOn={engineOn}
                engineFault={engineFault}
                modelsReady={modelsReady}
                modelCheckState={modelCheckState}
                busy={busy}
                error={error}
                models={models}
                installing={installing}
                installProgress={installProgress}
                availableUpdate={
                  updateUi === "available" || updateUi === "downloading"
                    ? availableUpdate
                    : null
                }
                updateUi={updateUi}
                updateProgress={updateProgress}
                updateBannerDismissed={updateBannerDismissed}
                onStart={() => void onStart()}
                onStop={() => void onStop()}
                onInstall={() => void onInstallModels()}
                onInstallUpdate={() => void onInstallUpdate()}
                onDismissUpdate={() => setUpdateBannerDismissed(true)}
                onOpenSettings={(category = "general") => {
                  setSettingsCategory(category);
                  setView("settings");
                }}
              />
            ) : view === "teach" ? (
              <TeachVoiceView
                status={status}
                engineOn={engineOn}
                busy={busy}
                onStartEngine={() => void onStart()}
                onDone={() => {
                  setSettingsCategory("speech");
                  setView("settings");
                }}
              />
            ) : settings ? (
              <SettingsView
                settings={settings}
                engineOn={engineOn}
                busy={busy}
                inputs={inputs}
                outputs={outputs}
                selectedInput={selectedInput}
                selectedOutput={selectedOutput}
                modelsReady={modelsReady}
                modelCheckState={modelCheckState}
                installing={installing}
                installProgress={installProgress}
                advancedOpen={advancedOpen}
                logPath={logPath}
                capability={capability}
                error={error}
                saveState={saveState}
                category={settingsCategory}
                appVer={appVer}
                updateUi={updateUi}
                availableUpdate={availableUpdate}
                updateProgress={updateProgress}
                updateError={updateError}
                onCategoryChange={setSettingsCategory}
                onPatch={(p) => patchSettings(p)}
                onInputChange={(id) => void onInputChange(id)}
                onOutputChange={(id) => void onOutputChange(id)}
                onRefreshDevices={() => void refreshDevices()}
                onRefreshModels={() => void refreshModels()}
                onInstall={() => void onInstallModels()}
                onTeachVoice={() => setView("teach")}
                onToggleAdvanced={() => setAdvancedOpen((v) => !v)}
                onCheckUpdate={() => void runUpdateCheck()}
                onInstallUpdate={() => void onInstallUpdate()}
              />
            ) : (
              <div
                className="settings-loading-state flex min-h-[26rem] flex-col items-center justify-center px-6 text-center"
                role="status"
                aria-live="polite"
              >
                <div className="flex size-11 items-center justify-center rounded-2xl border border-white/[0.06] bg-white/[0.04] shadow-[0_10px_30px_rgba(0,0,0,0.16)]">
                  <LoaderCircle className="size-4 animate-spin text-white/55" />
                </div>
                <p className="mt-4 text-[14px] font-medium text-white/72">
                  Loading settings
                </p>
                <p className="mt-1 text-[12px] text-white/32">
                  Reading your preferences from this computer…
                </p>
              </div>
            )}
          </motion.div>
        </AnimatePresence>
      </main>
    </div>
  );
}
