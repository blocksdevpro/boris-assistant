import { useEffect, useRef, useState } from "react";
import { CheckCircle2, MicOff, UserX, Volume2 } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { Popover } from "radix-ui";
import { openMicPrivacySettings, type ListenHint, type StatusPicture } from "@/bridge";
import { logger } from "@/lib/logger";
import { makeRoomMotion, popSpring, settleSpring, softEase } from "@/lib/mainMotion";

/** "Mic is working" stays this long after a silent mic comes back. */
const RECOVERED_FOR_MS = 2600;

/** Ignored-wake notices are informational: they leave on their own. */
const FADE_AFTER_MS: Partial<Record<ListenHint, number>> = {
  ignored_voice: 7000,
  ignored_playback: 4000,
};

/**
 * Small pill under the Home status line saying why "Hey Boris" is not doing
 * anything, Discord-style: short label, one action, details on demand.
 */
export function ListenHintPill({
  status,
  onChooseMic,
  onTeachVoice,
}: {
  status: StatusPicture;
  /** Boris settings → Speech, where the microphone is chosen. */
  onChooseMic: () => void;
  onTeachVoice: () => void;
}) {
  const reduced = Boolean(useReducedMotion());
  const listening =
    status.engine === "On" && (status.phase === "Armed" || status.phase === "Quiet");
  const raw = status.listen_hint ?? null;
  const hint = listening ? raw : null;
  const [faded, setFaded] = useState<ListenHint | null>(null);
  const [open, setOpen] = useState(false);
  // Close the loop on a fix: a silent mic that comes back says so briefly
  // instead of the warning just vanishing.
  const previousRaw = useRef(raw);
  const [recovered, setRecovered] = useState(false);

  useEffect(() => {
    const was = previousRaw.current;
    previousRaw.current = raw;
    if (was === "mic_silent" && raw === null && listening) setRecovered(true);
    else if (raw !== null) setRecovered(false);
  }, [raw, listening]);

  useEffect(() => {
    if (!recovered) return;
    const timer = setTimeout(() => setRecovered(false), RECOVERED_FOR_MS);
    return () => clearTimeout(timer);
  }, [recovered]);

  useEffect(() => {
    setOpen(false);
    if (!hint) {
      setFaded(null);
      return;
    }
    const after = FADE_AFTER_MS[hint];
    if (!after) return;
    const timer = setTimeout(() => setFaded(hint), after);
    return () => clearTimeout(timer);
  }, [hint]);

  const visible: ListenHint | "mic_ok" | null =
    hint && hint !== faded ? hint : recovered && listening ? "mic_ok" : null;
  const device =
    status.mic.label && status.mic.label !== "—" ? status.mic.label : "your microphone";

  return (
    <AnimatePresence initial={false} mode="wait">
      {visible ? (
        <motion.div key={visible} {...makeRoomMotion(reduced)}>
          <div className="pt-3">
            <motion.div
              data-tone={
                visible === "mic_silent" ? "attention" : visible === "mic_ok" ? "ok" : undefined
              }
              data-quiet={visible === "ignored_playback" || visible === "mic_ok" || undefined}
              className="listen-pill"
              initial={reduced ? { opacity: 0 } : { opacity: 0, scale: 0.92, filter: "blur(4px)" }}
              animate={reduced ? { opacity: 1 } : { opacity: 1, scale: 1, filter: "blur(0px)" }}
              exit={reduced ? { opacity: 0 } : { opacity: 0, scale: 0.96, transition: { duration: 0.16 } }}
              transition={reduced ? { duration: 0 } : { ...settleSpring, opacity: { duration: 0.28, ease: softEase } }}
              style={{ transformOrigin: "left center" }}
            >
              {visible === "mic_ok" ? (
                <>
                  <motion.span
                    className="inline-flex"
                    initial={reduced ? false : { scale: 0.5, opacity: 0 }}
                    animate={{ scale: 1, opacity: 1 }}
                    transition={reduced ? { duration: 0 } : { ...popSpring, delay: 0.08 }}
                  >
                    <CheckCircle2 className="size-3.5 text-[var(--boris-green)]" strokeWidth={2.2} />
                  </motion.span>
                  <span>Mic is working</span>
                </>
              ) : visible === "mic_silent" ? (
                <>
                  <motion.span
                    className="inline-flex"
                    initial={false}
                    animate={reduced ? undefined : { rotate: [0, -14, 11, -6, 0] }}
                    transition={reduced ? undefined : { duration: 0.6, ease: "easeInOut", delay: 0.3 }}
                  >
                    <MicOff className="size-3.5 text-[var(--boris-orange)]" strokeWidth={2.2} />
                  </motion.span>
                  <span>Can&apos;t hear your mic</span>
                  <MicFixPopover
                    open={open}
                    onOpenChange={setOpen}
                    device={device}
                    reduced={reduced}
                    onChooseMic={() => {
                      setOpen(false);
                      onChooseMic();
                    }}
                  />
                </>
              ) : visible === "ignored_voice" ? (
                <>
                  <UserX className="size-3.5 text-white/55" strokeWidth={2} />
                  <span>Didn&apos;t sound like you</span>
                  <button type="button" className="listen-pill__action" onClick={onTeachVoice}>
                    Re-teach
                  </button>
                </>
              ) : (
                <>
                  <Volume2 className="size-3.5 text-white/45" strokeWidth={2} />
                  <span className="text-white/65">Ignored a speaker or TV</span>
                </>
              )}
            </motion.div>
          </div>
        </motion.div>
      ) : null}
    </AnimatePresence>
  );
}

function MicFixPopover({
  open,
  onOpenChange,
  device,
  reduced,
  onChooseMic,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  device: string;
  reduced: boolean;
  onChooseMic: () => void;
}) {
  return (
    <Popover.Root open={open} onOpenChange={onOpenChange}>
      <Popover.Trigger asChild>
        <button type="button" className="listen-pill__action" aria-haspopup="dialog">
          Fix
        </button>
      </Popover.Trigger>
      <AnimatePresence>
        {open ? (
          <Popover.Portal forceMount>
            <Popover.Content
              forceMount
              asChild
              side="bottom"
              align="start"
              sideOffset={10}
              collisionPadding={16}
            >
              <motion.div
                className="listen-popover z-50"
                initial={reduced ? { opacity: 0 } : { opacity: 0, y: -6, scale: 0.96, filter: "blur(4px)" }}
                animate={reduced ? { opacity: 1 } : { opacity: 1, y: 0, scale: 1, filter: "blur(0px)" }}
                exit={reduced ? { opacity: 0 } : { opacity: 0, y: -4, scale: 0.98, transition: { duration: 0.14, ease: softEase } }}
                transition={reduced ? { duration: 0 } : { ...popSpring, opacity: { duration: 0.2, ease: softEase } }}
              >
                <p className="text-[13.5px] font-semibold tracking-[-0.01em] text-white/92">
                  No sound from {device}
                </p>
                <ul className="mt-2 space-y-1.5 text-[12.5px] leading-snug text-[var(--boris-label-secondary)]">
                  <li>Windows may be blocking mic access for desktop apps.</li>
                  <li>The mic may be muted, by a key or a headset switch.</li>
                  <li>Another mic may be selected in Boris.</li>
                </ul>
                <div className="mt-3.5 flex flex-wrap gap-2">
                  <button
                    type="button"
                    onClick={() =>
                      void openMicPrivacySettings().catch((e) =>
                        logger.error("openMicPrivacySettings failed", String(e)),
                      )
                    }
                    className="h-8 rounded-full bg-white px-3.5 text-[12.5px] font-medium text-[#0b0b0c] transition-colors duration-200 hover:bg-white/90"
                  >
                    Open Windows mic settings
                  </button>
                  <button
                    type="button"
                    onClick={onChooseMic}
                    className="h-8 rounded-full px-3 text-[12.5px] text-[var(--boris-label-secondary)] transition-colors duration-200 hover:bg-white/[0.07] hover:text-white/85"
                  >
                    Choose mic
                  </button>
                </div>
              </motion.div>
            </Popover.Content>
          </Popover.Portal>
        ) : null}
      </AnimatePresence>
    </Popover.Root>
  );
}
