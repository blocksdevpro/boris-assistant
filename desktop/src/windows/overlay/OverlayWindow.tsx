import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { OverlayArtifactCard } from "@/components/artifacts";
import {
  EVENTS,
  getSettings,
  useStatus,
  type AppSettings,
  type StatusPicture,
} from "@/bridge";
import { isTauriRuntime } from "@/lib/runtime";
import { cn } from "@/lib/utils";
import { toneFor } from "@/lib/phaseVisual";
import {
  isConfirmContext,
  overlayStageMode,
  overlayThinkingText,
  pickCaption,
  pickOverlayPresence,
  shouldShowOverlayCard,
  type Caption,
} from "@/lib/statusPresentation";
import {
  CaptionBody,
  deviceFaultKey,
  DeviceFaultBadge,
  OverlayThoughts,
} from "./OverlayPrimitives";
import {
  PresenceIndicator,
  presenceStateFromStatus,
} from "@/components/presence";

const soft = [0.22, 1, 0.36, 1] as const;
const islandSpring = {
  type: "spring",
  stiffness: 460,
  damping: 42,
  mass: 0.78,
} as const;
const READY_CAPTION_MS = 5_000;
const OVERLAY_PREFERENCES_EVENT = "overlay-preferences";
const OVERLAY_WILL_HIDE_EVENT = "overlay-will-hide";

type OverlayPreferences = Pick<
  AppSettings,
  "overlay_caption_mode" | "overlay_scale_percent"
>;

type OverlayPreferencesEvent = {
  captionMode: AppSettings["overlay_caption_mode"];
  scalePercent: number;
};

const DEFAULT_PREFERENCES: OverlayPreferences = {
  overlay_caption_mode: "full",
  overlay_scale_percent: 100,
};

function initialOverlayPreferences(): OverlayPreferences {
  if (!import.meta.env.DEV || isTauriRuntime()) return DEFAULT_PREFERENCES;
  const params = new URLSearchParams(window.location.search);
  const requestedMode = params.get("captions");
  const requestedScale = Number(params.get("scale"));
  const overlay_caption_mode =
    requestedMode === "hidden" ||
    requestedMode === "assistant" ||
    requestedMode === "full"
      ? requestedMode
      : DEFAULT_PREFERENCES.overlay_caption_mode;
  const overlay_scale_percent = Number.isFinite(requestedScale)
    ? Math.min(125, Math.max(75, requestedScale))
    : DEFAULT_PREFERENCES.overlay_scale_percent;
  return { overlay_caption_mode, overlay_scale_percent };
}

/** Always-on-top, click-through voice presence. */
export function OverlayWindow() {
  const status = useStatus();
  const tauriRuntime = isTauriRuntime();
  const reduceMotion = Boolean(useReducedMotion());
  const [preferences, setPreferences] =
    useState<OverlayPreferences>(initialOverlayPreferences);
  const [preferencesReady, setPreferencesReady] = useState(
    () => !isTauriRuntime(),
  );
  const [readyCaptionHidden, setReadyCaptionHidden] = useState(false);
  const [hostHiding, setHostHiding] = useState(false);

  const tone = useMemo(
    () => toneFor(status.phase, status.engine),
    [status.phase, status.engine],
  );
  const { primary, secondary } = useMemo(
    () => pickOverlayPresence(status, tone.label, tone.hint),
    [status, tone.label, tone.hint],
  );
  const liveCaption = useMemo(() => pickCaption(status), [status]);
  const isReady =
    status.engine === "On" &&
    (status.phase === "Armed" || status.phase === "Quiet");

  const privateCaption = filterCaption(
    liveCaption,
    preferences.overlay_caption_mode,
  );
  const showCard = shouldShowOverlayCard(status);
  const thought = useRetainedThinking(status);
  const contentHidden = preferences.overlay_caption_mode === "hidden";
  const stageMode = contentHidden ? "presence" : overlayStageMode(status);
  const caption =
    isReady && readyCaptionHidden && !showCard ? null : privateCaption;
  const orbOnly = isReady && readyCaptionHidden && !showCard;
  const confirm = isConfirmContext(status);
  const displayThought =
    contentHidden || showCard || liveCaption?.kind === "error" ? null : thought;
  const displayCard = !contentHidden && showCard;
  const faultKey = deviceFaultKey(status);
  const visualState =
    status.engine === "Fault"
      ? "fault"
      : confirm
        ? "confirm"
        : status.engine === "Starting"
          ? "starting"
          : status.phase.toLowerCase();
  const accessibleSummary = overlayAccessibleSummary({
    status,
    primary,
    secondary,
    // Confirmation context must remain available to assistive technology
    // even when visual captions are hidden for screen-share privacy.
    caption: confirm ? liveCaption : caption,
    faultKey,
  });
  const scale = Math.min(
    1.25,
    Math.max(0.75, preferences.overlay_scale_percent / 100),
  );
  const surfaceReady =
    preferencesReady &&
    (!tauriRuntime || status.engine !== "Off" || status.phase !== "Off");

  useEffect(() => {
    if (!tauriRuntime) return;
    let active = true;
    let unlisten = () => {};
    let receivedLivePreferences = false;

    void getSettings()
      .then((settings) => {
        if (!active || receivedLivePreferences) return;
        setPreferences({
          overlay_caption_mode: settings.overlay_caption_mode,
          overlay_scale_percent: settings.overlay_scale_percent,
        });
      })
      .catch(() => {
        // Browser fixtures have no native settings bridge.
      })
      .finally(() => {
        if (active) setPreferencesReady(true);
      });

    void listen<OverlayPreferencesEvent>(
      OVERLAY_PREFERENCES_EVENT,
      ({ payload }) => {
        if (!active) return;
        receivedLivePreferences = true;
        setPreferencesReady(true);
        setPreferences({
          overlay_caption_mode: payload.captionMode,
          overlay_scale_percent: payload.scalePercent,
        });
      },
    )
      .then((stop) => {
        unlisten = stop;
      })
      .catch(() => {
        // Browser fixtures have no native event bus.
      });

    return () => {
      active = false;
      unlisten();
    };
  }, [tauriRuntime]);

  useEffect(() => {
    if (!tauriRuntime) return;
    let active = true;
    let unlisten = () => {};
    void listen(OVERLAY_WILL_HIDE_EVENT, () => {
      if (active) setHostHiding(true);
    })
      .then((stop) => {
        unlisten = stop;
      })
      .catch(() => {
        // The native host owns this event; browser fixtures do not emit it.
      });
    return () => {
      active = false;
      unlisten();
    };
  }, [tauriRuntime]);

  // Any newer snapshot invalidates the host's generation-guarded hide timer.
  useEffect(() => {
    setHostHiding(false);
  }, [status]);

  // The host hides the window shortly after this. Collapsing first makes the
  // disappearance feel deliberate and fixes the former 9s + 12s timer chain.
  useEffect(() => {
    if (!isReady || liveCaption?.kind !== "said") {
      setReadyCaptionHidden(false);
      return;
    }
    setReadyCaptionHidden(false);
    const timer = window.setTimeout(
      () => setReadyCaptionHidden(true),
      READY_CAPTION_MS,
    );
    return () => window.clearTimeout(timer);
  }, [isReady, liveCaption?.kind, liveCaption?.text, status.turn]);

  useEffect(() => {
    document.documentElement.classList.add("overlay-mode");
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    const meta = document.querySelector('meta[name="color-scheme"]');
    const previous = meta?.getAttribute("content") ?? null;
    meta?.setAttribute("content", "only light");
    return () => {
      if (previous != null) meta?.setAttribute("content", previous);
    };
  }, []);

  // Host keeps the HWND off-screen until this fires. Two rAFs wait for the
  // island's first paint so show()+park does not flash a raw rectangle.
  useEffect(() => {
    if (!tauriRuntime || !surfaceReady) return;
    let cancelled = false;
    const id = window.requestAnimationFrame(() => {
      window.requestAnimationFrame(() => {
        if (cancelled) return;
        void emit(EVENTS.overlayReady).catch(() => {
          // Host fallback timer will show the island if this never arrives.
        });
      });
    });
    return () => {
      cancelled = true;
      window.cancelAnimationFrame(id);
    };
  }, [surfaceReady, tauriRuntime]);

  return (
    <div className="overlay-surface relative flex h-full w-full items-center justify-center bg-transparent">
      <div
        className="overlay-stage flex items-center justify-center bg-transparent"
        data-mode={stageMode}
        style={{
          ["--overlay-scale" as string]: scale,
          opacity: surfaceReady ? 1 : 0,
        } as CSSProperties}
      >
        <motion.div
          data-tauri-drag-region
          data-phase={status.phase.toLowerCase()}
          data-state={visualState}
          layout={reduceMotion ? false : true} // size + position, so the pill grows from its midpoint
          className={cn(
            "overlay-island overlay-island--premium relative flex select-none",
            orbOnly
              ? "items-center justify-center gap-2 px-3 py-2.5"
              : stageMode === "card" && displayCard
                ? "h-[264px] max-h-full w-full max-w-[360px] flex-col overflow-hidden px-3.5 py-3"
                : stageMode === "thought"
                  ? "max-h-full w-max min-w-[228px] max-w-[356px] flex-col overflow-hidden px-3.5 py-3"
                  : "w-max min-w-[228px] max-w-[356px] flex-col px-3.5 py-3",
          )}
          style={
            {
              ["--overlay-accent" as string]: tone.accent,
              ["--overlay-glow" as string]: tone.glow,
            } as CSSProperties
          }
          initial={reduceMotion ? false : { opacity: 0 }}
          animate={{
            opacity: hostHiding ? 0 : 1,
            borderRadius: 20,
          }}
          transition={
            reduceMotion
              ? { duration: 0 }
              : {
                  opacity: { duration: 0.18, ease: soft },
                  borderRadius: islandSpring,
                  layout: islandSpring,
                }
          }
          aria-hidden="true"
        >
          <AnimatePresence mode="popLayout" initial={false}>
            {orbOnly ? (
              <motion.div
                key="ready-chip"
                data-tauri-drag-region
                className="overlay-ready-orb flex items-center gap-2"
                initial={reduceMotion ? false : { opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={reduceMotion ? undefined : { opacity: 0 }}
                transition={{ duration: reduceMotion ? 0 : 0.22, ease: soft }}
              >
                <PresenceIndicator
                  state={presenceStateFromStatus(status)}
                  accent={tone.accent}
                  reducedMotion={reduceMotion}
                  size="md"
                />
                <span
                  data-tauri-drag-region
                  className="text-[13px] font-semibold tracking-[-0.025em] text-white/92"
                >
                  Ready
                </span>
              </motion.div>
            ) : (
              <motion.div
                key="overlay-details"
                data-tauri-drag-region
                className={cn(
                  "overlay-details flex min-h-0 w-full min-w-0 flex-col",
                  displayCard && "h-full",
                )}
                initial={reduceMotion ? false : { opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={reduceMotion ? undefined : { opacity: 0 }}
                transition={{ duration: reduceMotion ? 0 : 0.22, ease: soft }}
              >
                <div
                  data-tauri-drag-region
                  className="overlay-status-row flex min-h-8 items-center gap-2"
                >
                  <PresenceIndicator
                    state={presenceStateFromStatus(status)}
                    accent={tone.accent}
                    reducedMotion={reduceMotion}
                  />

                  <div
                    data-tauri-drag-region
                    className="flex min-w-0 flex-1 items-center"
                  >
                    <div
                      data-tauri-drag-region
                      className="flex min-w-0 flex-1 items-baseline gap-2"
                    >
                      <AnimatePresence mode="popLayout" initial={false}>
                        <motion.span
                          key={primary}
                          data-tauri-drag-region
                          className="overlay-status-primary shrink-0 truncate text-[13px] font-semibold leading-[1.15] tracking-[-0.025em] text-white/95"
                          initial={reduceMotion ? false : { opacity: 0 }}
                          animate={{ opacity: 1 }}
                          exit={reduceMotion ? undefined : { opacity: 0 }}
                          transition={{ duration: reduceMotion ? 0 : 0.14, ease: soft }}
                        >
                          {primary}
                        </motion.span>
                      </AnimatePresence>
                      <AnimatePresence mode="popLayout" initial={false}>
                        {secondary ? (
                          <motion.span
                            key="activity-detail"
                            data-tauri-drag-region
                            className="overlay-status-secondary min-w-0 flex-1 truncate text-[11px] font-medium leading-[1.15] tracking-[-0.01em] text-white/62"
                            initial={reduceMotion ? false : { opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={reduceMotion ? undefined : { opacity: 0 }}
                            transition={{ duration: reduceMotion ? 0 : 0.14, ease: soft }}
                          >
                            {secondary}
                          </motion.span>
                        ) : null}
                      </AnimatePresence>
                    </div>
                  </div>

                  <AnimatePresence mode="wait" initial={false}>
                    {faultKey ? (
                      <DeviceFaultBadge
                        key={faultKey}
                        status={status}
                        reducedMotion={reduceMotion}
                      />
                    ) : null}
                  </AnimatePresence>
                </div>

                <AnimatePresence mode="popLayout" initial={false}>
                  {displayCard && status.artifact ? (
                    <OverlayArtifactCard
                      key={status.artifact.id}
                      peek={status.artifact}
                    />
                  ) : caption ? (
                    <motion.div
                      key={caption.kind}
                      data-tauri-drag-region
                      className={cn(
                        "overlay-caption mt-2 min-w-0 max-w-[324px] overflow-hidden rounded-[12px] px-2.5 py-2",
                        confirm && "overlay-caption--confirm",
                        caption.kind === "error" && "overlay-caption--error",
                      )}
                      data-kind={caption.kind}
                      initial={reduceMotion ? false : { opacity: 0 }}
                      animate={{ opacity: 1 }}
                      exit={reduceMotion ? undefined : { opacity: 0 }}
                      transition={{ duration: reduceMotion ? 0 : 0.18, ease: soft }}
                    >
                      <CaptionBody
                        caption={caption}
                        expanded={confirm || caption.kind === "error"}
                      />
                    </motion.div>
                  ) : null}
                </AnimatePresence>

                <AnimatePresence initial={false}>
                  {displayThought ? (
                    <OverlayThoughts
                      key="thought"
                      text={displayThought}
                      reducedMotion={reduceMotion}
                    />
                  ) : null}
                </AnimatePresence>
              </motion.div>
            )}
          </AnimatePresence>
        </motion.div>
        <p
          className="sr-only"
          role={status.engine === "Fault" ? "alert" : "status"}
          aria-live={status.engine === "Fault" ? "assertive" : "polite"}
          aria-atomic="true"
        >
          {surfaceReady ? accessibleSummary : ""}
        </p>
        {surfaceReady && thought ? (
          <p className="sr-only" aria-live="off">
            Live reasoning: {thought}
          </p>
        ) : null}
      </div>
    </div>
  );
}

function filterCaption(
  caption: Caption | null,
  mode: AppSettings["overlay_caption_mode"],
): Caption | null {
  if (!caption || mode === "hidden") return null;
  if (mode === "assistant" && caption.kind === "heard") return null;
  return caption;
}

function overlayAccessibleSummary({
  status,
  primary,
  secondary,
  caption,
  faultKey,
}: {
  status: StatusPicture;
  primary: string;
  secondary: string;
  caption: Caption | null;
  faultKey: string | null;
}): string {
  const parts = [`Boris: ${primary}`];
  if (secondary) parts.push(secondary);
  if (faultKey === "audio") parts.push("Microphone and speaker unavailable");
  else if (faultKey === "mic") parts.push("Microphone unavailable");
  else if (faultKey === "speaker") parts.push("Speaker unavailable");
  if (caption) {
    const speaker =
      caption.kind === "heard"
        ? "You said"
        : caption.kind === "said"
          ? "Boris said"
          : "Error";
    parts.push(`${speaker}: ${caption.text}`);
  }
  if (status.artifact) parts.push(`Created card: ${status.artifact.title}`);
  return parts.join(". ");
}

/** Preserve the last streamed thought while a same-turn tool is running. */
function useRetainedThinking(status: StatusPicture): string | null {
  const retained = useRef<{
    turn: StatusPicture["turn"];
    text: string;
  } | null>(null);

  const live = status.thinking?.trim() || null;
  if (status.phase !== "Thinking" || status.said?.trim()) {
    retained.current = null;
  } else if (live) {
    retained.current = { turn: status.turn, text: live };
  }

  const remembered = retained.current;
  const thinking =
    live || (remembered && remembered.turn === status.turn ? remembered.text : null);
  return overlayThinkingText({ ...status, thinking });
}
