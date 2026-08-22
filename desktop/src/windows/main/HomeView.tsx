import { useEffect, useRef } from "react";
import {
  AlertCircle,
  Download,
  LoaderCircle,
  MessageCircle,
  Power,
  PowerOff,
} from "lucide-react";
import { motion, useReducedMotion } from "framer-motion";
import { SessionArtifactDesk } from "@/components/artifacts";
import { Button } from "@/components/ui/button";
import type {
  DownloadProgress,
  ModelsStatus,
  StatusPicture,
} from "@/bridge";
import { GridPresence, presenceStateFromStatus } from "@/components/presence";
import { toneFor } from "@/lib/phaseVisual";
import {
  conversationLines,
  humanizeActivity,
} from "@/lib/statusPresentation";
import type {
  AvailableUpdate,
  UpdateProgress,
} from "@/lib/updater";
import { cn } from "@/lib/utils";
import type {
  ModelCheckState,
  SettingsCategory,
  UpdateUiState,
} from "./mainWindowShared";

export function HomeView({
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
  const reduceMotion = useReducedMotion();
  const act = humanizeActivity(status.activity);
  const showActivity =
    act && (status.phase === "Thinking" || status.phase === "AwaitingConfirm");
  const stopAvailable = engineOn || engineFault;
  const showUpdateBanner =
    availableUpdate != null &&
    !updateBannerDismissed &&
    (updateUi === "available" || updateUi === "downloading");

  return (
    <div className="home-view mx-auto flex min-h-full w-full max-w-4xl flex-col gap-5 px-6 py-7 sm:px-8 sm:py-8">
      <section
        aria-live="polite"
        aria-atomic="true"
        className="home-status-panel relative overflow-hidden rounded-[22px] border border-white/[0.065] bg-white/[0.04] px-5 py-5 shadow-[0_1px_0_rgba(255,255,255,0.035)_inset,0_20px_55px_rgba(0,0,0,0.14)] sm:px-6"
      >
        <div
          className="pointer-events-none absolute -right-16 -top-24 size-64 rounded-full opacity-[0.08] blur-3xl"
          style={{ background: tone.accent }}
          aria-hidden
        />
        <div className="relative flex items-start justify-between gap-5">
          <div className="min-w-0 flex-1">
            <p className="mb-2.5 text-[10px] font-semibold uppercase tracking-[0.12em] text-white/30">
              Assistant status
            </p>
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
              <GridPresence
                state={presenceStateFromStatus(status)}
                accent={tone.accent}
                reducedMotion={Boolean(reduceMotion)}
                size="md"
              />
              <h1 className="text-[25px] font-semibold tracking-[-0.035em] text-white sm:text-[27px]">
                {tone.label}
              </h1>
            </div>
            <p className="mt-1.5 text-[13px] leading-relaxed text-white/46">
              {tone.hint}
            </p>
            {showActivity ? (
              <motion.p
                key={act}
                initial={reduceMotion ? false : { opacity: 0, y: 3 }}
                animate={{ opacity: 1, y: 0 }}
                className="mt-2 text-[12px] font-medium text-white/58"
              >
                {act}
              </motion.p>
            ) : null}
            {engineOn && (status.turn || contextMeter) ? (
              <p className="mt-2.5 text-[11px] text-white/29">
                {status.turn ? `Current turn ${status.turn}` : ""}
                {status.turn && contextMeter ? " · " : ""}
                {contextMeter ? `Context ${contextMeter}` : ""}
              </p>
            ) : null}
            {status.phase === "Thinking" && status.thinking?.trim() ? (
              <p className="mt-2 line-clamp-2 text-[12px] leading-relaxed text-white/42">
                {status.thinking.trim()}
              </p>
            ) : null}
          </div>

          <div className="flex shrink-0 gap-2 pt-1">
            <Button
              type="button"
              size="lg"
              disabled={busy}
              onClick={stopAvailable ? onStop : onStart}
              className={cn(
                "h-10 gap-2 rounded-full px-5 text-[13px] font-semibold shadow-sm",
                "transition-[background-color,color,box-shadow,transform] duration-200 ease-[cubic-bezier(0.22,1,0.36,1)] active:scale-[0.97]",
                stopAvailable
                  ? "border border-white/10 bg-white/[0.035] text-white/72 shadow-none hover:bg-white/[0.08] hover:text-white"
                  : "border border-white/90 bg-white text-[#0b0b0c] shadow-[0_8px_24px_rgba(0,0,0,0.2)] hover:bg-white/92 hover:shadow-[0_10px_28px_rgba(0,0,0,0.26)]",
                "disabled:bg-white/15 disabled:text-white/35 disabled:shadow-none",
              )}
              title={
                !stopAvailable && !modelsReady
                  ? "Open Speech settings to finish setup"
                  : undefined
              }
            >
              {busy ? (
                <LoaderCircle
                  className="size-3.5 animate-spin"
                  strokeWidth={2.25}
                />
              ) : stopAvailable ? (
                <PowerOff className="size-3.5" strokeWidth={2} />
              ) : (
                <Power className="size-3.5" strokeWidth={2.25} />
              )}
              {busy
                ? stopAvailable
                  ? "Stopping…"
                  : "Starting…"
                : engineFault
                  ? "Reset"
                  : engineOn
                    ? "Stop"
                    : "Start"}
            </Button>
          </div>
        </div>
      </section>

      {error ? (
        <p
          role="alert"
          className="flex items-start gap-2.5 rounded-[14px] border border-red-400/10 bg-red-500/[0.08] px-4 py-3 text-[13px] leading-relaxed text-red-200/85 shadow-[0_12px_32px_rgba(0,0,0,0.1)]"
        >
          <AlertCircle className="mt-0.5 size-4 shrink-0 text-red-300/80" />
          <span>{error}</span>
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
      <SessionArtifactDesk peek={status.artifact} engineOn={engineOn} />
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
      <div
        role="status"
        className="settings-group flex items-center gap-3 rounded-[12px] px-4 py-3.5 text-[13px] text-white/55"
      >
        <LoaderCircle className="size-4 animate-spin" />
        Checking speech models…
      </div>
    );
  }

  if (state === "error" && !installing) {
    return (
      <div className="settings-group rounded-[12px] px-4 py-3.5">
        <p className="text-[15px] font-medium text-white/90">
          Speech model check failed
        </p>
        <p className="mt-1 text-[12px] text-white/50">
          Open Speech settings to retry. No download has started.
        </p>
        <button
          type="button"
          onClick={onOpenSettings}
          className="mt-3 min-h-9 rounded-full bg-white/10 px-3.5 text-[13px] text-white/80 hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
        >
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

function LiveThoughts({
  text,
  compact = false,
  className,
}: {
  text: string;
  compact?: boolean;
  className?: string;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = scroller.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [text]);

  return (
    <div
      ref={scroller}
      className={cn(
        "overflow-y-auto rounded-[12px] bg-white/[0.035] px-3.5 py-2.5 ring-1 ring-white/[0.05]",
        compact ? "max-h-[6.5rem]" : "max-h-[9.5rem]",
        className,
      )}
    >
      <p className="text-[10px] font-medium uppercase tracking-[0.08em] text-white/28">
        Thinking
      </p>
      <p
        className={cn(
          "mt-1 whitespace-pre-wrap text-white/52",
          compact ? "text-[12px] leading-snug" : "text-[13px] leading-relaxed",
        )}
      >
        {text}
      </p>
    </div>
  );
}

function ConversationView({ status }: { status: StatusPicture }) {
  const lines = conversationLines(status);
  const reduceMotion = useReducedMotion();

  return (
    <section
      aria-labelledby="current-turn-heading"
      aria-live="polite"
      className="conversation-panel settings-group flex min-h-[240px] flex-col gap-4 rounded-[18px] border border-white/[0.055] px-5 py-4.5 shadow-[0_1px_0_rgba(255,255,255,0.025)_inset,0_18px_46px_rgba(0,0,0,0.1)]"
    >
      <div className="flex items-center justify-between gap-3">
        <h2
          id="current-turn-heading"
          className="text-[12px] font-medium tracking-[0.01em] text-white/45"
        >
          Current turn
        </h2>
        {status.phase === "Thinking" ? (
          <span className="inline-flex items-center gap-1.5 text-[10px] font-medium text-white/30">
            <span className="size-1.5 animate-pulse rounded-full bg-white/45" />
            Working
          </span>
        ) : null}
      </div>
      {lines.map((line, i) => {
        switch (line.kind) {
          case "placeholder":
            return (
              <motion.div
                key={`p-${i}`}
                initial={reduceMotion ? false : { opacity: 0, y: 4 }}
                animate={{ opacity: 1, y: 0 }}
                className="flex flex-1 flex-col items-center justify-center py-5 text-center"
              >
                <div className="flex size-11 items-center justify-center rounded-2xl border border-white/[0.055] bg-white/[0.035] text-white/28">
                  <MessageCircle className="size-[18px]" strokeWidth={1.6} />
                </div>
                <p className="mt-3 max-w-sm text-[14px] leading-relaxed text-white/32">
                  {line.text}
                </p>
              </motion.div>
            );
          case "error":
            return (
              <motion.p
                key={`e-${i}`}
                initial={reduceMotion ? false : { opacity: 0, y: 4 }}
                animate={{ opacity: 1, y: 0 }}
                className="rounded-[13px] border border-red-400/10 bg-red-500/[0.08] px-3.5 py-2.5 text-[13px] leading-relaxed text-red-200/85"
              >
                {line.text}
              </motion.p>
            );
          case "status":
            return (
              <p key={`s-${i}`} className="text-[13px] text-white/40">
                {line.text}
              </p>
            );
          case "thought":
            return <LiveThoughts key={`t-${i}`} text={line.text} />;
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
              <Bubble
                key={`y-${i}`}
                who="You"
                text={line.text}
                muted={line.muted}
              />
            );
          case "boris":
            return (
              <Bubble key={`b-${i}`} who="Boris" text={line.text} accent />
            );
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
