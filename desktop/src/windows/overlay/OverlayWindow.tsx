import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type RefObject } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { AnimatePresence, MotionConfig, animate, motion, useMotionValue, useReducedMotion, type MotionValue } from "framer-motion";
import { CollectInputField } from "@/components/CollectInputField";
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
  OVERLAY_MIN_DWELL_MS,
  overlayContentMotion,
  overlayExit,
  overlayFade,
  overlaySpring,
} from "@/lib/overlayMotion";
import { usePacedValue } from "@/lib/usePacedValue";
import { OverlayText } from "./OverlayText";
import {
  isConfirmContext,
  overlayInputUsesCard,
  overlayStageMode,
  overlayThinkingText,
  pickCaption,
  pickOverlayPresence,
  shouldShowOverlayCard,
  type Caption,
  type OverlayStageMode,
} from "@/lib/statusPresentation";
import {
  CaptionBody,
  deviceFaultKey,
  DeviceFaultBadge,
  OverlayThoughts,
} from "./OverlayPrimitives";
import {
  BorisOrb,
  orbStateFromStatus,
} from "@/components/presence";

const READY_CAPTION_MS = 5_000;
const OVERLAY_PREFERENCES_EVENT = "overlay-preferences";
const OVERLAY_WILL_HIDE_EVENT = "overlay-will-hide";

type OverlayPreferences = Pick<
  AppSettings,
  "overlay_caption_mode" | "overlay_scale_percent"
>;

export type OverlayDebugPreferences = OverlayPreferences;

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
  const requestedScale = params.has("scale") ? Number(params.get("scale")) : 100;
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
export function OverlayWindow({
  debugPreferences,
  debugReadyCaptionMs,
  debugReducedMotion,
}: {
  debugPreferences?: OverlayDebugPreferences;
  debugReadyCaptionMs?: number;
  debugReducedMotion?: boolean;
} = {}) {
  const liveStatus = useStatus();
  // The engine can hop Listening -> Transcribing -> Thinking -> Working inside
  // a few hundred ms. Rendering every hop is what reads as jank, so each
  // visual shape is held briefly; streamed text within a shape stays live.
  const status = usePacedValue(
    liveStatus,
    OVERLAY_MIN_DWELL_MS,
    overlayShapeKey,
    isUrgentStatus,
  );
  const tauriRuntime = isTauriRuntime();
  const reduceMotion = debugReducedMotion ?? Boolean(useReducedMotion());
  const [preferences, setPreferences] =
    useState<OverlayPreferences>(initialOverlayPreferences);
  const [preferencesReady, setPreferencesReady] = useState(
    () => !isTauriRuntime(),
  );
  const [readyCaptionHidden, setReadyCaptionHidden] = useState(false);
  const [hostHiding, setHostHiding] = useState(false);
  const activePreferences = debugPreferences ?? preferences;
  const readyCaptionDelay = debugReadyCaptionMs ?? READY_CAPTION_MS;

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
    activePreferences.overlay_caption_mode,
  );
  const showCard = shouldShowOverlayCard(status);
  const thought = useRetainedThinking(status);
  const contentHidden = activePreferences.overlay_caption_mode === "hidden";
  const stageMode = contentHidden ? "presence" : overlayStageMode(status);
  const caption =
    isReady && readyCaptionHidden && !showCard ? null : privateCaption;
  const orbOnly = isReady && readyCaptionHidden && !showCard;
  const confirm = isConfirmContext(status);
  const displayThought =
    contentHidden || showCard || status.input || liveCaption?.kind === "error"
      ? null
      : thought;
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
    Math.max(0.75, activePreferences.overlay_scale_percent / 100),
  );
  const surfaceReady =
    preferencesReady &&
    (!tauriRuntime || liveStatus.engine !== "Off" || liveStatus.phase !== "Off");
  const hidden = hostHiding || !surfaceReady;
  const islandRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const { width: islandWidth, height: islandHeight } = useIslandMorph(
    islandRef,
    contentRef,
    reduceMotion || hidden,
  );

  useEffect(() => {
    if (!tauriRuntime) return;
    let active = true;
    let unlisten = () => { };
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
    let unlisten = () => { };
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
  }, [liveStatus]);

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
      readyCaptionDelay,
    );
    return () => window.clearTimeout(timer);
  }, [isReady, liveCaption?.kind, liveCaption?.text, readyCaptionDelay, status.turn]);

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
    <MotionConfig reducedMotion={debugReducedMotion === undefined ? "user" : reduceMotion ? "always" : "never"} transition={reduceMotion ? { duration: 0 } : { ...overlaySpring, opacity: overlayFade }}>
      <div className="overlay-surface relative flex h-full w-full items-center justify-center bg-transparent">
        <div
          className="overlay-stage flex items-start justify-center bg-transparent"
          data-mode={stageMode}
          style={{
            ["--overlay-scale" as string]: scale,
            opacity: surfaceReady ? 1 : 0,
          } as CSSProperties}
        >
          <motion.div
            ref={islandRef}
            data-tauri-drag-region
            data-phase={status.phase.toLowerCase()}
            data-state={visualState}
            className="overlay-island overlay-island--premium relative flex select-none items-start justify-start overflow-hidden"
            style={{
              ...({
                ["--overlay-accent" as string]: tone.accent,
                ["--overlay-glow" as string]: tone.glow,
                transformOrigin: "50% 0%",
              } as CSSProperties),
              width: islandWidth,
              height: islandHeight,
            }}
            initial={reduceMotion ? false : { opacity: 0, scale: 0.92, y: -10, filter: "blur(6px)" }}
            animate={{
              opacity: hidden ? 0 : 1,
              scale: hidden && !reduceMotion ? 0.94 : 1,
              y: hidden && !reduceMotion ? -6 : 0,
              filter: hidden && !reduceMotion ? "blur(6px)" : "blur(0px)",
              borderRadius: orbOnly ? 24 : 20,
              transitionEnd: hidden ? undefined : { filter: "none" },
            }}
            transition={
              reduceMotion
                ? { duration: 0 }
                : hostHiding
                  ? overlayExit
                  : {
                    default: overlaySpring,
                    opacity: overlayFade,
                    filter: overlayFade,
                  }
            }
            aria-hidden={hidden ? true : undefined}
            inert={hidden}
          >
            {/* Content lays out at its final size immediately; the shell
                morphs its real width/height toward it (no transform scale, so
                border, shadow, blur and text never stretch) and clips while
                it travels. Content stays pinned to the shell's leading edge. */}
            <div
              ref={contentRef}
              data-tauri-drag-region
              className={cn(
                "overlay-island__content relative flex shrink-0",
                overlayContentClass({
                  orbOnly,
                  status,
                  stageMode,
                  displayCard,
                }),
              )}
            >
            <div
              data-tauri-drag-region
              className={cn(
                "overlay-details flex min-h-0 w-full min-w-0 flex-col",
                (displayCard || overlayInputUsesCard(status)) && "h-full",
              )}
            >
              <div
                data-tauri-drag-region
                className="overlay-status-row flex min-h-8 items-center gap-2"
              >
                <div className="flex shrink-0 items-center">
                  <BorisOrb
                    state={orbStateFromStatus(status)}
                    active={surfaceReady && !hostHiding}
                    accent={tone.accent}
                    reducedMotion={reduceMotion}
                  />
                </div>

                <div
                  data-tauri-drag-region
                  className="flex min-w-0 flex-1 items-center"
                >
                  <div
                    data-tauri-drag-region
                    className="relative flex min-w-0 flex-1 items-baseline gap-2"
                  >
                    <span
                      data-tauri-drag-region
                      className="overlay-status-primary shrink-0 truncate text-[13px] font-semibold leading-[1.15] tracking-[-0.025em] text-white/95"
                    >
                      <OverlayText text={orbOnly ? "Ready" : primary} />
                    </span>
                    <AnimatePresence mode="popLayout" initial={false}>
                      {!orbOnly && secondary ? (
                        <motion.span
                          key="activity-detail"
                          data-tauri-drag-region
                          className="overlay-status-secondary min-w-0 flex-1 truncate text-[11px] font-medium leading-[1.15] tracking-[-0.01em] text-white/62"
                          layout={reduceMotion ? false : "position"}
                          {...overlayContentMotion(reduceMotion)}
                        >
                          <OverlayText text={secondary} />
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

              {/* Exiting content leaves layout immediately so the shell
                  springs toward the compact size while the fade finishes
                  under its clip. */}
              <AnimatePresence mode="popLayout" initial={false}>
                {status.input ? (
                  <motion.div key={`input-${status.input.id}`} className="relative flex min-h-0 flex-1 flex-col" {...overlayContentMotion(reduceMotion)}>
                    <CollectInputField
                      input={status.input}
                      compact
                    />
                  </motion.div>
                ) : displayCard && status.artifact ? (
                  <motion.div key={`card-${status.artifact.id}`} className="relative flex min-h-0 flex-1 flex-col" {...overlayContentMotion(reduceMotion)}>
                    <OverlayArtifactCard
                      peek={status.artifact}
                      fallback={status.fallback_report}
                    />
                  </motion.div>
                ) : !orbOnly && caption ? (
                  <motion.div
                    key={`caption-${status.turn}-${caption.kind}`}
                    data-tauri-drag-region
                    className={cn(
                      "overlay-caption mt-2 min-w-0 max-w-[324px] overflow-hidden rounded-[12px] px-2.5 py-2",
                      confirm && "overlay-caption--confirm",
                      caption.kind === "error" && "overlay-caption--error",
                    )}
                    data-kind={caption.kind}
                    {...overlayContentMotion(reduceMotion)}
                  >
                    <CaptionBody
                      caption={caption}
                      expanded={confirm || caption.kind === "error"}
                    />
                  </motion.div>
                ) : null}
              </AnimatePresence>

              <AnimatePresence mode="popLayout" initial={false}>
                {!orbOnly && displayThought ? (
                  <motion.div key={`thought-${status.turn}`} className="relative min-w-0" {...overlayContentMotion(reduceMotion)}>
                    <OverlayThoughts
                      text={displayThought}
                      reducedMotion={reduceMotion}
                    />
                  </motion.div>
                ) : null}
              </AnimatePresence>
            </div>
            </div>
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
              Boris is working: {thought}
            </p>
          ) : null}
        </div>
      </div>
    </MotionConfig>
  );
}

function overlayContentClass({
  orbOnly,
  status,
  stageMode,
  displayCard,
}: {
  orbOnly: boolean;
  status: StatusPicture;
  stageMode: OverlayStageMode;
  displayCard: boolean;
}): string {
  // Sizes never depend on the shell (it is animating toward them), and are
  // 2px under the former island box so the 1px border lands on the same
  // outer geometry.
  if (orbOnly) return "w-max flex-col px-3 py-2";
  if (overlayInputUsesCard(status) || (stageMode === "card" && displayCard)) {
    return "h-[262px] max-h-[298px] w-[358px] flex-col overflow-hidden px-3.5 py-3";
  }
  if (status.input) {
    return "max-h-[298px] w-[358px] flex-col overflow-hidden px-3.5 py-3";
  }
  if (stageMode === "thought") {
    return "max-h-[298px] w-max min-w-[226px] max-w-[354px] flex-col overflow-hidden px-3.5 py-3";
  }
  return "w-max min-w-[226px] max-w-[354px] flex-col px-3.5 py-3";
}

/** What the overlay *looks like* — changes here are paced; text inside is not. */
function overlayShapeKey(status: StatusPicture): string {
  const tone = toneFor(status.phase, status.engine);
  const { primary } = pickOverlayPresence(status, tone.label, tone.hint);
  return [
    status.engine,
    status.phase,
    primary,
    pickCaption(status)?.kind ?? "-",
    status.input?.id ?? "-",
    shouldShowOverlayCard(status) ? (status.artifact?.id ?? "card") : "-",
    status.listen_hint ?? "-",
    orbStateFromStatus(status),
  ].join("|");
}

/** States the user must act on are never held back. */
function isUrgentStatus(status: StatusPicture): boolean {
  return (
    status.engine === "Fault" ||
    Boolean(status.input) ||
    isConfirmContext(status)
  );
}

/**
 * Drive the island shell's real width/height toward its content's natural
 * size with one interruptible spring. Retargeting mid-flight keeps velocity,
 * so a burst of size changes reads as one continuous morph.
 */
function useIslandMorph(
  island: RefObject<HTMLDivElement | null>,
  content: RefObject<HTMLDivElement | null>,
  instant: boolean,
): { width: MotionValue<number>; height: MotionValue<number> } {
  const width = useMotionValue(0);
  const height = useMotionValue(0);
  const target = useRef<{ w: number; h: number } | null>(null);
  const instantRef = useRef(instant);
  instantRef.current = instant;

  useLayoutEffect(() => {
    const shell = island.current;
    const inner = content.current;
    if (!shell || !inner) return;

    const measure = () => {
      const box = window.getComputedStyle(inner);
      const frame = window.getComputedStyle(shell);
      const w =
        parseFloat(box.width) +
        parseFloat(frame.borderLeftWidth) +
        parseFloat(frame.borderRightWidth);
      const h =
        parseFloat(box.height) +
        parseFloat(frame.borderTopWidth) +
        parseFloat(frame.borderBottomWidth);
      if (!Number.isFinite(w) || !Number.isFinite(h) || w <= 0 || h <= 0) return;
      const first = target.current === null;
      if (!first && target.current!.w === w && target.current!.h === h) return;
      target.current = { w, h };
      if (first || instantRef.current) {
        width.jump(w);
        height.jump(h);
        // Motion values render on the next frame; paint the first size now
        // so the island never shows a collapsed frame.
        shell.style.width = `${w}px`;
        shell.style.height = `${h}px`;
        return;
      }
      animate(width, w, overlaySpring);
      animate(height, h, overlaySpring);
    };

    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(inner);
    return () => {
      observer.disconnect();
      width.stop();
      height.stop();
    };
  }, [island, content, width, height]);

  // Hidden or reduced motion: settle immediately so the next reveal starts
  // at the right size instead of morphing in from a stale one.
  useEffect(() => {
    if (!instant || !target.current) return;
    width.jump(target.current.w);
    height.jump(target.current.h);
  }, [instant, width, height]);

  return { width, height };
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
  if (status.input) parts.push(`Needs typed input: ${status.input.label}`);
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
