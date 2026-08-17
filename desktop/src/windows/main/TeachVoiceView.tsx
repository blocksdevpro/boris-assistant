import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { AlertCircle, Check, Mic, Power } from "lucide-react";
import {
  clearWakeProfile,
  startWakeEnroll,
  type StatusPicture,
} from "@/bridge";
import { cn } from "@/lib/utils";

const WANT = 4;

type TeachStage = "off" | "starting" | "listening" | "ready" | "error";

export function TeachVoiceView({
  status,
  engineOn,
  busy,
  onStartEngine,
  onDone,
}: {
  status: StatusPicture;
  engineOn: boolean;
  busy: boolean;
  onStartEngine: () => void;
  onDone: () => void;
}) {
  const enroll = status.wake_enroll;
  const [acceptReady, setAcceptReady] = useState(false);
  const staleCompletedSample = enroll?.ready === true && !acceptReady;
  const ready = enroll?.ready === true && acceptReady;
  const have = staleCompletedSample
    ? 0
    : Math.min(enroll?.have ?? 0, enroll?.want ?? WANT);
  const want = Math.max(enroll?.want ?? WANT, WANT);
  const hint = enroll?.hint?.trim() || null;
  const [startErr, setStartErr] = useState<string | null>(null);
  const started = useRef(false);
  const reduceMotion = useReducedMotion();

  const beginEnrollment = useCallback(() => {
    if (started.current) return;
    started.current = true;
    setAcceptReady(false);
    setStartErr(null);
    void clearWakeProfile()
      .then(() => startWakeEnroll(WANT))
      .catch((error) => {
        started.current = false;
        setStartErr(error instanceof Error ? error.message : String(error));
      });
  }, []);

  useEffect(() => {
    // A non-ready status proves the new enrollment session has replaced any
    // completed sample that was present when the page opened.
    if (enroll && !enroll.ready) setAcceptReady(true);
  }, [enroll]);

  useEffect(() => {
    if (!engineOn) {
      // Allow the flow to recover if Boris is stopped and started while this
      // view stays mounted.
      started.current = false;
      setAcceptReady(false);
      return;
    }
    beginEnrollment();
  }, [beginEnrollment, engineOn]);

  const stage: TeachStage = !engineOn
    ? "off"
    : startErr
      ? "error"
      : ready
        ? "ready"
        : staleCompletedSample
          ? "starting"
          : enroll
            ? "listening"
            : "starting";

  const statusCopy =
    stage === "off"
      ? "Start Boris to turn on your microphone"
      : stage === "error"
        ? "Voice setup couldn’t start"
        : stage === "ready"
          ? "Voice sample saved"
          : stage === "starting"
            ? "Preparing your microphone…"
            : (hint ??
              (have === 0
                ? "Listening for “Hey Boris”…"
                : `${have} of ${want} samples recorded`));

  const transition = reduceMotion
    ? { duration: 0 }
    : { duration: 0.28, ease: [0.22, 1, 0.36, 1] as const };

  return (
    <div className="teach-view relative mx-auto flex min-h-full w-full max-w-xl flex-col overflow-hidden px-7 pb-6 pt-7 sm:px-10">
      <div
        className="pointer-events-none absolute left-1/2 top-[46%] size-80 -translate-x-1/2 -translate-y-1/2 rounded-full bg-white/[0.018] blur-3xl"
        aria-hidden
      />

      <header className="teach-header relative mx-auto max-w-md text-center">
        <div className="teach-kicker mb-3 inline-flex items-center gap-2 rounded-full border border-white/[0.07] bg-white/[0.035] px-3 py-1.5 text-[11px] font-medium tracking-wide text-white/55">
          <Mic className="size-3" strokeWidth={2} />
          Voice setup
        </div>
        <AnimatePresence mode="wait" initial={false}>
          <motion.div
            key={ready ? "complete-heading" : "setup-heading"}
            initial={reduceMotion ? false : { opacity: 0, y: 5 }}
            animate={{ opacity: 1, y: 0 }}
            exit={reduceMotion ? { opacity: 1 } : { opacity: 0, y: -4 }}
            transition={transition}
          >
            <h1 className="teach-heading text-[28px] font-semibold tracking-[-0.035em] text-white sm:text-[30px]">
              {ready ? "You’re all set" : "Teach Boris your voice"}
            </h1>
            <p className="teach-description mx-auto mt-2 max-w-sm text-[14px] leading-relaxed text-white/48 sm:text-[15px]">
              {ready
                ? "Boris can now better tell your voice apart from nearby speakers."
                : "Say “Hey Boris” four times in your normal voice. Pause briefly between each one."}
            </p>
          </motion.div>
        </AnimatePresence>
      </header>

      <section className="teach-stage relative flex flex-1 flex-col items-center justify-center py-5 sm:py-7">
        <ListeningMark stage={stage} have={have} want={want} />

        <div
          className="teach-status mt-5 min-h-11 text-center"
          aria-live="polite"
          aria-atomic="true"
        >
          <AnimatePresence mode="wait" initial={false}>
            <motion.div
              key={statusCopy}
              initial={reduceMotion ? false : { opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={reduceMotion ? { opacity: 1 } : { opacity: 0, y: -3 }}
              transition={transition}
            >
              <p
                className={cn(
                  "text-[13px] font-medium tracking-[0.01em]",
                  stage === "error"
                    ? "text-red-300/90"
                    : stage === "ready"
                      ? "text-emerald-200/80"
                      : stage === "listening"
                        ? "text-white/78"
                        : "text-white/48",
                )}
              >
                {statusCopy}
              </p>
              {stage === "listening" ? (
                <p className="mt-1 text-[12px] text-white/35">
                  Speak from your usual distance
                </p>
              ) : null}
            </motion.div>
          </AnimatePresence>
        </div>

        <TakeProgress have={have} want={want} ready={ready} />
      </section>

      <footer className="relative mx-auto w-full max-w-sm">
        {startErr ? (
          <div
            className="mb-3 flex items-start gap-2.5 rounded-xl border border-red-300/10 bg-red-300/[0.055] px-3 py-2.5 text-left"
            role="alert"
          >
            <AlertCircle className="mt-0.5 size-4 shrink-0 text-red-300/80" />
            <p className="text-[12px] leading-relaxed text-red-100/65">
              {startErr}
            </p>
          </div>
        ) : null}

        {stage === "off" ? (
          <PrimaryButton disabled={busy} onClick={onStartEngine}>
            <Power className="size-4" strokeWidth={2} />
            {busy ? "Starting Boris…" : "Start Boris"}
          </PrimaryButton>
        ) : stage === "error" ? (
          <PrimaryButton
            onClick={() => {
              started.current = false;
              beginEnrollment();
            }}
          >
            Try again
          </PrimaryButton>
        ) : stage === "ready" ? (
          <PrimaryButton onClick={onDone}>
            Finish setup
            <Check className="size-4" strokeWidth={2.25} />
          </PrimaryButton>
        ) : (
          <p className="text-center text-[11px] leading-relaxed text-white/28">
            Keep this page open while Boris learns your voice.
          </p>
        )}
      </footer>
    </div>
  );
}

function PrimaryButton({
  children,
  disabled,
  onClick,
}: {
  children: React.ReactNode;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      className={cn(
        "flex h-11 w-full items-center justify-center gap-2 rounded-xl bg-white",
        "text-[14px] font-semibold text-[#111113] shadow-[0_1px_0_rgba(255,255,255,0.35)_inset]",
        "transition-[transform,background-color,box-shadow,opacity] duration-200",
        "hover:-translate-y-px hover:bg-white/94 hover:shadow-[0_8px_24px_rgba(0,0,0,0.22)]",
        "active:translate-y-0 active:scale-[0.99]",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/35 focus-visible:ring-offset-2 focus-visible:ring-offset-[#0b0b0c]",
        "disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:translate-y-0 disabled:hover:shadow-none",
      )}
    >
      {children}
    </button>
  );
}

function TakeProgress({
  have,
  want,
  ready,
}: {
  have: number;
  want: number;
  ready: boolean;
}) {
  return (
    <div
      className="teach-progress mt-3 w-full max-w-[17rem]"
      role="progressbar"
      aria-label="Voice samples recorded"
      aria-valuemin={0}
      aria-valuemax={want}
      aria-valuenow={have}
      aria-valuetext={
        ready ? "Voice setup complete" : `${have} of ${want} samples recorded`
      }
    >
      <ol className="flex items-center" aria-hidden>
        {Array.from({ length: want }, (_, index) => {
          const complete = index < have;
          return (
            <li key={index} className="flex flex-1 items-center last:flex-none">
              <motion.span
                className={cn(
                  "flex size-7 shrink-0 items-center justify-center rounded-full border text-[11px] font-semibold",
                  complete
                    ? "border-white bg-white text-[#111113]"
                    : "border-white/12 bg-white/[0.035] text-white/35",
                )}
                animate={
                  complete
                    ? { scale: [0.82, 1.08, 1], opacity: 1 }
                    : { scale: 1, opacity: 1 }
                }
                transition={{ duration: 0.32, ease: [0.22, 1, 0.36, 1] }}
              >
                {complete ? (
                  <Check className="size-3.5" strokeWidth={2.5} />
                ) : (
                  index + 1
                )}
              </motion.span>
              {index < want - 1 ? (
                <span className="mx-1.5 h-px flex-1 overflow-hidden bg-white/10">
                  <span
                    className={cn(
                      "block h-full origin-left bg-white/70 transition-transform duration-500 ease-out",
                      index < have - 1 ? "scale-x-100" : "scale-x-0",
                    )}
                  />
                </span>
              ) : null}
            </li>
          );
        })}
      </ol>
    </div>
  );
}

function ListeningMark({
  stage,
  have,
  want,
}: {
  stage: TeachStage;
  have: number;
  want: number;
}) {
  const ready = stage === "ready";
  const listening = stage === "listening";
  const progress = want > 0 ? have / want : 0;
  const circumference = 2 * Math.PI * 68;

  return (
    <div
      className="teach-listening-mark relative flex size-40 items-center justify-center sm:size-44"
      role="img"
      aria-label={
        ready
          ? "Voice setup complete"
          : listening
            ? "Microphone is listening"
            : "Microphone is waiting"
      }
    >
      <svg
        viewBox="0 0 160 160"
        className="absolute inset-0 size-full -rotate-90"
        aria-hidden
      >
        <circle
          cx="80"
          cy="80"
          r="68"
          fill="none"
          stroke="rgba(255,255,255,0.075)"
          strokeWidth="1.5"
        />
        <motion.circle
          cx="80"
          cy="80"
          r="68"
          fill="none"
          stroke={ready ? "rgba(167,243,208,0.9)" : "rgba(255,255,255,0.72)"}
          strokeWidth="2"
          strokeLinecap="round"
          strokeDasharray={circumference}
          initial={false}
          animate={{ strokeDashoffset: circumference * (1 - progress) }}
          transition={{ duration: 0.55, ease: [0.22, 1, 0.36, 1] }}
        />
      </svg>

      <div className="relative flex size-[68%] flex-col items-center justify-center">
        <AnimatePresence mode="wait" initial={false}>
          <motion.div
            key={ready ? "ready-mark" : "listen-mark"}
            className="flex flex-col items-center"
            initial={{ opacity: 0, scale: 0.9 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.94 }}
            transition={{ duration: 0.26, ease: [0.22, 1, 0.36, 1] }}
          >
            {ready ? (
              <>
                <Check
                  className="size-7 text-emerald-100/90"
                  strokeWidth={1.8}
                />
                <span className="mt-1 text-[12px] font-medium text-emerald-100/65">
                  Saved
                </span>
              </>
            ) : (
              <>
                <Mic className="size-5 text-white/58" strokeWidth={1.7} />
                <span className="mt-1.5 text-[20px] font-semibold tracking-[-0.04em] text-white/94">
                  Hey Boris
                </span>
                <span
                  className="mt-1 flex h-2.5 items-center gap-0.5"
                  aria-hidden
                >
                  {Array.from({ length: 5 }, (_, index) => (
                    <span
                      key={index}
                      className={cn(
                        "teach-wave-bar h-1 w-0.5 rounded-full bg-white/45",
                        !listening &&
                          "[animation-play-state:paused] opacity-35",
                      )}
                      style={{ animationDelay: `${index * 90}ms` }}
                    />
                  ))}
                </span>
              </>
            )}
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}
