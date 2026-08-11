import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  ChevronLeft,
  ChevronRight,
  Download,
  Power,
  PowerOff,
  RefreshCw,
  Settings as SettingsIcon,
} from "lucide-react";
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
import { cn } from "@/lib/utils";

type View = "home" | "settings";

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
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [inputs, setInputs] = useState<DeviceDto[]>([]);
  const [outputs, setOutputs] = useState<DeviceDto[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [models, setModels] = useState<ModelsStatus | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installProgress, setInstallProgress] =
    useState<DownloadProgress | null>(null);
  const [logPath, setLogPath] = useState("");
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const autoStarted = useRef(false);

  const engineOn = status.engine === "On" || status.engine === "Starting";
  const engineFault = status.engine === "Fault";
  const modelsReady = Boolean(
    models?.parakeet_ready && models?.supertone_ready,
  );
  const contextMeter = formatContextMeter(
    status.context_used,
    status.context_limit,
  );

  const refreshDevices = useCallback(async () => {
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
    try {
      setModels(await getModelsStatus());
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("refreshModels failed", msg);
      setError(msg);
    }
  }, []);

  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  const flushSave = useCallback(async (next: AppSettings) => {
    try {
      await saveSettings(next);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logger.error("saveSettings failed", msg);
      setError(msg);
    }
  }, []);

  const patchSettings = useCallback(
    (patch: Partial<AppSettings>) => {
      const base = settingsRef.current ?? { ...EMPTY_SETTINGS };
      const next = { ...base, ...patch };
      setSettings(next);
      settingsRef.current = next;
      if (saveTimer.current) clearTimeout(saveTimer.current);
      saveTimer.current = setTimeout(() => {
        void flushSave(next);
      }, 320);
    },
    [flushSave],
  );

  useEffect(() => {
    return () => {
      if (saveTimer.current) clearTimeout(saveTimer.current);
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

  // Restore preferred devices once lists + settings are ready.
  useEffect(() => {
    if (!settings) return;
    if (inputs.length === 0 && outputs.length === 0) return;

    const pick = (list: DeviceDto[], preferred: string) => {
      if (preferred && list.some((d) => d.id === preferred)) return preferred;
      return list.find((d) => d.is_default)?.id ?? list[0]?.id ?? "";
    };

    const inputId = pick(inputs, settings.input_device);
    const outputId = pick(outputs, settings.output_device);

    if (inputId && inputId !== settings.input_device) {
      // Keep UI selection without thrashing disk on first match-to-default.
    }
    // Apply host preference if we have a stored id.
    if (settings.input_device && inputId === settings.input_device) {
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
    if (settings.output_device && outputId === settings.output_device) {
      void switchOutput(outputId).catch((e) => {
        logger.error("restore preferred output device failed", {
          deviceId: outputId,
          error: e instanceof Error ? e.message : String(e),
        });
      });
    }
  }, [settings, inputs, outputs]);

  const onStart = async () => {
    const s = settingsRef.current ?? settings;
    if (!s) return;
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
    const unsub = await onModelsProgress((p) => setInstallProgress(p));
    try {
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
            busy={busy}
            error={error}
            models={models}
            installing={installing}
            installProgress={installProgress}
            onStart={() => void onStart()}
            onStop={() => void onStop()}
            onInstall={() => void onInstallModels()}
            onOpenSettings={() => setView("settings")}
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
            installing={installing}
            installProgress={installProgress}
            advancedOpen={advancedOpen}
            logPath={logPath}
            capability={capability}
            onPatch={(p) => patchSettings(p)}
            onInputChange={(id) => void onInputChange(id)}
            onOutputChange={(id) => void onOutputChange(id)}
            onRefreshDevices={() => void refreshDevices()}
            onRefreshModels={() => void refreshModels()}
            onInstall={() => void onInstallModels()}
            onToggleAdvanced={() => setAdvancedOpen((v) => !v)}
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
  busy,
  error,
  models,
  installing,
  installProgress,
  onStart,
  onStop,
  onInstall,
  onOpenSettings,
}: {
  status: StatusPicture;
  tone: ReturnType<typeof toneFor>;
  contextMeter: string | null;
  engineOn: boolean;
  engineFault: boolean;
  modelsReady: boolean;
  busy: boolean;
  error: string | null;
  models: ModelsStatus | null;
  installing: boolean;
  installProgress: DownloadProgress | null;
  onStart: () => void;
  onStop: () => void;
  onInstall: () => void;
  onOpenSettings: () => void;
}) {
  const act = humanizeActivity(status.activity);
  const showActivity =
    act &&
    (status.phase === "Thinking" || status.phase === "AwaitingConfirm");

  return (
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-6 px-5 py-6">
      {/* Status strip — no glow card, no orb */}
      <section className="flex items-start justify-between gap-4">
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
            {status.turn ? (
              <span className="text-[12px] tabular-nums text-white/30">
                #{status.turn}
              </span>
            ) : null}
            {contextMeter && engineOn ? (
              <span className="text-[12px] tabular-nums text-white/25">
                {contextMeter}
              </span>
            ) : null}
          </div>
          <p className="mt-1 pl-[18px] text-[13px] leading-snug text-white/45">
            {tone.hint}
          </p>
          {showActivity ? (
            <p className="mt-1.5 pl-[18px] text-[12px] text-white/50">{act}</p>
          ) : null}
          {status.detail ? (
            <p className="mt-2 pl-[18px] text-[13px] text-red-300/90">
              {status.detail}
            </p>
          ) : null}
        </div>

        <div className="flex shrink-0 gap-2">
          <Button
            type="button"
            size="lg"
            disabled={busy || engineOn || !modelsReady}
            onClick={onStart}
            className={cn(
              "h-10 gap-2 rounded-full px-5 text-[13px] font-semibold",
              "bg-white text-[#0b0b0c] hover:bg-white/90",
              "disabled:bg-white/15 disabled:text-white/35",
            )}
            title={
              modelsReady
                ? undefined
                : "Download speech models before starting"
            }
          >
            <Power className="size-3.5" strokeWidth={2.25} />
            Start
          </Button>
          <Button
            type="button"
            size="lg"
            variant="outline"
            disabled={busy || status.engine === "Off"}
            onClick={onStop}
            className={cn(
              "h-10 gap-2 rounded-full border-white/10 bg-transparent px-4 text-[13px] font-medium text-white/70",
              "hover:bg-white/[0.06] hover:text-white",
              "disabled:opacity-30",
            )}
          >
            <PowerOff className="size-3.5" strokeWidth={2} />
            Stop
          </Button>
        </div>
      </section>

      {error ? (
        <p className="rounded-xl bg-red-500/10 px-3.5 py-2.5 text-[13px] text-red-300 ring-1 ring-red-500/15">
          {error}
        </p>
      ) : null}
      {engineFault ? (
        <p className="text-[13px] text-amber-200/80">
          Something went wrong — try Stop, then Start again.
        </p>
      ) : null}

      {!modelsReady ? (
        <ModelsBanner
          models={models}
          installing={installing}
          progress={installProgress}
          onInstall={onInstall}
          onOpenSettings={onOpenSettings}
        />
      ) : null}

      <ConversationView status={status} />
    </div>
  );
}

function ModelsBanner({
  models,
  installing,
  progress,
  onInstall,
  onOpenSettings,
}: {
  models: ModelsStatus | null;
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
          <div className="main-progress-track">
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
    <section className="flex min-h-[160px] flex-col gap-4">
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
  settings,
  engineOn,
  busy,
  inputs,
  outputs,
  selectedInput,
  selectedOutput,
  modelsReady,
  installing,
  installProgress,
  advancedOpen,
  logPath,
  capability,
  onPatch,
  onInputChange,
  onOutputChange,
  onRefreshDevices,
  onRefreshModels,
  onInstall,
  onToggleAdvanced,
}: {
  settings: AppSettings;
  engineOn: boolean;
  busy: boolean;
  inputs: DeviceDto[];
  outputs: DeviceDto[];
  selectedInput: string;
  selectedOutput: string;
  modelsReady: boolean;
  installing: boolean;
  installProgress: DownloadProgress | null;
  advancedOpen: boolean;
  logPath: string;
  capability: (typeof CAPABILITY_OPTIONS)[number];
  onPatch: (p: Partial<AppSettings>) => void;
  onInputChange: (id: string) => void;
  onOutputChange: (id: string) => void;
  onRefreshDevices: () => void;
  onRefreshModels: () => void;
  onInstall: () => void;
  onToggleAdvanced: () => void;
}) {
  const locked = engineOn;

  return (
    <div className="mx-auto flex w-full max-w-xl flex-col gap-7 px-5 py-6 pb-12">
      <h1 className="text-[28px] font-bold tracking-[-0.03em] text-white">
        Settings
      </h1>

      {/* General */}
      <SettingsGroup
        title="General"
        footer={
          locked
            ? "Some options apply on next Start."
            : capability.footer
        }
      >
        <SettingsRow label="Start engine on launch">
          <Toggle
            checked={settings.start_engine_on_launch}
            onChange={(v) => onPatch({ start_engine_on_launch: v })}
            aria-label="Start engine on launch"
          />
        </SettingsRow>
        <SettingsRow label="Show overlay on wake">
          <Toggle
            checked={settings.show_overlay_on_wake}
            onChange={(v) => onPatch({ show_overlay_on_wake: v })}
            aria-label="Show overlay on wake"
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
        <SettingsRow
          label="Trusted mode"
          subtitle="Auto-allow notes and sandbox file writes. Shell and open URL still need yes. Confirm budget defaults to 12 per turn."
        >
          <Toggle
            checked={settings.trusted_auto_moderate}
            disabled={locked}
            onChange={(v) => onPatch({ trusted_auto_moderate: v })}
            aria-label="Trusted mode"
          />
        </SettingsRow>
        <SettingsRow label="Tools" last>
          <select
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

      {/* Speech */}
      <SettingsGroup
        title="Speech"
        footer="Voice applies on next Start. Models install under ~/.boris/models."
      >
        <SettingsRow label="Voice">
          <select
            className={selectCompactClass}
            value={settings.tts_voice_id || "M4"}
            disabled={locked}
            onChange={(e) => onPatch({ tts_voice_id: e.target.value })}
          >
            <option value="M4" className="bg-[#1c1c1e] text-white">
              M4
            </option>
          </select>
        </SettingsRow>
        <SettingsRow
          label="Speech models"
          subtitle={
            modelsReady
              ? "Ready"
              : installing
                ? "Downloading…"
                : "Not installed"
          }
          last={modelsReady && !installing}
        >
          {modelsReady ? (
            <button
              type="button"
              onClick={onRefreshModels}
              className="text-[13px] text-white/40 hover:text-white/70"
            >
              Refresh
            </button>
          ) : (
            <button
              type="button"
              disabled={installing}
              onClick={onInstall}
              className="text-[13px] font-medium text-white/80 hover:text-white disabled:opacity-40"
            >
              {installing ? "…" : "Download"}
            </button>
          )}
        </SettingsRow>
        {installing && installProgress ? (
          <div className="border-t border-white/[0.06] px-4 py-3">
            <div className="main-progress-track">
              <div
                className="main-progress-fill"
                style={{
                  width: `${
                    installProgress.total_bytes
                      ? Math.min(
                          100,
                          Math.round(
                            (installProgress.bytes_downloaded /
                              installProgress.total_bytes) *
                              100,
                          ),
                        )
                      : 4
                  }%`,
                }}
              />
            </div>
            <p className="mt-1.5 truncate text-[12px] text-white/40">
              {installProgress.file_name}
            </p>
          </div>
        ) : null}
      </SettingsGroup>

      {/* API keys — secrets in ~/.boris/auth.json */}
      <SettingsGroup
        title="API keys"
        footer={
          locked
            ? "Stop the engine to change API keys."
            : "Stored only in ~/.boris/auth.json on this PC. Never shared."
        }
      >
        <SettingsField
          label="OpenRouter"
          subtitle="Required for chat. openrouter.ai/keys"
        >
          <Input
            type="password"
            placeholder="sk-or-v1-…"
            value={settings.openrouter_api_key}
            disabled={locked}
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => onPatch({ openrouter_api_key: e.target.value })}
            className={fieldInputClass}
          />
        </SettingsField>
        <SettingsField
          label="Exa (web search)"
          subtitle="Recommended for reliable web_search. Free tier at dashboard.exa.ai"
          last
        >
          <Input
            type="password"
            placeholder="Exa API key"
            value={settings.exa_api_key}
            disabled={locked}
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => onPatch({ exa_api_key: e.target.value })}
            className={fieldInputClass}
          />
        </SettingsField>
      </SettingsGroup>

      {/* Models */}
      <SettingsGroup
        title="Models"
        footer={locked ? "Stop the engine to change models." : undefined}
      >
        <ModelField
          label="Model"
          value={settings.openrouter_model}
          disabled={locked}
          onChange={(v) => onPatch({ openrouter_model: v })}
        />
        <ModelField
          label="Fast model"
          subtitle="Empty matches Model"
          value={settings.openrouter_fast_model}
          disabled={locked}
          onChange={(v) => onPatch({ openrouter_fast_model: v })}
          allowEmpty
          last={!advancedOpen}
        />
        <button
          type="button"
          onClick={onToggleAdvanced}
          className={cn(
            "flex w-full min-h-[44px] items-center justify-between px-4 py-2.5 text-left",
            "border-t border-white/[0.06] text-[15px] text-white/80",
            "hover:bg-white/[0.03]",
          )}
        >
          <span>Advanced</span>
          <ChevronRight
            className={cn(
              "size-4 text-white/30 transition-transform",
              advancedOpen && "rotate-90",
            )}
          />
        </button>
        {advancedOpen ? (
          <>
            <ProviderField
              label="Inference host"
              value={settings.openrouter_model_provider}
              disabled={locked}
              onChange={(v) => onPatch({ openrouter_model_provider: v })}
            />
            <ProviderField
              label="Fast host"
              value={settings.openrouter_fast_provider}
              disabled={locked}
              onChange={(v) => onPatch({ openrouter_fast_provider: v })}
            />
            <SettingsRow
              label="Don’t fall back to other hosts"
              last
            >
              <Toggle
                checked={settings.openrouter_pin_provider}
                disabled={
                  locked ||
                  (!settings.openrouter_model_provider.trim() &&
                    !settings.openrouter_fast_provider.trim())
                }
                onChange={(v) => onPatch({ openrouter_pin_provider: v })}
                aria-label="Pin preferred host"
              />
            </SettingsRow>
          </>
        ) : null}
      </SettingsGroup>

      {/* Sound */}
      <SettingsGroup title="Sound">
        <SettingsRow
          label="Microphone"
          stacked
        >
          <div className="flex items-center gap-2">
            <select
              className={selectDeviceClass}
              value={selectedInput}
              disabled={busy || inputs.length === 0}
              title={
                inputs.find((d) => d.id === selectedInput)?.name ?? undefined
              }
              onChange={(e) => onInputChange(e.target.value)}
            >
              {inputs.length === 0 ? (
                <option value="">No devices</option>
              ) : (
                inputs.map((d) => (
                  <option
                    key={d.id}
                    value={d.id}
                    className="bg-[#1c1c1e] text-white"
                  >
                    {shortDeviceName(d.name)}
                    {d.is_default ? " · default" : ""}
                  </option>
                ))
              )}
            </select>
          </div>
        </SettingsRow>
        <SettingsRow label="Speakers" stacked last>
          <div className="flex items-center gap-2">
            <select
              className={selectDeviceClass}
              value={selectedOutput}
              disabled={busy || outputs.length === 0}
              title={
                outputs.find((d) => d.id === selectedOutput)?.name ?? undefined
              }
              onChange={(e) => onOutputChange(e.target.value)}
            >
              {outputs.length === 0 ? (
                <option value="">No devices</option>
              ) : (
                outputs.map((d) => (
                  <option
                    key={d.id}
                    value={d.id}
                    className="bg-[#1c1c1e] text-white"
                  >
                    {shortDeviceName(d.name)}
                    {d.is_default ? " · default" : ""}
                  </option>
                ))
              )}
            </select>
            <button
              type="button"
              disabled={busy}
              onClick={onRefreshDevices}
              className="inline-flex size-9 shrink-0 items-center justify-center rounded-lg bg-white/[0.06] text-white/50 hover:bg-white/[0.1] hover:text-white/80 disabled:opacity-40"
              aria-label="Refresh devices"
            >
              <RefreshCw className="size-3.5" />
            </button>
          </div>
        </SettingsRow>
      </SettingsGroup>

      {/* Diagnostics */}
      <SettingsGroup title="Diagnostics" footer="Log filter applies after restart.">
        <SettingsField label="Log filter">
          <Input
            placeholder="info  or  boris=debug"
            value={settings.logging_filter}
            onChange={(e) => onPatch({ logging_filter: e.target.value })}
            className={cn(fieldInputClass, "font-mono text-[13px]")}
          />
        </SettingsField>
        {logPath ? (
          <div className="px-4 py-3">
            <p className="text-[13px] text-white/45">Log file</p>
            <p
              className="mt-1 break-all font-mono text-[11px] leading-snug text-white/40"
              title={logPath}
            >
              {logPath.replace(/\\/g, "/")}
            </p>
          </div>
        ) : null}
      </SettingsGroup>
    </div>
  );
}

/** Model picker: preset trailing, custom id full-width under the row. */
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

  useEffect(() => {
    if (match) setForceCustom(false);
    else if (isCustomValue) setForceCustom(true);
  }, [match, isCustomValue]);

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
      <SettingsRow label={label} subtitle={subtitle} last={last && !showCustom}>
        <select
          className={selectCompactClass}
          disabled={disabled}
          value={selectValue}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "__custom__") {
              setForceCustom(true);
              // Clear preset text so the custom field starts empty for typing.
              if (match) onChange("");
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
            value={value}
            disabled={disabled}
            placeholder="provider/model-id"
            onChange={(e) => {
              setForceCustom(true);
              onChange(e.target.value);
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

  useEffect(() => {
    if (match) setForceCustom(false);
    else if (isCustomValue) setForceCustom(true);
  }, [match, isCustomValue]);

  const inCustomMode = forceCustom || isCustomValue;
  const selectValue = inCustomMode ? "__custom__" : match ? first : "";
  const showCustom = selectValue === "__custom__";

  return (
    <>
      <SettingsRow label={label} last={last && !showCustom}>
        <select
          className={selectCompactClass}
          disabled={disabled}
          value={selectValue}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "__custom__") {
              setForceCustom(true);
              if (match) onChange("");
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
            value={value}
            disabled={disabled}
            placeholder="coreweave,baseten"
            onChange={(e) => {
              setForceCustom(true);
              onChange(e.target.value);
            }}
            className={cn(fieldInputClass, "h-9 w-full font-mono text-[13px]")}
          />
        </div>
      ) : null}
    </>
  );
}
