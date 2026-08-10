import {
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { AnimatePresence, LayoutGroup, motion } from "framer-motion";
import { Mic, Volume2 } from "lucide-react";
import { useStatus, type StatusPicture } from "@/bridge";
import { cn } from "@/lib/utils";
import { toneFor } from "@/lib/phaseVisual";
import {
  canCollapseToOrb,
  isConfirmContext,
  pickCaption,
  pickOverlayPresence,
  showProgressBar,
  type Caption,
} from "@/lib/statusPresentation";

/** Soft ease-out — calm, no bounce. */
const soft = [0.22, 1, 0.36, 1] as const;
const softIdle = [0.16, 1, 0.3, 1] as const;

const DUR = {
  fast: 0.2,
  base: 0.32,
  slow: 0.42,
  idle: 0.6,
} as const;

/** Collapse to orb-only after this idle on Ready. */
const IDLE_ORB_MS = 12_000;

/** Fade last Boris line on Ready after this (UI-only hide). */
const READY_CAPTION_MS = 9_000;

/**
 * OVERLAY — always-on-top voice presence.
 *
 * Layout rules (bugs we fixed):
 * - Title vertically centers with the orb when there is no secondary line
 * - Secondary row only mounts when it has real complementary text
 * - Primary never echoes secondary ("Working" + "Working…")
 * - No absolute-position title stack fighting the sizer (was causing off-alignment)
 */
export function OverlayWindow() {
  const status = useStatus();
  const tone = useMemo(
    () => toneFor(status.phase, status.engine),
    [status.phase, status.engine],
  );

  const { primary, secondary } = useMemo(
    () => pickOverlayPresence(status, tone.label, tone.hint),
    [status, tone.label, tone.hint],
  );

  const liveCaption = useMemo(() => pickCaption(status), [status]);
  const progress = showProgressBar(status);
  const confirm = isConfirmContext(status);
  const hasSecondary = Boolean(secondary.trim());
  const isReady =
    status.engine === "On" &&
    (status.phase === "Armed" || status.phase === "Quiet");

  const [orbOnly, setOrbOnly] = useState(false);
  /** Hide Ready caption after a soft delay so the island can idle. */
  const [readyCaptionHidden, setReadyCaptionHidden] = useState(false);

  const caption: Caption | null =
    isReady && readyCaptionHidden ? null : liveCaption;

  // Soft-hide last Boris line on Ready so orb collapse can proceed.
  useEffect(() => {
    if (!isReady || liveCaption?.kind !== "said") {
      setReadyCaptionHidden(false);
      return;
    }
    setReadyCaptionHidden(false);
    const t = window.setTimeout(
      () => setReadyCaptionHidden(true),
      READY_CAPTION_MS,
    );
    return () => window.clearTimeout(t);
  }, [isReady, liveCaption?.kind, liveCaption?.text, status.turn]);

  // Idle → orb
  useEffect(() => {
    if (!canCollapseToOrb(status) || caption) {
      setOrbOnly(false);
      return;
    }
    const t = window.setTimeout(() => setOrbOnly(true), IDLE_ORB_MS);
    return () => window.clearTimeout(t);
  }, [
    status.engine,
    status.phase,
    status.detail,
    status.activity,
    caption,
  ]);

  useEffect(() => {
    document.documentElement.classList.add("overlay-mode");
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    const meta = document.querySelector('meta[name="color-scheme"]');
    const prev = meta?.getAttribute("content") ?? null;
    meta?.setAttribute("content", "only light");
    return () => {
      if (prev != null) meta?.setAttribute("content", prev);
    };
  }, []);

  // ── Orb-only idle presence ─────────────────────────────────────────────
  if (orbOnly) {
    return (
      <div className="overlay-surface flex h-full w-full items-center justify-center bg-transparent p-3">
        <motion.div
          data-tauri-drag-region
          layout
          className="overlay-island relative flex items-center justify-center rounded-full p-2.5"
          initial={{ opacity: 0, scale: 0.9 }}
          animate={{
            opacity: 1,
            scale: 1,
            borderColor: "rgba(255,255,255,0.1)",
            boxShadow: `
              0 6px 18px rgba(0, 0, 0, 0.35),
              inset 0 1px 0 rgba(255, 255, 255, 0.08)
            `,
          }}
          transition={{ duration: DUR.idle, ease: softIdle }}
          title={tone.hint || primary}
        >
          <PresenceOrb motion={tone.motion} accent={tone.accent} size="md" />
        </motion.div>
      </div>
    );
  }

  return (
    <div className="overlay-surface flex h-full w-full items-center justify-center bg-transparent p-3">
      <LayoutGroup id="boris-overlay">
        <motion.div
          data-tauri-drag-region
          layout
          className={cn(
            "overlay-island relative flex w-max min-w-[220px] max-w-[min(340px,100%)]",
            "select-none flex-col rounded-[20px] px-3.5 py-2.5",
            confirm && "overlay-island--confirm",
          )}
          animate={{
            borderColor: confirm
              ? `color-mix(in oklch, ${tone.accent} 45%, rgba(255,255,255,0.1))`
              : "rgba(255,255,255,0.12)",
            boxShadow: `
              0 8px 24px rgba(0, 0, 0, 0.38),
              inset 0 1px 0 rgba(255, 255, 255, 0.09),
              0 0 ${isReady ? 0 : 12}px color-mix(in oklch, ${tone.glow} 55%, transparent)
            `,
          }}
          transition={{
            duration: isReady ? DUR.idle : DUR.slow,
            ease: isReady ? softIdle : soft,
            layout: { duration: DUR.slow, ease: softIdle },
          }}
        >
          {/* ── Primary row: orb + text, always vertically centered ── */}
          <div
            data-tauri-drag-region
            className="flex min-h-9 items-center gap-2.5"
          >
            <PresenceOrb motion={tone.motion} accent={tone.accent} />

            <div
              data-tauri-drag-region
              className={cn(
                "flex min-w-0 flex-1 flex-col justify-center",
                hasSecondary ? "gap-0.5 py-0.5" : "py-0",
              )}
            >
              {/*
                In-flow title (not absolute). Absolute + invisible sizer was
                shifting baselines and looking “off-aligned” next to the orb.
              */}
              <AnimatePresence mode="wait" initial={false}>
                <motion.span
                  key={`label-${primary}`}
                  data-tauri-drag-region
                  className="block truncate text-[13px] font-semibold leading-[1.2] tracking-[-0.02em] text-white"
                  initial={{ opacity: 0, y: 1 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -1 }}
                  transition={{
                    duration: isReady ? DUR.slow : DUR.fast,
                    ease: isReady ? softIdle : soft,
                  }}
                >
                  {primary}
                </motion.span>
              </AnimatePresence>

              {/* Secondary only when it adds information — no empty reserved line */}
              <AnimatePresence initial={false}>
                {hasSecondary ? (
                  <motion.p
                    key={`sub-${secondary}`}
                    data-tauri-drag-region
                    className="block truncate text-[11px] leading-[1.25] tracking-[-0.01em]"
                    initial={{ opacity: 0, height: 0, y: 1 }}
                    animate={{
                      opacity: 1,
                      height: "auto",
                      y: 0,
                      color: confirm
                        ? "rgba(255, 220, 160, 0.75)"
                        : "rgba(255,255,255,0.45)",
                    }}
                    exit={{ opacity: 0, height: 0, y: -1 }}
                    transition={{
                      duration: DUR.base,
                      ease: soft,
                    }}
                  >
                    {secondary}
                  </motion.p>
                ) : null}
              </AnimatePresence>
            </div>

            <DeviceFaultPips status={status} />
          </div>

          {/* Hairline progress while working (tools / thinking) */}
          <AnimatePresence initial={false}>
            {progress ? (
              <motion.div
                key="work-progress"
                data-tauri-drag-region
                className="mt-1.5 overflow-hidden"
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: "auto" }}
                exit={{ opacity: 0, height: 0 }}
                transition={{ duration: DUR.base, ease: soft }}
              >
                <span
                  className="overlay-think-bar block h-px w-full rounded-full"
                  style={
                    {
                      ["--think-accent" as string]: "rgba(255,255,255,0.55)",
                    } as CSSProperties
                  }
                />
              </motion.div>
            ) : null}
          </AnimatePresence>

          {/* ── Caption (You / Boris / Error) ─────────────────────────── */}
          <AnimatePresence initial={false} mode="popLayout">
            {caption ? (
              <motion.div
                key={`caption-${caption.kind}-${caption.text.slice(0, 24)}`}
                data-tauri-drag-region
                layout
                className={cn(
                  "overlay-caption min-w-0 max-w-[300px] overflow-hidden rounded-[12px] px-2.5 py-1.5",
                  confirm && "overlay-caption--confirm",
                )}
                initial={{ opacity: 0, height: 0, marginTop: 0 }}
                animate={{
                  opacity: isReady && caption.kind === "said" ? 0.7 : 1,
                  height: "auto",
                  marginTop: 8,
                }}
                exit={{
                  opacity: 0,
                  height: 0,
                  marginTop: 0,
                  transition: {
                    height: { duration: DUR.slow, ease: softIdle },
                    opacity: { duration: DUR.base, ease: softIdle },
                    marginTop: { duration: DUR.slow, ease: softIdle },
                  },
                }}
                transition={{
                  height: { duration: DUR.slow, ease: soft },
                  opacity: {
                    duration: isReady ? DUR.slow : DUR.base,
                    ease: isReady ? softIdle : soft,
                  },
                  marginTop: { duration: DUR.slow, ease: soft },
                  layout: { duration: DUR.slow, ease: softIdle },
                }}
              >
                <CaptionBody
                  caption={caption}
                  isReady={isReady}
                  streamKey={`${status.turn ?? ""}-${status.phase}`}
                />
              </motion.div>
            ) : null}
          </AnimatePresence>
        </motion.div>
      </LayoutGroup>
    </div>
  );
}

function CaptionBody({
  caption,
  isReady,
  streamKey,
}: {
  caption: Caption;
  isReady: boolean;
  streamKey: string;
}) {
  const textColor =
    caption.kind === "error"
      ? "rgba(252, 165, 165, 0.95)"
      : caption.kind === "said"
        ? isReady
          ? "rgba(255,255,255,0.55)"
          : "rgba(255,255,255,0.82)"
        : "rgba(255,255,255,0.68)";

  const textKey =
    caption.kind === "error"
      ? `err-${caption.text.slice(0, 48)}`
      : caption.kind === "said"
        ? `said-${caption.text.slice(0, 96)}`
        : `heard-${streamKey}-${caption.text.slice(0, 48)}`;

  return (
    <motion.p
      data-tauri-drag-region
      className="line-clamp-2 text-[12px] leading-snug tracking-[-0.01em]"
      animate={{ color: textColor }}
      transition={{
        duration: isReady ? DUR.idle : DUR.base,
        ease: isReady ? softIdle : soft,
      }}
    >
      {caption.kind !== "error" ? (
        <span className="mr-1.5 text-[10px] font-medium uppercase tracking-[0.06em] text-white/30">
          {caption.kind === "said" ? "Boris" : "You"}
        </span>
      ) : null}
      <AnimatePresence mode="wait" initial={false}>
        <motion.span
          key={textKey}
          initial={{ opacity: 0.4 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: DUR.fast, ease: soft }}
        >
          {caption.text}
        </motion.span>
      </AnimatePresence>
    </motion.p>
  );
}

function PresenceOrb({
  motion: motionKind,
  accent,
  size = "sm",
}: {
  motion: ReturnType<typeof toneFor>["motion"];
  accent: string;
  size?: "sm" | "md";
}) {
  const box = size === "md" ? "size-9" : "size-8";
  const core = size === "md" ? "size-3" : "size-2.5";

  return (
    <div
      data-tauri-drag-region
      className={cn("relative flex shrink-0 items-center justify-center", box)}
      aria-hidden
    >
      <AnimatePresence initial={false}>
        {motionKind !== "none" ? (
          <motion.span
            key={`ring-${motionKind}`}
            className="absolute inset-0"
            initial={{ opacity: 0, scale: 0.9 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 1.04 }}
            transition={{ duration: DUR.slow, ease: soft }}
          >
            <motion.span
              className={cn(
                "absolute inset-0 rounded-full border",
                motionKind === "listen" && "overlay-ring-listen",
                motionKind === "think" && "overlay-ring-think",
                motionKind === "speak" && "overlay-ring-speak",
                motionKind === "breathe" && "overlay-ring-breathe",
              )}
              animate={{ borderColor: accent }}
              transition={{ duration: DUR.slow, ease: soft }}
            />
          </motion.span>
        ) : null}
      </AnimatePresence>

      <motion.span
        className={cn(
          "absolute rounded-full",
          core,
          motionKind === "breathe" && "overlay-core-breathe",
          motionKind === "listen" && "overlay-core-listen",
          motionKind === "think" && "overlay-core-think",
          motionKind === "speak" && "overlay-core-speak",
        )}
        animate={{
          backgroundColor: accent,
          boxShadow: `0 0 10px color-mix(in oklch, ${accent} 55%, transparent)`,
        }}
        transition={{ duration: DUR.slow, ease: soft }}
      />
    </div>
  );
}

/** Only show device pips when something is wrong — not a permanent dashboard. */
function DeviceFaultPips({ status }: { status: StatusPicture }) {
  if (status.engine === "Off") return null;
  const micBad = !status.mic.ok;
  const speakerBad = !status.speaker.ok;
  if (!micBad && !speakerBad) return null;

  return (
    <div
      data-tauri-drag-region
      className="flex h-9 shrink-0 items-center gap-1"
    >
      {micBad ? (
        <FaultPip
          title={`Mic · ${status.mic.label}`}
          icon={<Mic className="size-3" strokeWidth={2} />}
        />
      ) : null}
      {speakerBad ? (
        <FaultPip
          title={`Speaker · ${status.speaker.label}`}
          icon={<Volume2 className="size-3" strokeWidth={2} />}
        />
      ) : null}
    </div>
  );
}

function FaultPip({ title, icon }: { title: string; icon: ReactNode }) {
  return (
    <div
      data-tauri-drag-region
      title={title}
      className="relative flex size-6 items-center justify-center rounded-full border border-amber-400/25 bg-amber-400/10 text-amber-200/80"
    >
      {icon}
    </div>
  );
}
