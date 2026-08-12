import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  AlertCircle,
  Check,
  ChevronLeft,
  ChevronRight,
  Download,
  ExternalLink,
  Eye,
  EyeOff,
  LoaderCircle,
  Power,
  PowerOff,
  RefreshCw,
  Settings as SettingsIcon,
} from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Update } from "@tauri-apps/plugin-updater";
import { TitleBar } from "@/components/TitleBar";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  SettingsField,
  SettingsGroup,
  SettingsRow,
  Toggle,
} from "@/components/settings";
import {
  downloadModels,
  EMPTY_SETTINGS,
  formatContextMeter,
  getModelsStatus,
  getSettings,
  listInputDevices,
  listOutputDevices,
  MODEL_PRESETS,
  onModelsProgress,
  PROVIDER_PRESETS,
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
  type StatusPicture,
} from "@/bridge";
import { getLogPath, logger } from "@/lib/logger";
import { toneFor } from "@/lib/phaseVisual";
import {
  conversationLines,
  humanizeActivity,
} from "@/lib/statusPresentation";
import {
  appVersion,
  checkForUpdate,
  downloadAndInstallUpdate,
  type AvailableUpdate,
  type UpdateProgress,
} from "@/lib/updater";
import { cn } from "@/lib/utils";

type View = "home" | "settings";
type SettingsCategory = "general" | "overlay" | "speech" | "connections" | "advanced";
type ModelCheckState = "loading" | "ready" | "missing" | "error";
type SaveState = "idle" | "saving" | "saved" | "error";
type UpdateUiState =
  | "idle"
  | "checking"
  | "up_to_date"
  | "available"
  | "downloading"
  | "error";

const CAPABILITY_OPTIONS: { id: string; label: string; footer: string }[] = [
  {
    id: "full",
    label: "Full",
    footer: "Shell, web, and files. Approvals still apply.",
  },
  {
    id: "local_power",
    label: "Local only",
    footer: "Files and system helpers. No shell or network.",
  },
  {
    id: "voice_safe",
    label: "Voice-safe",
    footer: "Light tools only — time, notes, profile, skills.",
  },
];

/** Compact trailing control (Tools, Voice, short picks). */
const selectCompactClass = cn(
  "h-9 max-w-[11rem] appearance-none rounded-lg border-0",
  "bg-white/[0.08] px-3 text-[14px] text-white/90 outline-none",
  "focus-visible:ring-2 focus-visible:ring-white/15",
  "disabled:cursor-not-allowed disabled:opacity-40",
);

/** Device / long labels — take available trailing width, ellipsize. */
const selectDeviceClass = cn(
  "h-9 w-full min-w-0 max-w-full appearance-none rounded-lg border-0",
  "bg-white/[0.08] px-3 text-[13px] text-white/90 outline-none",
  "focus-visible:ring-2 focus-visible:ring-white/15",
  "disabled:cursor-not-allowed disabled:opacity-40",
);

const fieldInputClass = cn(
  "h-10 border-0 bg-white/[0.08] text-[14px] text-white",
  "placeholder:text-white/30 focus-visible:border-transparent",
  "focus-visible:ring-2 focus-visible:ring-white/15",
);

function shortDeviceName(name: string): string {
  // "Microphone (Razer BlackShark V2)" → keep full if short; else drop prefix.
  const m = name.match(/^\s*(?:Microphone|Speakers?|Headset)\s*\((.+)\)\s*$/i);
  if (m?.[1]) return m[1].trim();
  return name;
}

/**
 * Main window — Home (run) + Settings (prefs).
 * Quiet, Apple-like surface. Motion stays on the overlay island.
 */
export function MainWindow() {
  const status = useStatus();
  const tone = useMemo(
    () => toneFor(status.phase, status.engine),
    [status.phase, status.engine],
  );

  const [view, setView] = useState<View>("home");
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
  );

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
  const savedIndicatorTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  const flushSave = useCallback(async (next: AppSettings) => {
    try {
      await saveSettings(next);
      setSaveState("saved");
      if (savedIndicatorTimer.current) clearTimeout(savedIndicatorTimer.current);
      savedIndicatorTimer.current = setTimeout(() => setSaveState("idle"), 1800);
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
      if (savedIndicatorTimer.current) clearTimeout(savedIndicatorTimer.current);
      saveTimer.current = setTimeout(() => {
        void flushSave(next);
      }, 320);
    },
    [flushSave],
  );

  useEffect(() => {
    return () => {
      if (saveTimer.current) clearTimeout(saveTimer.current);
      if (savedIndicatorTimer.current) clearTimeout(savedIndicatorTimer.current);
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

  const runUpdateCheck = useCallback(async (opts?: { silent?: boolean }) => {
    const silent = opts?.silent ?? false;
    if (installingUpdateRef.current) return;
    if (!silent) {
      setUpdateUi("checking");
      setUpdateError(null);
    }
    const result = await checkForUpdate();
    // Don't clobber an in-flight install if one started while we were checking.
    if (installingUpdateRef.current) return;
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

  // Quiet startup check — only surfaces UI when an update exists.
  useEffect(() => {
    const t = window.setTimeout(() => {
      void runUpdateCheck({ silent: true });
    }, 2500);
    return () => window.clearTimeout(t);
  }, [runUpdateCheck]);

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
      if (settings.input_device && inputId === settings.input_device) void switchInput(inputId).catch((e) => {
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
      if (settings.output_device && outputId === settings.output_device) void switchOutput(outputId).catch((e) => {
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
    settings?.input_device &&
    inputs.some((d) => d.id === settings.input_device)
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
    <div className="main-console flex h-screen flex-col overflow-hidden text-white">
      <TitleBar
        title={view === "settings" ? "Settings" : "Boris"}
        leading={
          view === "settings" ? (
            <button
              type="button"
              onClick={() => setView("home")}
              className="inline-flex items-center gap-0.5 rounded-md px-1.5 py-1 text-[13px] font-medium text-white/70 transition-colors hover:bg-white/[0.06] hover:text-white"
            >
              <ChevronLeft className="size-4" strokeWidth={2} />
              Home
            </button>
          ) : undefined
        }
        trailing={
          view === "home" ? (
            <button
              type="button"
              aria-label="Settings"
              onClick={() => setView("settings")}
              className="inline-flex size-8 items-center justify-center rounded-lg text-white/45 transition-colors hover:bg-white/[0.06] hover:text-white"
            >
              <SettingsIcon className="size-4" strokeWidth={1.75} />
            </button>
          ) : undefined
        }
      />

      <main className="min-h-0 flex-1 overflow-y-auto">
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
            onToggleAdvanced={() => setAdvancedOpen((v) => !v)}
            onCheckUpdate={() => void runUpdateCheck()}
            onInstallUpdate={() => void onInstallUpdate()}
          />
        ) : (
          <div className="flex h-full items-center justify-center text-[13px] text-white/40">
            Loading…
          </div>
        )}
      </main>
    </div>
  );
}

/* ── Home ─────────────────────────────────────────────────────────────────── */

function HomeView({
  status,
  tone,
  contextMeter,
  engineOn,
  engineFault,
  modelsReady,
  modelCheckState,
  busy,
  error,
  models,
  installing,
  installProgress,
  availableUpdate,
  updateUi,
  updateProgress,
  updateBannerDismissed,
  onStart,
  onStop,
  onInstall,
  onInstallUpdate,
  onDismissUpdate,
  onOpenSettings,
}: {
  status: StatusPicture;
  tone: ReturnType<typeof toneFor>;
  contextMeter: string | null;
  engineOn: boolean;
  engineFault: boolean;
  modelsReady: boolean;
  modelCheckState: ModelCheckState;
  busy: boolean;
  error: string | null;
  models: ModelsStatus | null;
  installing: boolean;
  installProgress: DownloadProgress | null;
  availableUpdate: AvailableUpdate | null;
  updateUi: UpdateUiState;
  updateProgress: UpdateProgress | null;
  updateBannerDismissed: boolean;
  onStart: () => void;
  onStop: () => void;
  onInstall: () => void;
  onInstallUpdate: () => void;
  onDismissUpdate: () => void;
  onOpenSettings: (category?: SettingsCategory) => void;
}) {
  const act = humanizeActivity(status.activity);
  const showActivity =
    act &&
    (status.phase === "Thinking" || status.phase === "AwaitingConfirm");
  const stopAvailable = engineOn || engineFault;
  const showUpdateBanner =
    availableUpdate != null &&
    !updateBannerDismissed &&
    (updateUi === "available" || updateUi === "downloading");

  return (
    <div className="mx-auto flex min-h-full w-full max-w-3xl flex-col gap-5 px-6 py-7">
      {/* Status strip — no glow card, no orb */}
      <section aria-live="polite" className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-baseline gap-x-2.5 gap-y-1">
            <span
              className="mt-1.5 size-2 shrink-0 rounded-full"
              style={{ background: tone.accent }}
              aria-hidden
            />
            <h1 className="text-[22px] font-semibold tracking-[-0.02em] text-white">
              {tone.label}
            </h1>
          </div>
          <p className="mt-1 pl-[18px] text-[13px] leading-snug text-white/45">
            {tone.hint}
          </p>
          {showActivity ? (
            <p className="mt-1.5 pl-[18px] text-[12px] text-white/50">{act}</p>
          ) : null}
          {engineOn && (status.turn || contextMeter) ? (
            <p className="mt-2 pl-[18px] text-[11px] text-white/30">
              {status.turn ? `Current turn ${status.turn}` : ""}
              {status.turn && contextMeter ? " · " : ""}
              {contextMeter ? `Context ${contextMeter}` : ""}
            </p>
          ) : null}
        </div>

        <div className="flex shrink-0 gap-2">
          <Button
            type="button"
            size="lg"
            disabled={busy}
            onClick={stopAvailable ? onStop : onStart}
            className={cn(
              "h-10 gap-2 rounded-full px-5 text-[13px] font-semibold",
              "bg-white text-[#0b0b0c] hover:bg-white/90",
              stopAvailable
                ? "border border-white/10 bg-transparent text-white/75 hover:bg-white/[0.06]"
                : "bg-white text-[#0b0b0c] hover:bg-white/90",
              "disabled:bg-white/15 disabled:text-white/35",
            )}
            title={
              !stopAvailable && !modelsReady
                ? "Open Speech settings to finish setup"
                : undefined
            }
          >
            {busy ? (
              <LoaderCircle className="size-3.5 animate-spin" strokeWidth={2.25} />
            ) : stopAvailable ? (
              <PowerOff className="size-3.5" strokeWidth={2} />
            ) : (
              <Power className="size-3.5" strokeWidth={2.25} />
            )}
            {busy ? (stopAvailable ? "Stopping…" : "Starting…") : engineFault ? "Reset" : engineOn ? "Stop" : "Start"}
          </Button>
        </div>
      </section>

      {error ? (
        <p role="alert" className="rounded-xl bg-red-500/10 px-3.5 py-2.5 text-[13px] text-red-300 ring-1 ring-red-500/15">
          {error}
        </p>
      ) : null}
      {engineFault && !error && !status.detail ? (
        <p role="alert" className="text-[13px] text-amber-200/80">
          Something went wrong — try Stop, then Start again.
        </p>
      ) : null}

      {showUpdateBanner && availableUpdate ? (
        <UpdateBanner
          update={availableUpdate}
          downloading={updateUi === "downloading"}
          progress={updateProgress}
          onInstall={onInstallUpdate}
          onDismiss={onDismissUpdate}
          onOpenSettings={() => onOpenSettings("general")}
        />
      ) : null}

      {modelCheckState !== "ready" ? (
        <ModelsBanner
          models={models}
          state={modelCheckState}
          installing={installing}
          progress={installProgress}
          onInstall={onInstall}
          onOpenSettings={() => onOpenSettings("speech")}
        />
      ) : null}

      <ConversationView status={status} />
    </div>
  );
}

function ModelsBanner({
  models,
  state,
  installing,
  progress,
  onInstall,
  onOpenSettings,
}: {
  models: ModelsStatus | null;
  state: ModelCheckState;
  installing: boolean;
  progress: DownloadProgress | null;
  onInstall: () => void;
  onOpenSettings: () => void;
}) {
  const pct =
    installing && progress?.total_bytes && progress.total_bytes > 0
      ? Math.min(
          100,
          Math.round((progress.bytes_downloaded / progress.total_bytes) * 100),
        )
      : null;

  if (state === "loading" && !installing) {
    return (
      <div role="status" className="settings-group flex items-center gap-3 rounded-[12px] px-4 py-3.5 text-[13px] text-white/55">
        <LoaderCircle className="size-4 animate-spin" />
        Checking speech models…
      </div>
    );
  }

  if (state === "error" && !installing) {
    return (
      <div className="settings-group rounded-[12px] px-4 py-3.5">
        <p className="text-[15px] font-medium text-white/90">Speech model check failed</p>
        <p className="mt-1 text-[12px] text-white/50">Open Speech settings to retry. No download has started.</p>
        <button type="button" onClick={onOpenSettings} className="mt-3 min-h-9 rounded-full bg-white/10 px-3.5 text-[13px] text-white/80 hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25">
          Open Speech settings
        </button>
      </div>
    );
  }

  return (
    <div className="settings-group rounded-[12px] px-4 py-3.5">
      <p className="text-[15px] font-medium tracking-[-0.01em] text-white/90">
        Download speech models to start
      </p>
      <p className="mt-1 text-[12px] leading-snug text-white/40">
        About 900 MB, stored on this computer.
        {models?.missing?.length
          ? ` ${models.missing.length} files missing.`
          : ""}
      </p>
      {installing ? (
        <div className="mt-3">
          <div
            className="main-progress-track"
            role="progressbar"
            aria-label="Speech model download"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={pct ?? undefined}
          >
            <div
              className="main-progress-fill"
              style={{ width: `${pct ?? 4}%` }}
            />
          </div>
          <p className="mt-1.5 truncate text-[12px] text-white/40">
            {progress?.file_name ?? "Preparing…"}
            {pct != null ? ` · ${pct}%` : ""}
          </p>
        </div>
      ) : null}
      <div className="mt-3 flex flex-wrap gap-2">
        <Button
          type="button"
          size="sm"
          disabled={installing}
          onClick={onInstall}
          className="h-8 gap-1.5 rounded-full bg-white/10 px-3.5 text-[13px] text-white hover:bg-white/15"
        >
          <Download className="size-3.5" strokeWidth={2} />
          {installing ? "Downloading…" : "Download"}
        </Button>
        <button
          type="button"
          onClick={onOpenSettings}
          className="h-8 rounded-full px-3 text-[13px] text-white/45 hover:text-white/70"
        >
          Details
        </button>
      </div>
    </div>
  );
}

function ConversationView({ status }: { status: StatusPicture }) {
  const lines = conversationLines(status);

  return (
    <section
      aria-labelledby="current-turn-heading"
      aria-live="polite"
      className="settings-group flex min-h-[220px] flex-col gap-4 rounded-[16px] px-5 py-4"
    >
      <h2 id="current-turn-heading" className="text-[12px] font-medium uppercase tracking-[0.08em] text-white/40">
        Current turn
      </h2>
      {lines.map((line, i) => {
        switch (line.kind) {
          case "placeholder":
            return (
              <p key={`p-${i}`} className="text-[15px] leading-relaxed text-white/30">
                {line.text}
              </p>
            );
          case "error":
            return (
              <p
                key={`e-${i}`}
                className="rounded-xl bg-red-500/10 px-3.5 py-2.5 text-[13px] text-red-300/90"
              >
                {line.text}
              </p>
            );
          case "status":
            return (
              <p key={`s-${i}`} className="text-[13px] text-white/40">
                {line.text}
              </p>
            );
          case "confirm":
            return (
              <div
                key={`c-${i}`}
                className="rounded-[12px] bg-amber-500/[0.08] px-4 py-3 ring-1 ring-amber-400/15"
              >
                <p className="text-[13px] font-medium text-amber-100/70">
                  Needs your OK
                  {line.activity ? (
                    <span className="ml-1.5 font-normal text-amber-100/45">
                      · {line.activity}
                    </span>
                  ) : null}
                </p>
                <p className="mt-1.5 text-[15px] leading-relaxed text-white/88">
                  {line.prompt}
                </p>
                <p className="mt-2 text-[12px] text-amber-100/40">
                  Say yes, no, sure, or cancel — no wake word
                </p>
              </div>
            );
          case "you":
            return (
              <Bubble key={`y-${i}`} who="You" text={line.text} muted={line.muted} />
            );
          case "boris":
            return <Bubble key={`b-${i}`} who="Boris" text={line.text} accent />;
          default:
            return null;
        }
      })}
    </section>
  );
}

function Bubble({
  who,
  text,
  accent,
  muted,
}: {
  who: string;
  text: string;
  accent?: boolean;
  muted?: boolean;
}) {
  return (
    <div className={cn("flex flex-col gap-1", muted && "opacity-50")}>
      <span className="text-[12px] text-white/35">{who}</span>
      <p
        className={cn(
          "max-w-[95%] break-words text-[15px] leading-relaxed tracking-[-0.01em]",
          accent ? "text-white/90" : "text-white/70",
        )}
      >
        {text}
      </p>
    </div>
  );
}

/* ── Settings ─────────────────────────────────────────────────────────────── */

function SettingsView({
  settings, engineOn, busy, inputs, outputs, selectedInput, selectedOutput,
  modelsReady, modelCheckState, installing, installProgress, advancedOpen,
  logPath, capability, error, saveState, category, appVer, updateUi,
  availableUpdate, updateProgress, updateError, onCategoryChange, onPatch,
  onInputChange, onOutputChange, onRefreshDevices, onRefreshModels, onInstall,
  onToggleAdvanced, onCheckUpdate, onInstallUpdate,
}: {
  settings: AppSettings; engineOn: boolean; busy: boolean; inputs: DeviceDto[];
  outputs: DeviceDto[]; selectedInput: string; selectedOutput: string;
  modelsReady: boolean; modelCheckState: ModelCheckState; installing: boolean;
  installProgress: DownloadProgress | null; advancedOpen: boolean; logPath: string;
  capability: (typeof CAPABILITY_OPTIONS)[number]; error: string | null;
  saveState: SaveState; category: SettingsCategory;
  appVer: string | null;
  updateUi: UpdateUiState;
  availableUpdate: AvailableUpdate | null;
  updateProgress: UpdateProgress | null;
  updateError: string | null;
  onCategoryChange: (category: SettingsCategory) => void;
  onPatch: (p: Partial<AppSettings>) => void; onInputChange: (id: string) => void;
  onOutputChange: (id: string) => void; onRefreshDevices: () => void;
  onRefreshModels: () => void; onInstall: () => void; onToggleAdvanced: () => void;
  onCheckUpdate: () => void; onInstallUpdate: () => void;
}) {
  const locked = engineOn;
  const [showOpenRouterKey, setShowOpenRouterKey] = useState(false);
  const [showExaKey, setShowExaKey] = useState(false);
  const categories: { id: SettingsCategory; label: string }[] = [
    { id: "general", label: "General" }, { id: "overlay", label: "Overlay" },
    { id: "speech", label: "Speech" }, { id: "connections", label: "Connections" },
    { id: "advanced", label: "Advanced" },
  ];
  const openExternal = (url: string) => {
    void openUrl(url).catch(() => window.open(url, "_blank", "noopener,noreferrer"));
  };
  return (
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-5 px-5 py-5 pb-12">
      <div className="flex items-center gap-2 overflow-x-auto pb-1" role="tablist" aria-label="Settings categories">
        {categories.map((item) => <button key={item.id} id={`settings-tab-${item.id}`} type="button" role="tab" aria-controls="settings-panel" aria-selected={category === item.id} onClick={() => onCategoryChange(item.id)} className={cn("min-h-9 shrink-0 rounded-full px-3.5 text-[13px] transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25", category === item.id ? "bg-white text-black" : "bg-white/[0.06] text-white/55 hover:bg-white/10 hover:text-white/85")}>{item.label}</button>)}
        <div role="status" aria-live="polite" className="ml-auto flex min-w-[76px] items-center justify-end gap-1.5 text-[12px] text-white/45">
          {saveState === "saving" ? <><LoaderCircle className="size-3 animate-spin" /> Saving…</> : null}
          {saveState === "saved" ? <><Check className="size-3 text-emerald-300" /> Saved</> : null}
          {saveState === "error" ? <><AlertCircle className="size-3 text-red-300" /> Save failed</> : null}
        </div>
      </div>
      {error ? <p role="alert" className="rounded-xl bg-red-500/10 px-3.5 py-2.5 text-[13px] text-red-300 ring-1 ring-red-500/15">{error}</p> : null}
      <div id="settings-panel" role="tabpanel" aria-labelledby={`settings-tab-${category}`}>
        {category === "general" ? (
          <GeneralSettings
            settings={settings}
            locked={locked}
            capability={capability}
            appVer={appVer}
            updateUi={updateUi}
            availableUpdate={availableUpdate}
            updateProgress={updateProgress}
            updateError={updateError}
            onPatch={onPatch}
            onCheckUpdate={onCheckUpdate}
            onInstallUpdate={onInstallUpdate}
          />
        ) : null}
        {category === "overlay" ? <OverlaySettings settings={settings} onPatch={onPatch} /> : null}
        {category === "speech" ? <SpeechSettings busy={busy} inputs={inputs} outputs={outputs} selectedInput={selectedInput} selectedOutput={selectedOutput} modelsReady={modelsReady} modelCheckState={modelCheckState} installing={installing} progress={installProgress} onInputChange={onInputChange} onOutputChange={onOutputChange} onRefreshDevices={onRefreshDevices} onRefreshModels={onRefreshModels} onInstall={onInstall} /> : null}
        {category === "connections" ? <ConnectionsSettings settings={settings} locked={locked} showOpenRouterKey={showOpenRouterKey} showExaKey={showExaKey} onToggleOpenRouter={() => setShowOpenRouterKey((v) => !v)} onToggleExa={() => setShowExaKey((v) => !v)} onOpenExternal={openExternal} onPatch={onPatch} /> : null}
        {category === "advanced" ? <AdvancedSettings settings={settings} locked={locked} advancedOpen={advancedOpen} logPath={logPath} onToggleAdvanced={onToggleAdvanced} onPatch={onPatch} /> : null}
      </div>
    </div>
  );
}

function GeneralSettings({
  settings,
  locked,
  capability,
  appVer,
  updateUi,
  availableUpdate,
  updateProgress,
  updateError,
  onPatch,
  onCheckUpdate,
  onInstallUpdate,
}: {
  settings: AppSettings;
  locked: boolean;
  capability: (typeof CAPABILITY_OPTIONS)[number];
  appVer: string | null;
  updateUi: UpdateUiState;
  availableUpdate: AvailableUpdate | null;
  updateProgress: UpdateProgress | null;
  updateError: string | null;
  onPatch: (p: Partial<AppSettings>) => void;
  onCheckUpdate: () => void;
  onInstallUpdate: () => void;
}) {
  const checking = updateUi === "checking";
  const downloading = updateUi === "downloading";
  const hasUpdate = updateUi === "available" || downloading;
  const versionLabel = appVer ? `v${appVer}` : "—";
  const statusSubtitle = checking
    ? "Checking…"
    : downloading
      ? "Downloading update…"
      : hasUpdate && availableUpdate
        ? `v${availableUpdate.version} ready`
        : updateUi === "up_to_date"
          ? "You're on the latest version"
          : updateUi === "error"
            ? "Check failed"
            : "Check for a newer installer";
  const pct =
    updateProgress?.contentLength != null && updateProgress.contentLength > 0
      ? Math.min(
          100,
          Math.round(
            (updateProgress.downloaded / updateProgress.contentLength) * 100,
          ),
        )
      : null;

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup
        title="General"
        footer={
          locked
            ? "Some changes apply after Boris is stopped."
            : capability.footer
        }
      >
        <SettingsRow label="Start Boris when the app opens">
          <Toggle
            checked={settings.start_engine_on_launch}
            onChange={(v) => onPatch({ start_engine_on_launch: v })}
            aria-label="Start Boris when the app opens"
          />
        </SettingsRow>
        <SettingsRow
          label="Long-term memory"
          subtitle="Remember notes across sessions"
        >
          <Toggle
            checked={settings.long_term_memory}
            disabled={locked}
            onChange={(v) => onPatch({ long_term_memory: v })}
            aria-label="Long-term memory"
          />
        </SettingsRow>
        <SettingsRow label="Tool access" labelFor="capability-preset" last>
          <select
            id="capability-preset"
            className={selectCompactClass}
            value={
              CAPABILITY_OPTIONS.some((p) => p.id === settings.capability_preset)
                ? settings.capability_preset
                : "full"
            }
            disabled={locked}
            onChange={(e) => onPatch({ capability_preset: e.target.value })}
          >
            {CAPABILITY_OPTIONS.map((p) => (
              <option key={p.id} value={p.id} className="bg-[#1c1c1e] text-white">
                {p.label}
              </option>
            ))}
          </select>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="Updates"
        footer="Updates are signed and downloaded from GitHub Releases. The app restarts after install."
      >
        <SettingsRow
          label="Version"
          subtitle={statusSubtitle}
          last={!hasUpdate && updateUi !== "error"}
        >
          <div className="flex items-center gap-2">
            <span className="rounded-lg bg-white/[0.06] px-3 py-2 text-[13px] text-white/65">
              {versionLabel}
            </span>
            {hasUpdate ? (
              <button
                type="button"
                disabled={downloading}
                onClick={onInstallUpdate}
                className="min-h-9 rounded-lg bg-white px-3 text-[13px] font-medium text-black hover:bg-white/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40"
              >
                {downloading ? "Installing…" : "Install"}
              </button>
            ) : (
              <button
                type="button"
                disabled={checking}
                onClick={onCheckUpdate}
                className="inline-flex min-h-9 items-center gap-1.5 rounded-lg px-3 text-[13px] font-medium text-white/75 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40"
              >
                {checking ? (
                  <LoaderCircle className="size-3.5 animate-spin" />
                ) : (
                  <RefreshCw className="size-3.5" />
                )}
                {checking ? "Checking…" : "Check"}
              </button>
            )}
          </div>
        </SettingsRow>
        {downloading ? (
          <div className="border-t border-white/[0.06] px-4 py-3" role="status">
            <div
              className="main-progress-track"
              role="progressbar"
              aria-label="App update download"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={pct ?? undefined}
            >
              <div
                className="main-progress-fill"
                style={{ width: `${pct ?? 4}%` }}
              />
            </div>
            <p className="mt-1.5 text-[12px] text-white/50">
              {pct != null ? `${pct}%` : "Downloading…"}
            </p>
          </div>
        ) : null}
        {updateUi === "error" && updateError ? (
          <div className="border-t border-white/[0.06] px-4 py-3">
            <p className="text-[12px] leading-snug text-red-300/90" role="alert">
              {updateError}
            </p>
          </div>
        ) : null}
        {hasUpdate && availableUpdate?.body ? (
          <div className="border-t border-white/[0.06] px-4 py-3">
            <p className="whitespace-pre-wrap text-[12px] leading-snug text-white/45">
              {availableUpdate.body}
            </p>
          </div>
        ) : null}
      </SettingsGroup>
    </div>
  );
}

function UpdateBanner({
  update,
  downloading,
  progress,
  onInstall,
  onDismiss,
  onOpenSettings,
}: {
  update: AvailableUpdate;
  downloading: boolean;
  progress: UpdateProgress | null;
  onInstall: () => void;
  onDismiss: () => void;
  onOpenSettings: () => void;
}) {
  const pct =
    progress?.contentLength != null && progress.contentLength > 0
      ? Math.min(
          100,
          Math.round((progress.downloaded / progress.contentLength) * 100),
        )
      : null;

  return (
    <div className="settings-group rounded-[12px] px-4 py-3.5" role="status">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-[14px] font-medium text-white/90">
            Update available · v{update.version}
          </p>
          <p className="mt-0.5 text-[12px] text-white/50">
            {downloading
              ? pct != null
                ? `Downloading… ${pct}%`
                : "Downloading…"
              : `You have v${update.currentVersion}. Install when you're ready.`}
          </p>
        </div>
        <div className="flex shrink-0 gap-2">
          {!downloading ? (
            <button
              type="button"
              onClick={onDismiss}
              className="min-h-9 rounded-lg px-3 text-[13px] text-white/55 hover:bg-white/[0.06] hover:text-white/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
            >
              Later
            </button>
          ) : null}
          <button
            type="button"
            disabled={downloading}
            onClick={onInstall}
            className="min-h-9 rounded-full bg-white px-3.5 text-[13px] font-medium text-black hover:bg-white/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40"
          >
            {downloading ? "Installing…" : "Install"}
          </button>
        </div>
      </div>
      {downloading ? (
        <div className="mt-3">
          <div
            className="main-progress-track"
            role="progressbar"
            aria-label="App update download"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={pct ?? undefined}
          >
            <div
              className="main-progress-fill"
              style={{ width: `${pct ?? 4}%` }}
            />
          </div>
        </div>
      ) : (
        <button
          type="button"
          onClick={onOpenSettings}
          className="mt-2 text-[12px] text-white/45 hover:text-white/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
        >
          Details in Settings
        </button>
      )}
    </div>
  );
}

function OverlaySettings({ settings, onPatch }: { settings: AppSettings; onPatch: (p: Partial<AppSettings>) => void }) {
  return <SettingsGroup title="Overlay" footer="Caption privacy controls what can be visible during screen sharing and recordings.">
    <SettingsRow label="Show overlay when Boris wakes"><Toggle checked={settings.show_overlay_on_wake} onChange={(v) => onPatch({ show_overlay_on_wake: v })} aria-label="Show overlay when Boris wakes" /></SettingsRow>
    <SettingsRow label="Captions" subtitle="Choose which spoken text appears" labelFor="overlay-captions"><select id="overlay-captions" className={selectCompactClass} value={settings.overlay_caption_mode} onChange={(e) => onPatch({ overlay_caption_mode: e.target.value as AppSettings["overlay_caption_mode"] })}><option value="full">You and Boris</option><option value="assistant">Boris only</option><option value="hidden">Hidden</option></select></SettingsRow>
    <SettingsRow label="Position" labelFor="overlay-position"><select id="overlay-position" className={selectCompactClass} value={settings.overlay_position} onChange={(e) => onPatch({ overlay_position: e.target.value as AppSettings["overlay_position"] })}><option value="top_center">Top center</option><option value="top_left">Top left</option><option value="top_right">Top right</option></select></SettingsRow>
    <SettingsField label={`Scale · ${settings.overlay_scale_percent}%`} labelFor="overlay-scale" last><input id="overlay-scale" type="range" min={75} max={125} step={5} value={settings.overlay_scale_percent} onChange={(e) => onPatch({ overlay_scale_percent: Number(e.target.value) })} className="h-9 w-full accent-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25" /></SettingsField>
  </SettingsGroup>;
}

function SpeechSettings({ busy, inputs, outputs, selectedInput, selectedOutput, modelsReady, modelCheckState, installing, progress, onInputChange, onOutputChange, onRefreshDevices, onRefreshModels, onInstall }: { busy: boolean; inputs: DeviceDto[]; outputs: DeviceDto[]; selectedInput: string; selectedOutput: string; modelsReady: boolean; modelCheckState: ModelCheckState; installing: boolean; progress: DownloadProgress | null; onInputChange: (id: string) => void; onOutputChange: (id: string) => void; onRefreshDevices: () => void; onRefreshModels: () => void; onInstall: () => void }) {
  const label = modelCheckState === "loading" ? "Checking…" : modelCheckState === "error" ? "Check failed" : modelsReady ? "Ready" : installing ? "Downloading…" : "Not installed";
  return <div className="flex flex-col gap-6">
    <SettingsGroup id="speech-models" title="Speech Models" footer="Speech processing runs locally on this computer.">
      <SettingsRow label="Voice" subtitle="More voices are coming"><span className="rounded-lg bg-white/[0.06] px-3 py-2 text-[13px] text-white/65">M4 · Default</span></SettingsRow>
      <SettingsRow label="Local speech models" subtitle={label} last={!installing}>
        <button type="button" disabled={installing || modelCheckState === "loading"} onClick={modelCheckState === "missing" ? onInstall : onRefreshModels} className="min-h-9 rounded-lg px-3 text-[13px] font-medium text-white/75 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40">{installing ? "Downloading…" : modelCheckState === "missing" ? "Download" : modelCheckState === "error" ? "Retry" : "Refresh"}</button>
      </SettingsRow>
      {installing ? <DownloadProgressView progress={progress} /> : null}
    </SettingsGroup>
    <SettingsGroup title="Sound" action={<button type="button" disabled={busy} onClick={onRefreshDevices} className="inline-flex min-h-9 items-center gap-1.5 rounded-lg px-2.5 text-[12px] text-white/55 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40"><RefreshCw className="size-3.5" /> Refresh devices</button>}>
      <SettingsRow label="Microphone" labelFor="input-device" stacked><select id="input-device" className={selectDeviceClass} value={selectedInput} disabled={busy || inputs.length === 0} title={inputs.find((d) => d.id === selectedInput)?.name} onChange={(e) => onInputChange(e.target.value)}>{inputs.length === 0 ? <option value="">No devices found</option> : inputs.map((d) => <option key={d.id} value={d.id} className="bg-[#1c1c1e] text-white">{shortDeviceName(d.name)}{d.is_default ? " · default" : ""}</option>)}</select></SettingsRow>
      <SettingsRow label="Speakers" labelFor="output-device" stacked last><select id="output-device" className={selectDeviceClass} value={selectedOutput} disabled={busy || outputs.length === 0} title={outputs.find((d) => d.id === selectedOutput)?.name} onChange={(e) => onOutputChange(e.target.value)}>{outputs.length === 0 ? <option value="">No devices found</option> : outputs.map((d) => <option key={d.id} value={d.id} className="bg-[#1c1c1e] text-white">{shortDeviceName(d.name)}{d.is_default ? " · default" : ""}</option>)}</select></SettingsRow>
    </SettingsGroup>
  </div>;
}

function ConnectionsSettings({ settings, locked, showOpenRouterKey, showExaKey, onToggleOpenRouter, onToggleExa, onOpenExternal, onPatch }: { settings: AppSettings; locked: boolean; showOpenRouterKey: boolean; showExaKey: boolean; onToggleOpenRouter: () => void; onToggleExa: () => void; onOpenExternal: (url: string) => void; onPatch: (p: Partial<AppSettings>) => void }) {
  return <div className="flex flex-col gap-6">
    <SettingsGroup title="API Keys" footer={locked ? "Stop Boris to change API keys." : "Keys stay in ~/.boris/auth.json on this computer."}>
      <SettingsField label="OpenRouter" subtitle="Required for chat" labelFor="openrouter-key"><SecretField id="openrouter-key" shown={showOpenRouterKey} onToggle={onToggleOpenRouter} value={settings.openrouter_api_key} disabled={locked} placeholder="sk-or-v1-…" onChange={(value) => onPatch({ openrouter_api_key: value })} /><HelpLink onClick={() => onOpenExternal("https://openrouter.ai/keys")}>Get an OpenRouter key</HelpLink></SettingsField>
      <SettingsField label="Exa" subtitle="Optional, for more reliable web search" labelFor="exa-key" last><SecretField id="exa-key" shown={showExaKey} onToggle={onToggleExa} value={settings.exa_api_key} disabled={locked} placeholder="Exa API key" onChange={(value) => onPatch({ exa_api_key: value })} /><HelpLink onClick={() => onOpenExternal("https://dashboard.exa.ai")}>Open Exa dashboard</HelpLink></SettingsField>
    </SettingsGroup>
    <SettingsGroup title="Chat Models" footer={locked ? "Stop Boris to change models." : "The fast model handles simpler requests."}>
      <ModelField label="Primary model" value={settings.openrouter_model} disabled={locked} onChange={(v) => onPatch({ openrouter_model: v })} />
      <ModelField label="Fast model" subtitle="Uses the primary model when unset" value={settings.openrouter_fast_model} disabled={locked} onChange={(v) => onPatch({ openrouter_fast_model: v })} allowEmpty last />
    </SettingsGroup>
  </div>;
}

function AdvancedSettings({ settings, locked, advancedOpen, logPath, onToggleAdvanced, onPatch }: { settings: AppSettings; locked: boolean; advancedOpen: boolean; logPath: string; onToggleAdvanced: () => void; onPatch: (p: Partial<AppSettings>) => void }) {
  return <div className="flex flex-col gap-6">
    <SettingsGroup title="Safety"><SettingsRow label="Trusted mode" subtitle="Automatically allow notes and writes inside the safe workspace" last><Toggle checked={settings.trusted_auto_moderate} disabled={locked} onChange={(v) => onPatch({ trusted_auto_moderate: v })} aria-label="Trusted mode" /></SettingsRow></SettingsGroup>
    <SettingsGroup title="Model Routing" footer="Optional. Leave this on Auto unless a specific inference provider is required.">
      <button type="button" onClick={onToggleAdvanced} aria-expanded={advancedOpen} aria-controls="provider-routing-controls" className="flex min-h-11 w-full items-center justify-between px-4 py-2.5 text-left text-[15px] text-white/80 hover:bg-white/[0.03] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-white/25"><span>Provider routing</span><ChevronRight className={cn("size-4 text-white/35 transition-transform", advancedOpen && "rotate-90")} /></button>
      {advancedOpen ? <div id="provider-routing-controls"><ProviderField label="Primary provider" value={settings.openrouter_model_provider} disabled={locked} onChange={(v) => onPatch({ openrouter_model_provider: v })} /><ProviderField label="Fast provider" value={settings.openrouter_fast_provider} disabled={locked} onChange={(v) => onPatch({ openrouter_fast_provider: v })} /><SettingsRow label="Require selected providers" subtitle="Do not use another provider if these are unavailable" last><Toggle checked={settings.openrouter_pin_provider} disabled={locked || (!settings.openrouter_model_provider.trim() && !settings.openrouter_fast_provider.trim())} onChange={(v) => onPatch({ openrouter_pin_provider: v })} aria-label="Require selected providers" /></SettingsRow></div> : null}
    </SettingsGroup>
    <SettingsGroup title="Diagnostics" footer="The log filter applies after restart."><SettingsField label="Log filter" labelFor="logging-filter"><Input id="logging-filter" placeholder="info or boris=debug" value={settings.logging_filter} onChange={(e) => onPatch({ logging_filter: e.target.value })} className={cn(fieldInputClass, "font-mono text-[13px]")} /></SettingsField>{logPath ? <div className="px-4 py-3"><p className="text-[13px] text-white/55">Log file</p><p className="mt-1 break-all font-mono text-[11px] leading-snug text-white/50" title={logPath}>{logPath.replace(/\\/g, "/")}</p></div> : null}</SettingsGroup>
  </div>;
}

function DownloadProgressView({ progress }: { progress: DownloadProgress | null }) {
  const pct = progress?.total_bytes ? Math.min(100, Math.round((progress.bytes_downloaded / progress.total_bytes) * 100)) : null;
  return <div className="border-t border-white/[0.06] px-4 py-3" role="status" aria-live="polite"><div className="main-progress-track" role="progressbar" aria-label="Speech model download" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined}><div className="main-progress-fill" style={{ width: `${pct ?? 4}%` }} /></div><p className="mt-1.5 truncate text-[12px] text-white/50">{progress?.file_name ?? "Preparing…"}{pct != null ? ` · ${pct}%` : ""}</p></div>;
}

function SecretField({ id, shown, onToggle, value, disabled, placeholder, onChange }: { id: string; shown: boolean; onToggle: () => void; value: string; disabled: boolean; placeholder: string; onChange: (value: string) => void }) {
  return <div className="relative"><Input id={id} type={shown ? "text" : "password"} placeholder={placeholder} value={value} disabled={disabled} autoComplete="off" spellCheck={false} onChange={(e) => onChange(e.target.value)} className={cn(fieldInputClass, "pr-11")} /><button type="button" disabled={disabled} onClick={onToggle} aria-label={shown ? "Hide API key" : "Show API key"} className="absolute right-1 top-1 inline-flex size-8 items-center justify-center rounded-md text-white/45 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40">{shown ? <EyeOff className="size-4" /> : <Eye className="size-4" />}</button></div>;
}

function HelpLink({ onClick, children }: { onClick: () => void; children: React.ReactNode }) {
  return <button type="button" onClick={onClick} className="inline-flex min-h-9 w-fit items-center gap-1.5 rounded-md px-1 text-[12px] text-sky-300/75 hover:text-sky-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25">{children}<ExternalLink className="size-3" /></button>;
}

function ModelField({
  label,
  subtitle,
  value,
  onChange,
  disabled,
  allowEmpty,
  last,
}: {
  label: string;
  subtitle?: string;
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
  allowEmpty?: boolean;
  last?: boolean;
}) {
  const match = MODEL_PRESETS.some((p) => p.id === value);
  const isCustomValue = Boolean(value) && !match;
  // When allowEmpty, empty means "Same as model" — track intentional Custom…
  // so the text field can open before the user types an id.
  const [forceCustom, setForceCustom] = useState(false);
  const [customDraft, setCustomDraft] = useState(value);
  const fieldId = `model-${label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;

  useEffect(() => {
    if (match) setForceCustom(false);
    else if (isCustomValue) setForceCustom(true);
    setCustomDraft(value);
  }, [match, isCustomValue, value]);

  const inCustomMode =
    forceCustom || isCustomValue || (!allowEmpty && !match);
  const selectValue = inCustomMode
    ? "__custom__"
    : !value && allowEmpty
      ? ""
      : match
        ? value
        : "__custom__";
  const showCustom = selectValue === "__custom__";

  return (
    <>
      <SettingsRow label={label} labelFor={fieldId} subtitle={subtitle} last={last && !showCustom}>
        <select
          id={fieldId}
          className={selectCompactClass}
          disabled={disabled}
          value={selectValue}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "__custom__") {
              setForceCustom(true);
              setCustomDraft(isCustomValue ? value : "");
              return;
            }
            setForceCustom(false);
            onChange(v);
          }}
        >
          {allowEmpty ? (
            <option value="" className="bg-[#1c1c1e] text-white">
              Same as model
            </option>
          ) : null}
          {MODEL_PRESETS.map((p) => (
            <option key={p.id} value={p.id} className="bg-[#1c1c1e] text-white">
              {p.label}
            </option>
          ))}
          <option value="__custom__" className="bg-[#1c1c1e] text-white">
            Custom…
          </option>
        </select>
      </SettingsRow>
      {showCustom ? (
        <div
          className={cn(
            "border-b border-white/[0.06] px-4 pb-3",
            last && "border-b-0",
          )}
        >
          <Input
            id={`${fieldId}-custom`}
            aria-label={`Custom ${label.toLowerCase()} identifier`}
            value={customDraft}
            disabled={disabled}
            placeholder="provider/model-id"
            onChange={(e) => {
              setForceCustom(true);
              setCustomDraft(e.target.value);
            }}
            onBlur={() => {
              const next = customDraft.trim();
              if (/^\S+\/\S+$/.test(next)) onChange(next);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const next = customDraft.trim();
                if (/^\S+\/\S+$/.test(next)) onChange(next);
              }
            }}
            className={cn(fieldInputClass, "h-9 w-full font-mono text-[13px]")}
          />
        </div>
      ) : null}
    </>
  );
}

function ProviderField({
  label,
  value,
  onChange,
  disabled,
  last,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
  last?: boolean;
}) {
  const first =
    value
      .split(/[,\s]+/)
      .map((s) => s.trim())
      .filter(Boolean)[0] ?? "";
  const match = PROVIDER_PRESETS.some((p) => p.id === first && p.id !== "");
  const isCustomValue = Boolean(value.trim()) && !match;
  // Empty is "Auto" in the preset list — same Custom… sticky mode as ModelField.
  const [forceCustom, setForceCustom] = useState(false);
  const [customDraft, setCustomDraft] = useState(value);
  const fieldId = `provider-${label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;

  useEffect(() => {
    if (match) setForceCustom(false);
    else if (isCustomValue) setForceCustom(true);
    setCustomDraft(value);
  }, [match, isCustomValue, value]);

  const inCustomMode = forceCustom || isCustomValue;
  const selectValue = inCustomMode ? "__custom__" : match ? first : "";
  const showCustom = selectValue === "__custom__";

  return (
    <>
      <SettingsRow label={label} labelFor={fieldId} last={last && !showCustom}>
        <select
          id={fieldId}
          className={selectCompactClass}
          disabled={disabled}
          value={selectValue}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "__custom__") {
              setForceCustom(true);
              setCustomDraft(isCustomValue ? value : "");
              return;
            }
            setForceCustom(false);
            onChange(v);
          }}
        >
          {PROVIDER_PRESETS.map((p) => (
            <option
              key={p.id || "auto"}
              value={p.id}
              className="bg-[#1c1c1e] text-white"
            >
              {p.label}
            </option>
          ))}
          <option value="__custom__" className="bg-[#1c1c1e] text-white">
            Custom…
          </option>
        </select>
      </SettingsRow>
      {showCustom ? (
        <div className="border-b border-white/[0.06] px-4 pb-3">
          <Input
            id={`${fieldId}-custom`}
            aria-label={`Custom ${label.toLowerCase()} value`}
            value={customDraft}
            disabled={disabled}
            placeholder="coreweave,baseten"
            onChange={(e) => {
              setForceCustom(true);
              setCustomDraft(e.target.value);
            }}
            onBlur={() => {
              const next = customDraft.trim();
              if (next) onChange(next);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && customDraft.trim()) onChange(customDraft.trim());
            }}
            className={cn(fieldInputClass, "h-9 w-full font-mono text-[13px]")}
          />
        </div>
      ) : null}
    </>
  );
}
