import {
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { AnimatePresence, LayoutGroup, motion } from "framer-motion";
import { Mic, Volume2 } from "lucide-react";
import { formatContextMeter, useStatus, type StatusPicture } from "@/bridge";
import { cn } from "@/lib/utils";
import { toneFor } from "@/lib/phaseVisual";

/** Soft ease-out — calm, no bounce. */
const soft = [0.22, 1, 0.36, 1] as const;
const softIdle = [0.16, 1, 0.3, 1] as const;

const DUR = {
  fast: 0.28,
  base: 0.42,
  slow: 0.55,
  idle: 0.7,
} as const;

/** Collapse to orb-only after this idle on Ready/Quiet. */
const IDLE_ORB_MS = 15_000;

const fadeSwap = {
  initial: { opacity: 0, y: 3, filter: "blur(2px)" },
  animate: { opacity: 1, y: 0, filter: "blur(0px)" },
  exit: { opacity: 0, y: -2, filter: "blur(2px)" },
  transition: { duration: DUR.base, ease: soft },
};

/**
 * OVERLAY — always-on-top voice island.
 *
 * - Progressive tool/confirm activity (compact chip)
 * - Context window meter (e.g. 12K / 500K)
 * - Collapses to a lone orb after ~15s idle on Ready
 * - Turn id is intentionally not shown (noisy)
 */
export function OverlayWindow() {
  const status = useStatus();
  const tone = useMemo(
    () => toneFor(status.phase, status.engine),
    [status.phase, status.engine],
  );
  const caption = pickCaption(status);
  const isReady =
    status.engine === "On" &&
    (status.phase === "Armed" || status.phase === "Quiet");
  const subtitle = pickSubtitle(status, tone, caption);
  const contextMeter = formatContextMeter(
    status.context_used,
    status.context_limit,
  );
  const activity = status.activity?.trim() || null;

  const [orbOnly, setOrbOnly] = useState(false);

  // Idle → orb: only when Ready/Quiet, engine On, no caption/activity.
  useEffect(() => {
    const canCollapse =
      status.engine === "On" &&
      (status.phase === "Armed" || status.phase === "Quiet") &&
      !caption &&
      !activity;

    if (!canCollapse) {
      setOrbOnly(false);
      return;
    }
    const t = window.setTimeout(() => setOrbOnly(true), IDLE_ORB_MS);
    return () => window.clearTimeout(t);
  }, [
    status.engine,
    status.phase,
    status.said,
    status.heard,
    status.detail,
    activity,
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
          initial={{ opacity: 0, scale: 0.85 }}
          animate={{
            opacity: 1,
            scale: 1,
            borderColor: `color-mix(in oklch, ${tone.accent} 28%, rgba(255,255,255,0.12))`,
            boxShadow: `
              0 8px 22px rgba(12, 14, 20, 0.4),
              inset 0 1px 0 rgba(255, 255, 255, 0.1),
              0 0 16px color-mix(in oklch, ${tone.glow} 40%, transparent)
            `,
          }}
          transition={{ duration: DUR.idle, ease: softIdle }}
          title={tone.hint}
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
            "overlay-island relative flex w-max min-w-[272px] max-w-[min(360px,100%)]",
            "select-none flex-col rounded-[22px] px-3.5 py-2.5",
          )}
          animate={{
            borderColor: `color-mix(in oklch, ${tone.accent} 32%, rgba(255,255,255,0.12))`,
            boxShadow: `
              0 10px 28px rgba(12, 14, 20, 0.45),
              inset 0 1px 0 rgba(255, 255, 255, 0.1),
              0 0 ${isReady ? 18 : 28}px color-mix(in oklch, ${tone.glow} ${isReady ? 35 : 50}%, transparent)
            `,
          }}
          transition={{
            duration: isReady ? DUR.idle : DUR.slow,
            ease: isReady ? softIdle : soft,
            layout: { duration: DUR.slow, ease: softIdle },
          }}
        >
          {/* ── Primary row ─────────────────────────────────────────── */}
          <div data-tauri-drag-region className="flex h-9 items-center gap-3">
            <PresenceOrb motion={tone.motion} accent={tone.accent} />

            <div
              data-tauri-drag-region
              className="flex min-w-0 flex-1 flex-col justify-center gap-0.5"
            >
              <div
                data-tauri-drag-region
                className="flex h-[1.125rem] items-center gap-2"
              >
                <div className="relative min-w-0 flex-1 overflow-hidden">
                  <AnimatePresence mode="sync" initial={false}>
                    <motion.span
                      key={`label-${tone.label}`}
                      data-tauri-drag-region
                      className="absolute inset-x-0 top-0 block truncate text-[13px] font-semibold leading-[1.125rem] tracking-tight text-white"
                      initial={fadeSwap.initial}
                      animate={fadeSwap.animate}
                      exit={fadeSwap.exit}
                      transition={{
                        duration: isReady ? DUR.slow : DUR.base,
                        ease: isReady ? softIdle : soft,
                      }}
                    >
                      {tone.label}
                    </motion.span>
                  </AnimatePresence>
                  <span
                    aria-hidden
                    className="invisible block truncate text-[13px] font-semibold leading-[1.125rem]"
                  >
                    {tone.label}
                  </span>
                </div>

                {/* Context window meter (Grok-style 12K / 500K) — not turn id */}
                <AnimatePresence initial={false}>
                  {contextMeter ? (
                    <motion.span
                      key={`ctx-${contextMeter}`}
                      data-tauri-drag-region
                      className="shrink-0 font-mono text-[10px] tabular-nums text-white/40"
                      initial={{ opacity: 0, scale: 0.92 }}
                      animate={{ opacity: 1, scale: 1 }}
                      exit={{ opacity: 0, scale: 0.92 }}
                      transition={{ duration: DUR.base, ease: soft }}
                      title="Estimated context window"
                    >
                      {contextMeter}
                    </motion.span>
                  ) : null}
                </AnimatePresence>
              </div>

              <div className="relative h-[1rem] overflow-hidden">
                <AnimatePresence mode="sync" initial={false}>
                  <motion.p
                    key={`sub-${activity ?? subtitle}`}
                    data-tauri-drag-region
                    className="absolute inset-x-0 top-0 truncate text-[11px] leading-4"
                    initial={fadeSwap.initial}
                    animate={{
                      ...fadeSwap.animate,
                      color: activity
                        ? "rgba(196, 181, 253, 0.85)"
                        : caption
                          ? "rgba(255,255,255,0.3)"
                          : "rgba(255,255,255,0.45)",
                    }}
                    exit={fadeSwap.exit}
                    transition={{
                      duration: isReady ? DUR.slow : DUR.base,
                      ease: isReady ? softIdle : soft,
                    }}
                  >
                    {activity ?? subtitle}
                  </motion.p>
                </AnimatePresence>
              </div>
            </div>

            <DevicePips status={status} />
          </div>

          {/* Compact progressive tool strip (Thinking only, no extra card bulk) */}
          <AnimatePresence initial={false}>
            {status.phase === "Thinking" && activity ? (
              <motion.div
                key="think-progress"
                data-tauri-drag-region
                className="mt-1.5 flex items-center gap-2 overflow-hidden"
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: "auto" }}
                exit={{ opacity: 0, height: 0 }}
                transition={{ duration: DUR.base, ease: soft }}
              >
                <span
                  className="overlay-think-bar h-0.5 flex-1 rounded-full"
                  style={
                    {
                      ["--think-accent" as string]: tone.accent,
                    } as CSSProperties
                  }
                />
              </motion.div>
            ) : null}
          </AnimatePresence>

          {/* ── Caption (You / Boris) ───────────────────────────────── */}
          <AnimatePresence initial={false} mode="popLayout">
            {caption ? (
              <motion.div
                key={`caption-${caption.kind}`}
                data-tauri-drag-region
                layout
                className="overlay-caption min-w-0 max-w-[320px] overflow-hidden rounded-xl px-2.5 py-1.5"
                initial={{ opacity: 0, height: 0, marginTop: 0, y: 8 }}
                animate={{
                  opacity: isReady && caption.kind === "said" ? 0.72 : 1,
                  height: "auto",
                  marginTop: 8,
                  y: 0,
                }}
                exit={{
                  opacity: 0,
                  height: 0,
                  marginTop: 0,
                  y: -4,
                  transition: {
                    height: { duration: DUR.slow, ease: softIdle },
                    opacity: { duration: DUR.base, ease: softIdle },
                    marginTop: { duration: DUR.slow, ease: softIdle },
                    y: { duration: DUR.base, ease: softIdle },
                  },
                }}
                transition={{
                  height: { duration: DUR.slow, ease: soft },
                  opacity: {
                    duration: isReady ? DUR.slow : DUR.base,
                    ease: isReady ? softIdle : soft,
                  },
                  marginTop: { duration: DUR.slow, ease: soft },
                  y: { duration: DUR.base, ease: soft },
                  layout: { duration: DUR.slow, ease: softIdle },
                }}
              >
                <CaptionBody
                  caption={caption}
                  isReady={isReady}
                  streamKey={status.turn ?? status.phase}
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
          : "rgba(255,255,255,0.8)"
        : "rgba(255,255,255,0.65)";

  const textKey =
    caption.kind === "error"
      ? `err-${caption.text.slice(0, 48)}`
      : caption.kind === "said"
        ? `said-${caption.text.slice(0, 96)}`
        : `heard-${streamKey}`;

  return (
    <motion.p
      data-tauri-drag-region
      className="line-clamp-2 text-[12px] leading-snug tracking-tight"
      animate={{ color: textColor }}
      transition={{
        duration: isReady ? DUR.idle : DUR.base,
        ease: isReady ? softIdle : soft,
      }}
    >
      {caption.kind !== "error" ? (
        <motion.span
          className="mr-1.5 text-[10px] font-medium uppercase tracking-wider"
          animate={{ color: "rgba(255,255,255,0.3)" }}
          transition={{ duration: DUR.base, ease: soft }}
        >
          {caption.kind === "said" ? "Boris" : "You"}
        </motion.span>
      ) : null}
      <AnimatePresence mode="sync" initial={false}>
        <motion.span
          key={textKey}
          initial={{ opacity: 0.35 }}
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
  const box = size === "md" ? "size-10" : "size-9";
  const core = size === "md" ? "size-3.5" : "size-3";

  return (
    <div
      data-tauri-drag-region
      className={cn("relative flex shrink-0 items-center justify-center", box)}
      aria-hidden
    >
      <AnimatePresence initial={false}>
        {motionKind !== "none" ? (
          <motion.span
            key={`ring-a-${motionKind}`}
            className="absolute inset-0"
            initial={{ opacity: 0, scale: 0.86 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 1.06 }}
            transition={{ duration: DUR.slow, ease: soft }}
          >
            <motion.span
              className={cn(
                "absolute inset-0 rounded-full border-[1.5px]",
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

      <AnimatePresence initial={false}>
        {motionKind === "listen" || motionKind === "speak" ? (
          <motion.span
            key={`ring-b-${motionKind}`}
            className="absolute inset-0.5"
            initial={{ opacity: 0, scale: 0.92 }}
            animate={{ opacity: 0.7, scale: 1 }}
            exit={{ opacity: 0, scale: 1.04 }}
            transition={{ duration: DUR.base, ease: soft }}
          >
            <motion.span
              className="overlay-ring-listen-delay absolute inset-0 rounded-full border"
              animate={{ borderColor: accent }}
              transition={{ duration: DUR.slow, ease: soft }}
            />
          </motion.span>
        ) : null}
      </AnimatePresence>

      <AnimatePresence mode="sync" initial={false}>
        <motion.span
          key={`core-wrap-${motionKind}`}
          className={cn("absolute flex items-center justify-center", core)}
          initial={{ opacity: 0, scale: 0.7 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 1.15 }}
          transition={{ duration: DUR.slow, ease: soft }}
        >
          <motion.span
            className={cn(
              "rounded-full",
              core,
              motionKind === "breathe" && "overlay-core-breathe",
              motionKind === "listen" && "overlay-core-listen",
              motionKind === "think" && "overlay-core-think",
              motionKind === "speak" && "overlay-core-speak",
            )}
            animate={{
              backgroundColor: accent,
              boxShadow: `0 0 16px ${accent}`,
            }}
            transition={{ duration: DUR.slow, ease: soft }}
          />
        </motion.span>
      </AnimatePresence>
    </div>
  );
}

function DevicePips({ status }: { status: StatusPicture }) {
  return (
    <div
      data-tauri-drag-region
      className="flex h-9 shrink-0 items-center gap-1.5"
    >
      <Pip
        ok={status.mic.ok && status.engine !== "Off"}
        title={`Mic · ${status.mic.label}`}
        icon={<Mic className="size-3" strokeWidth={2} />}
      />
      <Pip
        ok={status.speaker.ok && status.engine !== "Off"}
        title={`Speaker · ${status.speaker.label}`}
        icon={<Volume2 className="size-3" strokeWidth={2} />}
      />
    </div>
  );
}

function Pip({
  ok,
  title,
  icon,
}: {
  ok: boolean;
  title: string;
  icon: ReactNode;
}) {
  return (
    <motion.div
      data-tauri-drag-region
      title={title}
      className="relative flex size-7 items-center justify-center rounded-full border"
      animate={{
        opacity: ok ? 1 : 0.55,
        borderColor: ok ? "rgba(255,255,255,0.1)" : "rgba(255,255,255,0.05)",
        backgroundColor: ok
          ? "rgba(255,255,255,0.05)"
          : "rgba(255,255,255,0.03)",
        color: ok ? "rgba(255,255,255,0.7)" : "rgba(255,255,255,0.25)",
      }}
      transition={{ duration: DUR.base, ease: soft }}
    >
      {icon}
      <motion.span
        className="absolute bottom-0.5 right-0.5 size-1.5 rounded-full ring-1 ring-black/40"
        animate={{
          backgroundColor: ok ? "rgb(52 211 153)" : "rgba(255,255,255,0.2)",
          scale: ok ? 1 : 0.9,
        }}
        transition={{ duration: DUR.base, ease: soft }}
        aria-hidden
      />
    </motion.div>
  );
}

type Caption = {
  kind: "heard" | "said" | "error";
  text: string;
};

function pickSubtitle(
  status: StatusPicture,
  tone: ReturnType<typeof toneFor>,
  caption: Caption | null,
): string {
  if (!caption) return tone.hint;
  if (caption.kind === "error") return "Something came up";
  if (caption.kind === "said") {
    if (status.phase === "Armed" || status.phase === "Quiet") {
      return "Say the wake word";
    }
    if (status.phase === "AwaitingReply") return "Your turn to answer";
    if (status.phase === "AwaitingConfirm") return "Yes, no, sure, cancel…";
    return "Speaking…";
  }
  return "Listening…";
}

function pickCaption(status: StatusPicture): Caption | null {
  // detail is errors only (confirm text lives in activity / said)
  if (status.detail?.trim()) {
    return { kind: "error", text: status.detail.trim() };
  }
  if (
    (status.phase === "Talking" ||
      status.phase === "AwaitingReply" ||
      status.phase === "AwaitingConfirm") &&
    status.said?.trim()
  ) {
    return { kind: "said", text: status.said.trim() };
  }
  if (
    (status.phase === "Thinking" ||
      status.phase === "Reading" ||
      status.phase === "Hearing" ||
      status.phase === "Talking" ||
      status.phase === "AwaitingReply" ||
      status.phase === "AwaitingConfirm") &&
    status.heard?.trim()
  ) {
    if (
      (status.phase !== "Talking" &&
        status.phase !== "AwaitingReply" &&
        status.phase !== "AwaitingConfirm") ||
      !status.said?.trim()
    ) {
      return { kind: "heard", text: status.heard.trim() };
    }
  }
  if (status.said?.trim() && status.phase === "Armed") {
    return { kind: "said", text: status.said.trim() };
  }
  if (status.heard?.trim() && status.engine === "On") {
    return { kind: "heard", text: status.heard.trim() };
  }
  return null;
}
