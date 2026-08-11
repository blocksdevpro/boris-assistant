import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { listen } from "@tauri-apps/api/event";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { Mic, Volume2 } from "lucide-react";
import { getSettings, useStatus, type AppSettings, type StatusPicture } from "@/bridge";
import { cn } from "@/lib/utils";
import { toneFor } from "@/lib/phaseVisual";
import {
  isConfirmContext,
  pickCaption,
  pickOverlayPresence,
  type Caption,
} from "@/lib/statusPresentation";

const soft = [0.22, 1, 0.36, 1] as const;
const READY_CAPTION_MS = 5_000;
const OVERLAY_PREFERENCES_EVENT = "overlay-preferences";

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

/** Always-on-top, click-through voice presence. */
export function OverlayWindow() {
  const status = useStatus();
  const reduceMotion = Boolean(useReducedMotion());
  const [preferences, setPreferences] =
    useState<OverlayPreferences>(DEFAULT_PREFERENCES);
  const [readyCaptionHidden, setReadyCaptionHidden] = useState(false);

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
  const caption = isReady && readyCaptionHidden ? null : privateCaption;
  const orbOnly = isReady && readyCaptionHidden;
  const confirm = isConfirmContext(status);
  const scale = Math.min(
    1.25,
    Math.max(0.75, preferences.overlay_scale_percent / 100),
  );

  useEffect(() => {
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
      });

    void listen<OverlayPreferencesEvent>(
      OVERLAY_PREFERENCES_EVENT,
      ({ payload }) => {
        if (!active) return;
        receivedLivePreferences = true;
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
  }, []);

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

  return (
    <div className="overlay-surface relative h-full w-full bg-transparent">
      <div
        className="overlay-stage absolute left-1/2 top-1/2 flex items-center justify-center bg-transparent p-3"
        style={{
          ["--overlay-scale" as string]: scale,
        } as CSSProperties}
      >
        {orbOnly ? (
          <motion.div
            data-tauri-drag-region
            className="overlay-island relative flex items-center justify-center rounded-full p-2.5"
            initial={reduceMotion ? false : { opacity: 0, scale: 0.92 }}
            animate={{ opacity: 1, scale: 1 }}
            transition={{ duration: reduceMotion ? 0 : 0.3, ease: soft }}
            role="status"
            aria-live="polite"
            aria-label="Boris is ready"
          >
            <PresenceOrb
              motion="none"
              accent={tone.accent}
              reducedMotion={reduceMotion}
              size="md"
            />
          </motion.div>
        ) : (
          <motion.div
            data-tauri-drag-region
            className={cn(
              "overlay-island relative flex w-max min-w-[220px] max-w-[356px] select-none flex-col rounded-[18px] px-3 py-2",
              confirm && "overlay-island--confirm",
            )}
            animate={{
              borderColor: confirm
                ? `color-mix(in oklch, ${tone.accent} 45%, rgba(255,255,255,0.12))`
                : "rgba(255,255,255,0.14)",
              boxShadow: reduceMotion
                ? "0 8px 24px rgba(0,0,0,0.38), inset 0 1px 0 rgba(255,255,255,0.09)"
                : `0 8px 24px rgba(0,0,0,0.38), inset 0 1px 0 rgba(255,255,255,0.09), 0 0 12px color-mix(in oklch, ${tone.glow} 55%, transparent)`,
            }}
            transition={{ duration: reduceMotion ? 0 : 0.32, ease: soft }}
            role="status"
            aria-live={status.engine === "Fault" ? "assertive" : "polite"}
            aria-atomic="true"
          >
            <div data-tauri-drag-region className="flex min-h-8 items-center gap-2">
              <PresenceOrb
                motion={tone.motion}
                accent={tone.accent}
                reducedMotion={reduceMotion}
              />

              <div
                data-tauri-drag-region
                className="flex min-w-0 flex-1 items-baseline gap-1.5"
              >
                <AnimatePresence mode="wait" initial={false}>
                  <motion.span
                    key={primary}
                    data-tauri-drag-region
                    className="shrink-0 text-[13px] font-semibold leading-tight tracking-[-0.02em] text-white"
                    initial={reduceMotion ? false : { opacity: 0, y: 1 }}
                    animate={{ opacity: 1, y: 0 }}
                    exit={reduceMotion ? undefined : { opacity: 0, y: -1 }}
                    transition={{ duration: reduceMotion ? 0 : 0.2, ease: soft }}
                  >
                    {primary}
                  </motion.span>
                </AnimatePresence>
                {secondary ? (
                  <span
                    data-tauri-drag-region
                    className="min-w-0 truncate text-[11px] leading-tight text-white/65"
                  >
                    · {secondary}
                  </span>
                ) : null}
              </div>

              <DeviceFaultBadge status={status} />
            </div>

            <AnimatePresence initial={false}>
              {caption ? (
                <motion.div
                  key={`${caption.kind}-${caption.text.slice(0, 32)}`}
                  data-tauri-drag-region
                  className={cn(
                    "overlay-caption mt-1.5 min-w-0 max-w-[324px] overflow-hidden rounded-[10px] px-2 py-1.5",
                    confirm && "overlay-caption--confirm",
                  )}
                  initial={reduceMotion ? false : { opacity: 0, height: 0 }}
                  animate={{ opacity: 1, height: "auto" }}
                  exit={reduceMotion ? undefined : { opacity: 0, height: 0 }}
                  transition={{ duration: reduceMotion ? 0 : 0.28, ease: soft }}
                  role={caption.kind === "error" ? "alert" : undefined}
                >
                  <CaptionBody caption={caption} />
                </motion.div>
              ) : null}
            </AnimatePresence>
          </motion.div>
        )}
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

function CaptionBody({ caption }: { caption: Caption }) {
  const color =
    caption.kind === "error"
      ? "text-red-200"
      : caption.kind === "said"
        ? "text-white/90"
        : "text-white/80";

  return (
    <p
      data-tauri-drag-region
      className={cn(
        "line-clamp-1 text-[12px] leading-[1.35] tracking-[-0.01em]",
        color,
      )}
    >
      {caption.kind !== "error" ? (
        <span className="mr-1.5 text-[10px] font-semibold uppercase tracking-[0.06em] text-white/60">
          {caption.kind === "said" ? "Boris" : "You"}
        </span>
      ) : null}
      {caption.text}
    </p>
  );
}

function PresenceOrb({
  motion: motionKind,
  accent,
  reducedMotion,
  size = "sm",
}: {
  motion: ReturnType<typeof toneFor>["motion"];
  accent: string;
  reducedMotion: boolean;
  size?: "sm" | "md";
}) {
  const box = size === "md" ? "size-9" : "size-8";
  const core = size === "md" ? "size-3" : "size-2.5";
  const effectiveMotion = reducedMotion ? "none" : motionKind;

  return (
    <div
      data-tauri-drag-region
      className={cn("relative flex shrink-0 items-center justify-center", box)}
      aria-hidden="true"
    >
      {effectiveMotion !== "none" ? (
        <span
          className={cn(
            "absolute inset-0 rounded-full border",
            effectiveMotion === "listen" && "overlay-ring-listen",
            effectiveMotion === "think" && "overlay-ring-think",
            effectiveMotion === "speak" && "overlay-ring-speak",
            effectiveMotion === "breathe" && "overlay-ring-breathe",
          )}
          style={{ borderColor: accent }}
        />
      ) : null}
      <span
        className={cn(
          "absolute rounded-full",
          core,
          effectiveMotion === "listen" && "overlay-core-listen",
          effectiveMotion === "think" && "overlay-core-think",
          effectiveMotion === "speak" && "overlay-core-speak",
          effectiveMotion === "breathe" && "overlay-core-breathe",
        )}
        style={{
          backgroundColor: accent,
          boxShadow: `0 0 10px color-mix(in oklch, ${accent} 55%, transparent)`,
        }}
      />
    </div>
  );
}

/** Visible labels are required because the locked overlay cannot be hovered. */
function DeviceFaultBadge({ status }: { status: StatusPicture }) {
  if (status.engine === "Off") return null;
  const micBad = !status.mic.ok;
  const speakerBad = !status.speaker.ok;
  if (!micBad && !speakerBad) return null;

  const both = micBad && speakerBad;
  const label = both ? "Audio" : micBad ? "Mic" : "Speaker";
  const detail = both
    ? `${status.mic.label}; ${status.speaker.label}`
    : micBad
      ? status.mic.label
      : status.speaker.label;

  return (
    <span
      data-tauri-drag-region
      title={detail}
      className="flex h-6 shrink-0 items-center gap-1 rounded-full border border-amber-300/35 bg-amber-300/12 px-1.5 text-[10px] font-semibold text-amber-100"
      aria-label={`${label} unavailable: ${detail}`}
    >
      {speakerBad && !micBad ? (
        <Volume2 className="size-3" strokeWidth={2} aria-hidden="true" />
      ) : (
        <Mic className="size-3" strokeWidth={2} aria-hidden="true" />
      )}
      {label} unavailable
    </span>
  );
}
