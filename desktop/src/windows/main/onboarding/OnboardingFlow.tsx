import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowRight, ChevronLeft, LoaderCircle } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion, type Variants } from "framer-motion";
import type { AppSettings, DeviceDto, DownloadProgress } from "@/bridge";
import { BorisOrb } from "@/components/presence";
import type { BorisOrbState } from "@/components/presence/orbState";
import { settleSpring, softEase } from "@/lib/mainMotion";
import { cn } from "@/lib/utils";
import {
  RESIDENCY_OPTIONS,
  recommendedResidency,
  type ModelCheckState,
  type Residency,
} from "../mainWindowShared";
import { useApiKeyCheck } from "../useApiKeyCheck";
import {
  ConnectStep,
  DevicesStep,
  ReadyStep,
  SpeedStep,
  WelcomeStep,
  type VoiceDownload,
} from "./OnboardingSteps";
import "./onboarding.css";

const STEPS = ["welcome", "connect", "speed", "devices", "ready"] as const;
type Step = (typeof STEPS)[number];

const COMPONENT_NAMES: Record<string, string> = {
  parakeet: "listening model",
  supertone: "Boris's voice",
  speaker: "voice filter",
};

/**
 * One-time first-run setup: welcome → key → speed/memory → mic → ready.
 * Owns no persistence itself; every choice goes through `onPatch` at once,
 * so quitting halfway keeps what was picked.
 */
export function OnboardingFlow({
  settings,
  inputs,
  outputs,
  selectedInput,
  selectedOutput,
  modelCheckState,
  installing,
  installProgress,
  installFilesDone,
  installFilesTotal,
  installError,
  onPatch,
  onInputChange,
  onOutputChange,
  onInstall,
  onOpenExternal,
  onFinish,
  onSkip,
}: {
  settings: AppSettings;
  inputs: DeviceDto[];
  outputs: DeviceDto[];
  selectedInput: string;
  selectedOutput: string;
  modelCheckState: ModelCheckState;
  installing: boolean;
  installProgress: DownloadProgress | null;
  installFilesDone: number;
  installFilesTotal: number;
  installError: string | null;
  onPatch: (p: Partial<AppSettings>) => void;
  onInputChange: (id: string) => void;
  onOutputChange: (id: string) => void;
  onInstall: () => void;
  onOpenExternal: (url: string) => void;
  onFinish: () => void;
  onSkip: () => void;
}) {
  const reduce = Boolean(useReducedMotion());
  const [stepIndex, setStepIndex] = useState(0);
  const [direction, setDirection] = useState(1);
  const [downloadAttempted, setDownloadAttempted] = useState(false);
  const [acceptUnverifiedKey, setAcceptUnverifiedKey] = useState(false);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const bestPercent = useRef(0);
  const recommended = useMemo(recommendedResidency, []);
  const step: Step = STEPS[stepIndex]!;
  const keyCheck = useApiKeyCheck(settings.openrouter_api_key);

  // Pre-select the best speed/memory mode once, unless already chosen before.
  const residencySeeded = useRef(false);
  useEffect(() => {
    if (residencySeeded.current) return;
    residencySeeded.current = true;
    if (!settings.onboarding_complete && settings.model_residency !== recommended) {
      onPatch({ model_residency: recommended });
    }
  }, [onPatch, recommended, settings.model_residency, settings.onboarding_complete]);

  // Fetch the voice in the background as soon as the person commits to setup.
  useEffect(() => {
    if (step === "welcome" || downloadAttempted) return;
    if (modelCheckState !== "missing" || installing) return;
    setDownloadAttempted(true);
    onInstall();
  }, [step, downloadAttempted, modelCheckState, installing, onInstall]);

  const download: VoiceDownload = useMemo(() => {
    if (modelCheckState === "ready" && !installing) return { state: "ready" };
    if (installing) {
      let percent: number | null = null;
      if (installFilesTotal > 0) {
        const p = installProgress;
        const current =
          p?.status === "downloading" && p.total_bytes ? p.bytes_downloaded / p.total_bytes : 0;
        const raw = ((installFilesDone + current) / installFilesTotal) * 100;
        bestPercent.current = Math.max(bestPercent.current, Math.min(99, Math.round(raw)));
        percent = bestPercent.current;
      }
      const name = installProgress ? COMPONENT_NAMES[installProgress.component] : null;
      return {
        state: "downloading",
        percent,
        label: name ? `Downloading ${name}…` : "Starting download…",
      };
    }
    if (modelCheckState === "error") {
      return { state: "failed", message: installError ?? "Boris couldn't check his speech files." };
    }
    if (modelCheckState === "missing" && downloadAttempted) {
      return { state: "failed", message: installError ?? "Some files didn't download." };
    }
    return { state: "checking" };
  }, [modelCheckState, installing, installFilesTotal, installFilesDone, installProgress, installError, downloadAttempted]);

  const keyUsable =
    keyCheck.state === "ok" ||
    keyCheck.state === "no_credit" ||
    keyCheck.state === "unreachable" ||
    (acceptUnverifiedKey && keyCheck.state === "rejected");

  const canContinue =
    step === "connect" ? keyUsable : step === "ready" ? download.state === "ready" : true;

  const go = useCallback(
    (delta: 1 | -1) => {
      setDirection(delta);
      setStepIndex((i) => Math.min(STEPS.length - 1, Math.max(0, i + delta)));
    },
    [],
  );

  const next = useCallback(() => {
    if (!canContinue) return;
    if (step === "ready") onFinish();
    else go(1);
  }, [canContinue, go, onFinish, step]);

  // Enter moves forward from anywhere except a focused button or list.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Enter" || event.repeat) return;
      const target = event.target as HTMLElement | null;
      if (target?.closest("button, select, textarea, a")) return;
      event.preventDefault();
      next();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [next]);

  // Move focus to each new heading so keyboard and screen-reader users follow along.
  useEffect(() => {
    const timer = window.setTimeout(
      () => headingRef.current?.focus({ preventScroll: true }),
      reduce ? 0 : 260,
    );
    return () => window.clearTimeout(timer);
  }, [stepIndex, reduce]);

  const bigOrb = step === "welcome" || (step === "ready" && download.state === "ready");
  const orbState: BorisOrbState =
    step === "connect"
      ? keyCheck.state === "checking"
        ? "thinking"
        : "ready"
      : step === "devices"
        ? "hearing"
        : step === "ready"
          ? download.state === "ready"
            ? "hearing"
            : download.state === "failed"
              ? "fault"
              : "starting"
          : "ready";
  const tone =
    (step === "connect" && keyCheck.state === "ok") || (step === "ready" && download.state === "ready")
      ? "success"
      : (step === "connect" && keyCheck.state === "rejected") || download.state === "failed"
        ? "warn"
        : undefined;

  const residency =
    RESIDENCY_OPTIONS.find((o) => o.id === settings.model_residency)?.id ?? "balanced";

  const pageVariants: Variants = reduce
    ? {
        enter: { opacity: 0 },
        center: { opacity: 1, transition: { duration: 0.12 } },
        exit: { opacity: 0, transition: { duration: 0.08 } },
      }
    : {
        enter: (d: number) => ({ opacity: 0, x: d * 44, filter: "blur(8px)" }),
        center: {
          opacity: 1,
          x: 0,
          filter: "blur(0px)",
          transition: { ...settleSpring, opacity: { duration: 0.32, ease: softEase }, filter: { duration: 0.36 } },
        },
        exit: (d: number) => ({
          opacity: 0,
          x: d * -28,
          filter: "blur(6px)",
          transition: { duration: 0.18, ease: softEase },
        }),
      };

  const primaryLabel =
    step === "welcome"
      ? "Get started"
      : step === "ready"
        ? download.state === "ready"
          ? "Start talking to Boris"
          : download.state === "failed"
            ? "Waiting for download"
            : "Downloading…"
        : step === "connect" && keyCheck.state === "checking"
          ? "Checking…"
          : "Continue";

  return (
    <div className="onboarding mx-auto w-full max-w-3xl px-6" data-tone={tone}>
      {/* Top: progress + skip */}
      <div className="flex h-12 shrink-0 items-center justify-between gap-4 pt-2">
        <div className="flex w-24 items-center">
          <AnimatePresence initial={false}>
            {stepIndex > 0 ? (
              <motion.button
                key="back"
                type="button"
                onClick={() => go(-1)}
                initial={{ opacity: 0, x: -6 }}
                animate={{ opacity: 1, x: 0 }}
                exit={{ opacity: 0, x: -6 }}
                transition={{ duration: reduce ? 0 : 0.2, ease: softEase }}
                className="inline-flex h-8 items-center gap-0.5 rounded-[9px] px-2 text-[12.5px] font-medium text-white/55 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/20"
              >
                <ChevronLeft className="size-4" /> Back
              </motion.button>
            ) : null}
          </AnimatePresence>
        </div>

        <div
          className="flex flex-1 justify-center gap-1.5"
          role="progressbar"
          aria-label="Setup progress"
          aria-valuemin={0}
          aria-valuemax={STEPS.length - 1}
          aria-valuenow={stepIndex}
        >
          {STEPS.slice(1).map((s, i) => (
            <span key={s} className="relative h-1 w-10 overflow-hidden rounded-full bg-white/[0.09]">
              <motion.span
                className="absolute inset-0 origin-left rounded-full bg-white/85"
                initial={false}
                animate={{ scaleX: stepIndex > i ? 1 : 0 }}
                transition={reduce ? { duration: 0 } : { duration: 0.45, ease: softEase }}
              />
            </span>
          ))}
        </div>

        <div className="flex w-24 justify-end">
          {!(step === "ready" && download.state === "ready") ? (
            <button
              type="button"
              onClick={onSkip}
              className="h-8 rounded-[9px] px-2 text-[12px] text-white/35 hover:bg-white/[0.05] hover:text-white/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/20"
            >
              Skip setup
            </button>
          ) : null}
        </div>
      </div>

      {/* The orb carries through every step and changes size and mood. */}
      <motion.div
        className="onboarding__stage"
        initial={false}
        animate={{ height: bigOrb ? 156 : 96 }}
        transition={reduce ? { duration: 0 } : settleSpring}
        aria-hidden
      >
        <span className="onboarding__halo" />
        <motion.div
          initial={reduce ? false : { scale: 0.6, opacity: 0 }}
          animate={{ scale: bigOrb ? 1.45 : 0.85, opacity: 1 }}
          transition={reduce ? { duration: 0 } : { ...settleSpring, opacity: { duration: 0.5 } }}
        >
          <BorisOrb state={orbState} size="xl" reducedMotion={reduce} />
        </motion.div>
      </motion.div>

      {/* Step body */}
      <div className="relative flex-1 pb-6 pt-2">
        <AnimatePresence mode="wait" initial={false} custom={direction}>
          <motion.section
            key={step}
            custom={direction}
            variants={pageVariants}
            initial="enter"
            animate="center"
            exit="exit"
            aria-label={`Setup step ${stepIndex + 1} of ${STEPS.length}`}
          >
            {step === "welcome" ? <WelcomeStep headingRef={headingRef} /> : null}
            {step === "connect" ? (
              <ConnectStep
                headingRef={headingRef}
                apiKey={settings.openrouter_api_key}
                keyCheck={keyCheck}
                onKeyChange={(key) => {
                  setAcceptUnverifiedKey(false);
                  onPatch({ openrouter_api_key: key.trim() });
                }}
                onOpenExternal={onOpenExternal}
              />
            ) : null}
            {step === "speed" ? (
              <SpeedStep
                headingRef={headingRef}
                value={residency as Residency}
                recommended={recommended}
                onChange={(id) => onPatch({ model_residency: id })}
              />
            ) : null}
            {step === "devices" ? (
              <DevicesStep
                headingRef={headingRef}
                inputs={inputs}
                outputs={outputs}
                selectedInput={selectedInput}
                selectedOutput={selectedOutput}
                onInputChange={onInputChange}
                onOutputChange={onOutputChange}
              />
            ) : null}
            {step === "ready" ? (
              <ReadyStep
                headingRef={headingRef}
                download={download}
                settings={settings}
                onPatch={onPatch}
                onRetry={() => {
                  bestPercent.current = 0;
                  onInstall();
                }}
              />
            ) : null}
          </motion.section>
        </AnimatePresence>
      </div>

      {/* Footer: one obvious next action. */}
      <div className="onboarding__footer flex flex-col items-center gap-2 pb-6 pt-6">
        <motion.button
          type="button"
          onClick={next}
          disabled={!canContinue}
          whileTap={reduce || !canContinue ? undefined : { scale: 0.97 }}
          className={cn(
            "inline-flex h-11 min-w-[13rem] items-center justify-center gap-2 rounded-full px-6 text-[14px] font-semibold",
            "transition-[background-color,color,box-shadow,opacity] duration-200",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40 focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--boris-surface-canvas)]",
            canContinue
              ? "bg-white text-[#0b0b0c] shadow-[0_10px_30px_rgba(0,0,0,0.3)] hover:bg-white/92"
              : "cursor-not-allowed bg-white/[0.1] text-white/40",
          )}
        >
          <AnimatePresence mode="wait" initial={false}>
            <motion.span
              key={primaryLabel}
              className="inline-flex items-center gap-2"
              initial={reduce ? { opacity: 0 } : { opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={reduce ? { opacity: 0 } : { opacity: 0, y: -6 }}
              transition={{ duration: reduce ? 0 : 0.16, ease: softEase }}
            >
              {primaryLabel === "Checking…" || primaryLabel === "Downloading…" ? (
                <LoaderCircle className="size-4 animate-spin" />
              ) : null}
              {primaryLabel}
              {canContinue && step !== "ready" ? <ArrowRight className="size-4" strokeWidth={2.4} /> : null}
            </motion.span>
          </AnimatePresence>
        </motion.button>
        <div className="min-h-6">
          {step === "connect" && keyCheck.state === "rejected" && !acceptUnverifiedKey ? (
            <button
              type="button"
              onClick={() => setAcceptUnverifiedKey(true)}
              className="text-[12px] text-white/40 underline-offset-2 hover:text-white/70 hover:underline"
            >
              Use this key anyway
            </button>
          ) : step === "ready" && download.state !== "ready" ? (
            <p className="text-[12px] text-[var(--boris-label-tertiary)]">
              You can keep this window open while it downloads.
            </p>
          ) : null}
        </div>
      </div>
    </div>
  );
}
