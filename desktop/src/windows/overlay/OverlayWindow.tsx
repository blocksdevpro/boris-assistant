import { useEffect, useMemo, type ReactNode } from "react";
import { AnimatePresence, LayoutGroup, motion } from "framer-motion";
import { Mic, Volume2 } from "lucide-react";
import { useStatus, type StatusPicture } from "@/bridge";
import { cn } from "@/lib/utils";
import { toneFor } from "@/lib/phaseVisual";

/** Soft ease-out — calm, no bounce. */
const soft = [0.22, 1, 0.36, 1] as const;

const fadeSwap = {
  initial: { opacity: 0, y: 5, filter: "blur(3px)" },
  animate: { opacity: 1, y: 0, filter: "blur(0px)" },
  exit: { opacity: 0, y: -4, filter: "blur(2px)" },
  transition: { duration: 0.38, ease: soft },
};

/**
 * OVERLAY — always-on-top voice island.
 *
 * Phase / caption / accent transitions use Framer Motion for subtle crossfades
 * and layout animation. Tauri transparency rules still apply.
 */
export function OverlayWindow() {
  const status = useStatus();
  const tone = useMemo(
    () => toneFor(status.phase, status.engine),
    [status.phase, status.engine],
  );
  const caption = pickCaption(status);
  const phaseKey = `${status.engine}-${status.phase}`;

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
              0 0 28px color-mix(in oklch, ${tone.glow} 50%, transparent)
            `,
          }}
          transition={{ duration: 0.55, ease: soft }}
        >
          {/* ── Primary row ─────────────────────────────────────────── */}
          <div
            data-tauri-drag-region
            className="flex h-9 items-center gap-3"
          >
            <PresenceOrb motion={tone.motion} accent={tone.accent} />

            {/* Fixed-height text column keeps vertical alignment stable */}
            <div
              data-tauri-drag-region
              className="flex min-w-0 flex-1 flex-col justify-center gap-0.5"
            >
              <div
                data-tauri-drag-region
                className="flex h-[1.125rem] items-center gap-2"
              >
                <div className="relative min-w-0 flex-1 overflow-hidden">
                  <AnimatePresence mode="wait" initial={false}>
                    <motion.span
                      key={`label-${phaseKey}-${tone.label}`}
                      data-tauri-drag-region
                      className="block truncate text-[13px] font-semibold leading-[1.125rem] tracking-tight text-white"
                      initial={fadeSwap.initial}
                      animate={fadeSwap.animate}
                      exit={fadeSwap.exit}
                      transition={fadeSwap.transition}
                    >
                      {tone.label}
                    </motion.span>
                  </AnimatePresence>
                </div>

                <AnimatePresence initial={false}>
                  {status.turn ? (
                    <motion.span
                      key={`turn-${status.turn}`}
                      data-tauri-drag-region
                      className="shrink-0 font-mono text-[10px] tabular-nums text-white/35"
                      initial={{ opacity: 0 }}
                      animate={{ opacity: 1 }}
                      exit={{ opacity: 0 }}
                      transition={{ duration: 0.3, ease: soft }}
                    >
                      #{status.turn}
                    </motion.span>
                  ) : null}
                </AnimatePresence>
              </div>

              <div className="relative h-[1rem] overflow-hidden">
                <AnimatePresence mode="wait" initial={false}>
                  {!caption ? (
                    <motion.p
                      key={`hint-${phaseKey}-${tone.hint}`}
                      data-tauri-drag-region
                      className="truncate text-[11px] leading-4 text-white/45"
                      initial={fadeSwap.initial}
                      animate={fadeSwap.animate}
                      exit={fadeSwap.exit}
                      transition={fadeSwap.transition}
                    >
                      {tone.hint}
                    </motion.p>
                  ) : (
                    <motion.p
                      key="hint-spacer"
                      data-tauri-drag-region
                      className="truncate text-[11px] leading-4 text-white/30"
                      initial={{ opacity: 0 }}
                      animate={{ opacity: 1 }}
                      exit={{ opacity: 0 }}
                      transition={{ duration: 0.25, ease: soft }}
                    >
                      {caption.kind === "error"
                        ? "Something came up"
                        : caption.kind === "said"
                          ? "Speaking…"
                          : "Listening…"}
                    </motion.p>
                  )}
                </AnimatePresence>
              </div>
            </div>

            <DevicePips status={status} />
          </div>

          {/* ── Caption (You / Boris) ───────────────────────────────── */}
          <AnimatePresence initial={false} mode="sync">
            {caption ? (
              <motion.div
                key={`caption-${caption.kind}-${caption.text.slice(0, 64)}`}
                data-tauri-drag-region
                layout
                className="overlay-caption mt-2 min-w-0 max-w-[320px] overflow-hidden rounded-xl px-2.5 py-1.5"
                initial={{ opacity: 0, height: 0, marginTop: 0, y: 4 }}
                animate={{ opacity: 1, height: "auto", marginTop: 8, y: 0 }}
                exit={{ opacity: 0, height: 0, marginTop: 0, y: -2 }}
                transition={{
                  height: { duration: 0.4, ease: soft },
                  opacity: { duration: 0.32, ease: soft },
                  marginTop: { duration: 0.4, ease: soft },
                  y: { duration: 0.35, ease: soft },
                }}
              >
                <p
                  data-tauri-drag-region
                  className={cn(
                    "line-clamp-2 text-[12px] leading-snug tracking-tight",
                    caption.kind === "error"
                      ? "text-red-300/95"
                      : caption.kind === "said"
                        ? "text-white/80"
                        : "text-white/65",
                  )}
                >
                  {caption.kind !== "error" ? (
                    <span className="mr-1.5 text-[10px] font-medium uppercase tracking-wider text-white/30">
                      {caption.kind === "said" ? "Boris" : "You"}
                    </span>
                  ) : null}
                  {caption.text}
                </p>
              </motion.div>
            ) : null}
          </AnimatePresence>
        </motion.div>
      </LayoutGroup>
    </div>
  );
}

function PresenceOrb({
  motion: motionKind,
  accent,
}: {
  motion: ReturnType<typeof toneFor>["motion"];
  accent: string;
}) {
  return (
    <div
      data-tauri-drag-region
      className="relative flex size-9 shrink-0 items-center justify-center"
      aria-hidden
    >
      <AnimatePresence initial={false}>
        {motionKind !== "none" ? (
          <motion.span
            key={`ring-a-${motionKind}`}
            className={cn(
              "absolute inset-0 rounded-full border-[1.5px]",
              motionKind === "listen" && "overlay-ring-listen",
              motionKind === "think" && "overlay-ring-think",
              motionKind === "speak" && "overlay-ring-speak",
              motionKind === "breathe" && "overlay-ring-breathe",
            )}
            style={{ borderColor: accent }}
            initial={{ opacity: 0, scale: 0.88 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 1.05 }}
            transition={{ duration: 0.4, ease: soft }}
          />
        ) : null}
      </AnimatePresence>

      <AnimatePresence initial={false}>
        {motionKind === "listen" || motionKind === "speak" ? (
          <motion.span
            key={`ring-b-${motionKind}`}
            className="overlay-ring-listen-delay absolute inset-0.5 rounded-full border"
            style={{ borderColor: accent }}
            initial={{ opacity: 0 }}
            animate={{ opacity: 0.7 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.35, ease: soft }}
          />
        ) : null}
      </AnimatePresence>

      <motion.span
        className={cn(
          "relative size-3 rounded-full",
          motionKind === "breathe" && "overlay-core-breathe",
          motionKind === "listen" && "overlay-core-listen",
          motionKind === "think" && "overlay-core-think",
          motionKind === "speak" && "overlay-core-speak",
        )}
        animate={{
          backgroundColor: accent,
          boxShadow: `0 0 16px ${accent}`,
        }}
        transition={{ duration: 0.5, ease: soft }}
      />
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
      className={cn(
        "relative flex size-7 items-center justify-center rounded-full border",
        ok
          ? "border-white/10 bg-white/5 text-white/70"
          : "border-white/5 bg-white/[0.03] text-white/25",
      )}
      animate={{
        opacity: ok ? 1 : 0.55,
        borderColor: ok ? "rgba(255,255,255,0.1)" : "rgba(255,255,255,0.05)",
      }}
      transition={{ duration: 0.4, ease: soft }}
    >
      {icon}
      <motion.span
        className="absolute bottom-0.5 right-0.5 size-1.5 rounded-full ring-1 ring-black/40"
        animate={{
          backgroundColor: ok ? "rgb(52 211 153)" : "rgba(255,255,255,0.2)",
        }}
        transition={{ duration: 0.4, ease: soft }}
        aria-hidden
      />
    </motion.div>
  );
}

type Caption = {
  kind: "heard" | "said" | "error";
  text: string;
};

function pickCaption(status: StatusPicture): Caption | null {
  if (status.detail?.trim()) {
    return { kind: "error", text: status.detail.trim() };
  }
  if (
    (status.phase === "Talking" || status.phase === "AwaitingReply") &&
    status.said?.trim()
  ) {
    return { kind: "said", text: status.said.trim() };
  }
  if (
    (status.phase === "Thinking" ||
      status.phase === "Reading" ||
      status.phase === "Hearing" ||
      status.phase === "Talking" ||
      status.phase === "AwaitingReply") &&
    status.heard?.trim()
  ) {
    if (
      (status.phase !== "Talking" && status.phase !== "AwaitingReply") ||
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
