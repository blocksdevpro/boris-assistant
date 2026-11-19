import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronLeft,
  Activity,
  LoaderCircle,
  Settings as SettingsIcon,
} from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { openUrl } from "@tauri-apps/plugin-opener";
import { TitleBar } from "@/components/TitleBar";
import {
  checkApiKey,
  downloadModels,
  formatContextMeter,
  getModelsStatus,
  listInputDevices,
  listOutputDevices,
  onModelsProgress,
  runSelfCheck,
  saveSettings,
  startEngine,
  stopEngine,
  switchInput,
  switchOutput,
  useStatus,
  type ApiKeyStatus,
  type AppSettings,
  type DeviceDto,
  type DownloadProgress,
  type ModelsStatus,
  type SelfCheckReport,
} from "@/bridge";
import { getLogPath, logger } from "@/lib/logger";
import { toneFor } from "@/lib/phaseVisual";
import { HomeView } from "./HomeView";
import { DebugView } from "./DebugView";
import {
  SYSTEM_DEFAULT_DEVICE,
  followsDefaultDevice,
  type ModelCheckState,
  type SettingsCategory,
  type View,
} from "./mainWindowShared";
import { SettingsView } from "./settings";
import { TeachVoiceView } from "./TeachVoiceView";
import { useSettingsController } from "./settings/useSettingsController";
import { useUpdateController } from "./useUpdateController";
import { SetupCheckToast } from "./SetupCheckToast";
import { OnboardingFlow } from "./onboarding/OnboardingFlow";

/** Set once a setup check fully passes on this PC; later launches skip the auto-run. */
const SETUP_PASSED_STORAGE_KEY = "boris.setupCheck.passed";
/** The engine answers a self-check within a few seconds unless it is mid-turn. */
const SETUP_CHECK_START_TIMEOUT_MS = 20_000;

function setupCheckPassedBefore(): boolean {
  try {
    return window.localStorage.getItem(SETUP_PASSED_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

function rememberSetupCheckPassed() {
  try {
    window.localStorage.setItem(SETUP_PASSED_STORAGE_KEY, "1");
  } catch {
    // Storage unavailable: the check simply runs again next launch.
  }
}

/** Settle time before auto-start, so a key being typed never starts the engine. */
const AUTO_START_SETTLE_MS = 800;
/** Settle time before restarting a running engine for new key / model settings. */
const LLM_RESTART_SETTLE_MS = 1500;

/**
 * Settings baked into the engine at spawn (LLM, tools, memory, model
 * residency); a change restarts the engine between turns.
 */
function engineFingerprint(s: AppSettings): string {
  return JSON.stringify([
    s.openrouter_api_key.trim(),
    s.openrouter_model,
    s.openrouter_fast_model,
    s.openrouter_model_provider,
    s.openrouter_fast_provider,
    s.openrouter_pin_provider,
    s.capability_preset,
    s.long_term_memory,
    s.trusted_auto_moderate,
    s.model_residency,
    // Read once at engine start, so a change needs a restart to apply.
    s.voice_barge_in,
    s.ignore_speaker_playback,
    s.wake_chime,
  ]);
}

/** A model repair started from the setup check. */
type RepairState = {
  /** The failed report that asked for the repair (engine status resets). */
  report: SelfCheckReport | null;
  progress: DownloadProgress | null;
};

/** One line of repair progress for the setup toast. */
function repairLabel(progress: DownloadProgress | null): string {
  if (!progress) return "Stopping Boris…";
  const file = progress.file_name;
  if (progress.status === "starting" && progress.message === "checking") {
    return `Checking ${file}…`;
  }
  if (progress.status === "downloading" && progress.total_bytes) {
    const pct = Math.min(100, Math.round((progress.bytes_downloaded / progress.total_bytes) * 100));
    return `Downloading ${file} · ${pct}%`;
  }
  if (progress.status === "starting" || progress.status === "downloading") {
    return `Downloading ${file}…`;
  }
  return "Checking files…";
}

function openExternal(url: string) {
  void openUrl(url).catch(() => window.open(url, "_blank", "noopener,noreferrer"));
}

/** Files still to fetch; "VERSION:" entries are notes, not files. */
function missingFileCount(models: ModelsStatus | null): number {
  return models?.missing.filter((m) => !m.startsWith("VERSION:")).length ?? 0;
}

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
  const [inputs, setInputs] = useState<DeviceDto[]>([]);
  const [outputs, setOutputs] = useState<DeviceDto[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const {
    settings,
    settingsRef,
    saveState,
    patchSettings,
    cancelPendingSave,
  } = useSettingsController(setError);
  const {
    appVer,
    updateUi,
    availableUpdate,
    updateProgress,
    updateError,
    updateBannerDismissed,
    setUpdateBannerDismissed,
    runUpdateCheck,
    installAvailableUpdate,
  } = useUpdateController(settings?.update_channel);
  const [models, setModels] = useState<ModelsStatus | null>(null);
  const [modelCheckState, setModelCheckState] =
    useState<ModelCheckState>("loading");
  const [installing, setInstalling] = useState(false);
  const [installProgress, setInstallProgress] =
    useState<DownloadProgress | null>(null);
  const [installFilesDone, setInstallFilesDone] = useState(0);
  const [installFilesTotal, setInstallFilesTotal] = useState(0);
  const [installError, setInstallError] = useState<string | null>(null);
  const [logPath, setLogPath] = useState("");
  /** null until settings load; then whether the welcome flow is showing. */
  const [onboardingOpen, setOnboardingOpen] = useState<boolean | null>(null);
  const autoStarted = useRef(false);
  /** Engine settings the live engine was started with (null = not started here). */
  const startedLlm = useRef<string | null>(null);
  const [setupCheckOpen, setSetupCheckOpen] = useState(false);
  /** Request sent; hide the previous report until the engine starts the run. */
  const [awaitingSelfCheck, setAwaitingSelfCheck] = useState(false);
  /** Requested while Boris was mid-turn; sent as soon as he is idle again. */
  const [queuedSelfCheck, setQueuedSelfCheck] = useState(false);
  const [keyStatus, setKeyStatus] = useState<ApiKeyStatus | null>(null);
  /** Only the newest key check may land (a slow older one must not win). */
  const keyCheckSeq = useRef(0);
  const [repair, setRepair] = useState<RepairState | null>(null);
  /** After a model repair restarts Boris, run the check again to prove it. */
  const recheckAfterRestart = useRef(false);
  const setupAutoRun = useRef(false);

  const engineOn = status.engine === "On" || status.engine === "Starting";
  /** The engine only takes a self-check between turns. */
  const engineIdle =
    status.engine === "On" && (status.phase === "Armed" || status.phase === "Quiet");
  const engineIdleRef = useRef(engineIdle);
  engineIdleRef.current = engineIdle;
  const engineFault = status.engine === "Fault";
  const modelsReady = modelCheckState === "ready";
  const contextMeter = formatContextMeter(
    status.context_used,
    status.context_limit,
    status.context_estimated,
  );

  // First launch on this PC (nothing set up yet): show the welcome flow once.
  useEffect(() => {
    if (!settings || onboardingOpen !== null) return;
    const firstRun =
      !settings.onboarding_complete && !settings.openrouter_api_key.trim();
    logger.info("onboarding", { show: firstRun });
    setOnboardingOpen(firstRun);
  }, [settings, onboardingOpen]);

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

  /** Re-read model readiness; resolves to whether every model is ready. */
  const refreshModels = useCallback(async (): Promise<boolean> => {
    setError(null);
    setModelCheckState("loading");
    try {
      const next = await getModelsStatus();
      const ready = next.parakeet_ready && next.supertone_ready;
      setModels(next);
      setModelCheckState(ready ? "ready" : "missing");
      return ready;
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("refreshModels failed", msg);
      setError(msg);
      setModelCheckState("error");
      return false;
    }
  }, []);

  useEffect(() => {
    void refreshDevices();
    void refreshModels();
    void getLogPath().then((p) => {
      if (p) setLogPath(p);
    });
  }, [refreshDevices, refreshModels]);

  // Headsets get plugged in while the window is in the background; show them
  // in the pickers the next time the window comes forward.
  useEffect(() => {
    const onFocus = () => void refreshDevices();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refreshDevices]);

  const restoredInput = useRef(false);
  const restoredOutput = useRef(false);

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
          ? "Speech models could not be checked. Retry the check in Voice & Audio settings."
          : "Install the speech models before starting Boris.",
      );
      setSettingsCategory("speech");
      setView("settings");
      return;
    }
    cancelPendingSave();
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
      startedLlm.current = engineFingerprint(s);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("UI onStart failed", msg);
      setError(msg);
    } finally {
      setBusy(false);
    }
  };

  // Start listening by itself once setup is complete: on launch, or the moment
  // the models finish downloading / the key is pasted on a fresh install.
  // Waits for settings to settle so a half-typed key never starts the engine.
  useEffect(() => {
    if (autoStarted.current || !settings?.start_engine_on_launch) return;
    if (onboardingOpen !== false) return;
    if (!modelsReady || engineOn || busy) return;
    if (!settings.openrouter_api_key.trim()) return;
    const timer = setTimeout(() => {
      autoStarted.current = true;
      logger.info("auto-start: setup complete");
      void onStart();
    }, AUTO_START_SETTLE_MS);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentional one-shot
  }, [settings, modelsReady, engineOn, busy, onboardingOpen]);

  // The engine bakes key + models in at start. Restart it when they change
  // (e.g. a corrected key), but only between turns.
  useEffect(() => {
    if (!settings || !engineOn || busy || startedLlm.current == null) return;
    if (onboardingOpen) return;
    if (status.phase !== "Armed" && status.phase !== "Quiet") return;
    if (!settings.openrouter_api_key.trim()) return;
    if (engineFingerprint(settings) === startedLlm.current) return;
    const timer = setTimeout(() => {
      logger.info("engine settings changed — restarting engine");
      void onStart();
    }, LLM_RESTART_SETTLE_MS);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- onStart reads refs
  }, [settings, engineOn, busy, status.phase, onboardingOpen]);

  const recheckKey = useCallback(() => {
    const seq = ++keyCheckSeq.current;
    setKeyStatus(null);
    void checkApiKey(settingsRef.current?.openrouter_api_key ?? "").then((result) => {
      if (seq === keyCheckSeq.current) setKeyStatus(result);
    });
  }, [settingsRef]);

  const sendSelfCheck = useCallback(async () => {
    setAwaitingSelfCheck(true);
    try {
      await runSelfCheck();
    } catch (e) {
      setAwaitingSelfCheck(false);
      setSetupCheckOpen(false);
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const startSetupCheck = useCallback(() => {
    setSetupCheckOpen(true);
    recheckKey();
    if (engineIdleRef.current) {
      void sendSelfCheck();
    } else {
      // Mid-turn the engine would drop it: wait and say so instead.
      logger.info("setup check: queued until Boris is idle");
      setQueuedSelfCheck(true);
    }
  }, [recheckKey, sendSelfCheck]);

  // First time Boris is listening on this PC, prove the whole chain works.
  useEffect(() => {
    if (setupAutoRun.current || setupCheckPassedBefore()) return;
    if (status.engine !== "On" || status.phase !== "Armed") return;
    setupAutoRun.current = true;
    logger.info("setup check: first-run auto start");
    startSetupCheck();
  }, [status.engine, status.phase, startSetupCheck]);

  // Queued request: send it the moment the turn ends; drop it if Boris stops.
  useEffect(() => {
    if (!queuedSelfCheck) return;
    if (engineIdle) {
      setQueuedSelfCheck(false);
      void sendSelfCheck();
    } else if (status.engine === "Off" || status.engine === "Fault") {
      setQueuedSelfCheck(false);
      setSetupCheckOpen(false);
    }
  }, [queuedSelfCheck, engineIdle, status.engine, sendSelfCheck]);

  // After a model repair restarted Boris, run the check again on its own.
  useEffect(() => {
    if (!recheckAfterRestart.current || !engineIdle) return;
    recheckAfterRestart.current = false;
    startSetupCheck();
  }, [engineIdle, startSetupCheck]);

  // The engine picked up the request once it publishes a running report.
  useEffect(() => {
    if (awaitingSelfCheck && status.self_check?.running) setAwaitingSelfCheck(false);
  }, [awaitingSelfCheck, status.self_check?.running]);

  // Sent but never started: a turn began in between. Wait for the next idle
  // moment rather than failing; only a stuck idle engine is an error.
  useEffect(() => {
    if (!awaitingSelfCheck) return;
    const timer = setTimeout(() => {
      setAwaitingSelfCheck(false);
      if (status.engine === "On" && !engineIdleRef.current) {
        setQueuedSelfCheck(true);
        return;
      }
      setSetupCheckOpen(false);
      setError("The setup check didn't start. Try again in a moment.");
    }, SETUP_CHECK_START_TIMEOUT_MS);
    return () => clearTimeout(timer);
  }, [awaitingSelfCheck, status.engine]);

  // The engine's status resets when a repair stops it: show the snapshot.
  const setupReport = repair
    ? repair.report
    : awaitingSelfCheck || queuedSelfCheck
      ? null
      : (status.self_check ?? null);
  const dismissSetupCheck = useCallback(() => setSetupCheckOpen(false), []);
  const openSettingsFromToast = useCallback((category: SettingsCategory) => {
    setSettingsCategory(category);
    setView("settings");
  }, []);
  useEffect(() => {
    if (setupReport && !setupReport.running && setupReport.ok && keyStatus?.state === "ok") {
      rememberSetupCheckPassed();
    }
  }, [setupReport, keyStatus]);

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
    setInstallError(null);
    setInstallProgress(null);
    setInstallFilesDone(0);
    setInstallFilesTotal(missingFileCount(models));
    let unsub = () => {};
    try {
      unsub = await onModelsProgress((p) => {
        setInstallProgress(p);
        if (p.status === "done" || p.status === "failed") {
          setInstallFilesDone((n) => n + 1);
        }
      });
      const report = await downloadModels();
      await refreshModels();
      if (!report.ok) {
        const msg =
          report.errors[0] ??
          `Install incomplete (failed ${report.files_failed} file(s))`;
        setError(msg);
        setInstallError(msg);
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      setInstallError(msg);
      await refreshModels();
    } finally {
      unsub();
      setInstalling(false);
    }
  };

  /**
   * "Repair voice models" from a failed setup check: stop Boris (loaded
   * models hold their files open), hash-check and re-download damaged files,
   * start Boris again, then re-run the check so the fix is visible.
   */
  const onRepairModels = async () => {
    if (repair || installing) return;
    const wasOn = engineOn;
    setError(null);
    setRepair({ report: status.self_check ?? null, progress: null });
    let unsub = () => {};
    try {
      if (wasOn) await stopEngine();
      unsub = await onModelsProgress((progress) =>
        setRepair((r) => (r ? { ...r, progress } : r)),
      );
      const report = await downloadModels({ repair: true });
      const ready = await refreshModels();
      if (!report.ok || !ready) {
        throw new Error(report.errors[0] ?? `Repair incomplete (failed ${report.files_failed} file(s))`);
      }
      logger.info("model repair finished", { downloaded: report.files_downloaded });
      setRepair(null);
      if (wasOn) {
        recheckAfterRestart.current = true;
        await onStart();
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("model repair failed", msg);
      setRepair(null);
      // Restart only on a complete install; otherwise Speech settings shows
      // what is missing and offers the normal install.
      const ready = await refreshModels();
      setError(msg);
      if (wasOn && ready) await onStart();
    } finally {
      unsub();
    }
  };

  // "default" = follow the Windows default device (also when unplugged
  // devices come and go). A picked device that is not connected keeps its id
  // so the picker can say so; the engine uses the default until it returns.
  const selectedInput = followsDefaultDevice(settings?.input_device)
    ? SYSTEM_DEFAULT_DEVICE
    : settings!.input_device;

  const selectedOutput = followsDefaultDevice(settings?.output_device)
    ? SYSTEM_DEFAULT_DEVICE
    : settings!.output_device;

  const showAdvanced = Boolean(settings?.show_advanced_settings);

  const finishOnboarding = () => {
    patchSettings({ onboarding_complete: true });
    setOnboardingOpen(false);
    setView("home");
    logger.info("onboarding finished");
    if (!engineOn && !busy) {
      autoStarted.current = true;
      void onStart();
    }
  };

  const skipOnboarding = () => {
    patchSettings({ onboarding_complete: true });
    setOnboardingOpen(false);
    setView("home");
    logger.info("onboarding skipped");
  };

  return (
    <div className="main-console flex h-screen flex-col overflow-hidden text-white antialiased">
      <TitleBar
        title={
          onboardingOpen
            ? "Welcome"
            : view === "settings"
            ? "Settings"
            : view === "debug"
              ? "Developer monitor"
            : view === "teach"
              ? "Your voice"
              : "Boris"
        }
        leading={
          onboardingOpen ? undefined : view === "settings" || view === "debug" ? (
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
          view === "home" && !onboardingOpen ? (
            <div className="flex items-center gap-1">
            {showAdvanced ? (
              <button
                type="button"
                aria-label="Developer monitor"
                title="Developer monitor"
                onClick={() => setView("debug")}
                className="main-settings-button inline-flex size-8 items-center justify-center rounded-[9px] text-white/42 transition-[color,background-color,transform] duration-150 hover:bg-white/[0.07] hover:text-white active:scale-[0.94] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/20"
              >
                <Activity className="size-4" strokeWidth={1.75} />
              </button>
            ) : null}
            <button
              type="button"
              aria-label="Settings"
              onClick={() => setView("settings")}
              className="main-settings-button inline-flex size-8 items-center justify-center rounded-[9px] text-white/42 transition-[color,background-color,transform] duration-150 hover:bg-white/[0.07] hover:text-white active:scale-[0.94] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/20"
            >
              <SettingsIcon className="size-4" strokeWidth={1.75} />
            </button>
            </div>
          ) : undefined
        }
      />

      <main ref={mainScrollRef} className="main-scroll min-h-0 flex-1 overflow-y-auto">
        <AnimatePresence
          mode="wait"
          initial={false}
          onExitComplete={() => mainScrollRef.current?.scrollTo({ top: 0 })}
        >
          <motion.div
            key={onboardingOpen ? "onboarding" : view}
            className="min-h-full"
            initial={
              reduceMotion ? false : { opacity: 0, y: 5 }
            }
            animate={{ opacity: 1, y: 0 }}
            exit={
              reduceMotion
                ? { opacity: 1 }
                : { opacity: 0, y: -3 }
            }
            transition={
              reduceMotion
                ? { duration: 0 }
                : { duration: 0.16, ease: [0.22, 1, 0.36, 1] }
            }
          >
            {onboardingOpen && settings ? (
              <OnboardingFlow
                settings={settings}
                inputs={inputs}
                outputs={outputs}
                selectedInput={selectedInput}
                selectedOutput={selectedOutput}
                modelCheckState={modelCheckState}
                installing={installing}
                installProgress={installProgress}
                installFilesDone={installFilesDone}
                installFilesTotal={installFilesTotal}
                installError={installError}
                onPatch={patchSettings}
                onInputChange={(id) => void onInputChange(id)}
                onOutputChange={(id) => void onOutputChange(id)}
                onInstall={() => void onInstallModels()}
                onOpenExternal={openExternal}
                onFinish={finishOnboarding}
                onSkip={skipOnboarding}
              />
            ) : view === "home" ? (
              <HomeView
                showDetails={showAdvanced}
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
                onInstallUpdate={() => void installAvailableUpdate()}
                onDismissUpdate={() => setUpdateBannerDismissed(true)}
                onOpenSettings={(category = "general") => {
                  setSettingsCategory(category);
                  setView("settings");
                }}
                onTeachVoice={() => setView("teach")}
                onRunSetupCheck={
                  status.engine === "On" && !setupCheckOpen ? startSetupCheck : undefined
                }
              />
            ) : view === "debug" ? (
              <DebugView status={status} />
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
                logPath={logPath}
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
                onOpenMonitor={() => setView("debug")}
                onReplayOnboarding={() => {
                  setView("home");
                  setOnboardingOpen(true);
                }}
                onCheckUpdate={() => void runUpdateCheck()}
                onInstallUpdate={() => void installAvailableUpdate()}
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

      <AnimatePresence>
        {setupCheckOpen && !onboardingOpen ? (
          <SetupCheckToast
            key="setup-check"
            report={setupReport}
            keyStatus={keyStatus}
            appVersion={appVer ?? ""}
            waiting={queuedSelfCheck}
            repairLabel={repair ? repairLabel(repair.progress) : null}
            onRunAgain={startSetupCheck}
            onRetryKey={recheckKey}
            onRepairModels={() => void onRepairModels()}
            onDismiss={dismissSetupCheck}
            onOpenSettings={openSettingsFromToast}
          />
        ) : null}
      </AnimatePresence>
    </div>
  );
}
