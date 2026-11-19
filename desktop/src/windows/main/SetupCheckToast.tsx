import { useEffect, useState } from "react";
import { ChevronDown, Copy, X } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  openMicPrivacySettings,
  type ApiKeyStatus,
  type SelfCheckReport,
} from "@/bridge";
import { logger } from "@/lib/logger";
import { makeRoomMotion, popSpring, settleSpring, softEase, swapMotion } from "@/lib/mainMotion";
import type { SettingsCategory } from "./mainWindowShared";

/**
 * The once-per-install setup check, as a small floating toast.
 *
 * One ring with a segment per link in the voice chain fills as each check
 * lands; one line of text says what is being tested. A full pass closes the
 * ring into a check and the toast leaves on its own. A failure keeps it up
 * with the one thing to fix. The per-check list exists only behind Details.
 * Every fix button does what it says and reports back here: a model repair
 * shows its progress, then the check runs again on its own.
 */

type Check = { id: string; label: string; testing: string };

/** Order matches the engine's step order; the key check runs alongside. */
const CHECKS: Check[] = [
  { id: "microphone", label: "Microphone", testing: "Listening to your mic" },
  { id: "speaker", label: "Speaker", testing: "Checking your speaker" },
  { id: "voice", label: "Boris's voice", testing: "Warming up Boris's voice" },
  { id: "wake", label: "Wake word", testing: "Testing “Hey Boris”" },
  { id: "speech_detect", label: "Speech detection", testing: "Testing speech detection" },
  { id: "transcribe", label: "Speech-to-text", testing: "Testing speech-to-text" },
  { id: "key", label: "OpenRouter key", testing: "Checking your OpenRouter key" },
];

const MODEL_STEPS = new Set(["voice", "wake", "speech_detect", "transcribe"]);
/** A pass stays readable for this long, then the toast leaves. */
const SUCCESS_DISMISS_MS = 4200;

type Result = { ok: boolean | null; detail: string };

function results(report: SelfCheckReport | null, key: ApiKeyStatus | null) {
  const steps = new Map((report?.steps ?? []).map((s) => [s.id, s]));
  return new Map<string, Result>(
    CHECKS.map(({ id }) => {
      if (id === "key") {
        return [id, key ? { ok: key.state === "ok", detail: key.detail } : { ok: null, detail: "" }];
      }
      const s = steps.get(id);
      return [id, s ? { ok: s.ok, detail: s.detail } : { ok: null, detail: "" }];
    }),
  );
}

/** Plain-text report a friend can paste back to you. */
export function setupReportText(
  report: SelfCheckReport | null,
  key: ApiKeyStatus | null,
  appVersion: string,
): string {
  const r = results(report, key);
  return [
    `Boris setup check, ${new Date().toISOString()}`,
    `App ${appVersion || "unknown"}, ${navigator.userAgent}`,
    ...CHECKS.map(({ id, label }) => {
      const x = r.get(id)!;
      return `${x.ok == null ? "...." : x.ok ? "OK  " : "FAIL"} ${label}: ${x.detail || "not run"}`;
    }),
  ].join("\n");
}

export function SetupCheckToast({
  report,
  keyStatus,
  appVersion,
  waiting,
  repairLabel,
  onRunAgain,
  onRetryKey,
  onRepairModels,
  onDismiss,
  onOpenSettings,
}: {
  report: SelfCheckReport | null;
  keyStatus: ApiKeyStatus | null;
  appVersion: string;
  /** Requested mid-turn; starts when Boris finishes. */
  waiting: boolean;
  /** Model repair in progress (one line of progress), else null. */
  repairLabel: string | null;
  onRunAgain: () => void;
  onRetryKey: () => void;
  onRepairModels: () => void;
  onDismiss: () => void;
  onOpenSettings: (category: SettingsCategory) => void;
}) {
  const reduced = Boolean(useReducedMotion());
  const r = results(report, keyStatus);
  const repairing = repairLabel != null;
  const running = !repairing && (report == null || report.running || keyStatus == null);
  const failed = CHECKS.filter(({ id }) => r.get(id)!.ok === false);
  const passed = !running && failed.length === 0;
  const current = CHECKS.find(({ id }) => r.get(id)!.ok == null);
  const [details, setDetails] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!passed) return;
    const timer = setTimeout(onDismiss, SUCCESS_DISMISS_MS);
    return () => clearTimeout(timer);
  }, [passed, onDismiss]);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1600);
    return () => clearTimeout(timer);
  }, [copied]);

  const firstFail = failed[0];
  const title = repairing
    ? "Repairing Boris's voice"
    : running
    ? "Checking Boris"
    : passed
      ? "Boris is ready"
      : failed.length === 1
        ? `${firstFail!.label} needs attention`
        : `${failed.length} checks need attention`;
  const subtitle = repairing
    ? repairLabel
    : waiting
    ? "Waiting for Boris to finish…"
    : running
    ? `${current?.testing ?? "Finishing up"}…`
    : passed
      ? "Say “Hey Boris” to try it."
      : r.get(firstFail!.id)!.detail;

  const fix = firstFail
    ? fixAction(firstFail.id, keyStatus, { onOpenSettings, onRetryKey, onRepairModels })
    : null;

  const copy = () => {
    void navigator.clipboard
      .writeText(setupReportText(report, keyStatus, appVersion))
      .then(() => setCopied(true))
      .catch((e) => logger.warn("copy setup report failed", String(e)));
  };

  return (
    <motion.div
      role="status"
      aria-live="polite"
      className="setup-toast fixed bottom-5 right-5 z-50 w-[320px] origin-bottom-right"
      initial={reduced ? { opacity: 0 } : { opacity: 0, y: 18, scale: 0.96, filter: "blur(8px)" }}
      animate={reduced ? { opacity: 1 } : { opacity: 1, y: 0, scale: 1, filter: "blur(0px)" }}
      exit={
        reduced
          ? { opacity: 0, transition: { duration: 0 } }
          : { opacity: 0, y: 12, scale: 0.98, filter: "blur(4px)", transition: { duration: 0.22, ease: softEase } }
      }
      transition={
        reduced
          ? { duration: 0 }
          : { ...settleSpring, opacity: { duration: 0.32, ease: softEase }, filter: { duration: 0.4, ease: softEase } }
      }
    >
      <div className="flex items-start gap-3 px-4 py-3.5">
        <ChainRing
          results={CHECKS.map(({ id }) => r.get(id)!.ok)}
          current={repairing ? undefined : current?.id}
          passed={passed}
          reduced={reduced}
        />

        <div className="min-w-0 flex-1 pt-[1px]">
          <AnimatePresence mode="wait" initial={false}>
            <motion.p
              key={title}
              {...swapMotion(reduced)}
              className="truncate text-[14px] font-semibold tracking-[-0.01em] text-white/92"
            >
              {title}
            </motion.p>
          </AnimatePresence>
          <AnimatePresence mode="wait" initial={false}>
            <motion.p
              key={subtitle}
              {...swapMotion(reduced)}
              className="mt-0.5 line-clamp-2 text-[12.5px] leading-snug text-[var(--boris-label-secondary)]"
            >
              {subtitle}
            </motion.p>
          </AnimatePresence>
        </div>

        <button
          type="button"
          aria-label="Close setup check"
          onClick={onDismiss}
          className="-mr-1 -mt-0.5 rounded-full p-1 text-white/35 transition-colors duration-200 hover:bg-white/[0.08] hover:text-white/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
        >
          <X className="size-3.5" />
        </button>
      </div>

      <AnimatePresence initial={false}>
        {!running && !passed && !repairing ? (
          <motion.div key="actions" {...makeRoomMotion(reduced)}>
            <div className="flex items-center gap-2 px-4 pb-3.5 pl-[60px]">
              {fix ? (
                <motion.button
                  type="button"
                  onClick={fix.run}
                  whileTap={reduced ? undefined : { scale: 0.95 }}
                  className="h-7 rounded-full bg-white px-3 text-[12px] font-medium text-[#0b0b0c] transition-colors duration-200 hover:bg-white/90"
                >
                  {fix.label}
                </motion.button>
              ) : null}
              <button
                type="button"
                onClick={() => setDetails((d) => !d)}
                aria-expanded={details}
                className="inline-flex h-7 items-center gap-1 rounded-full px-2.5 text-[12px] text-[var(--boris-label-secondary)] transition-colors duration-200 hover:bg-white/[0.07] hover:text-white/80"
              >
                Details
                <motion.span
                  animate={{ rotate: details ? 180 : 0 }}
                  transition={reduced ? { duration: 0 } : { duration: 0.26, ease: softEase }}
                  className="inline-flex"
                >
                  <ChevronDown className="size-3.5" />
                </motion.span>
              </button>
            </div>
          </motion.div>
        ) : null}
      </AnimatePresence>

      <AnimatePresence initial={false}>
        {details && !running && !passed && !repairing ? (
          <motion.div key="details" {...makeRoomMotion(reduced)}>
            <div className="border-t border-white/[0.06] px-4 pb-3.5 pt-3">
              <ul className="space-y-1.5">
                {CHECKS.map(({ id, label }) => {
                  const x = r.get(id)!;
                  return (
                    <li key={id} className="flex items-start gap-2 text-[12px] leading-snug">
                      <span
                        aria-hidden="true"
                        className="mt-[5px] size-1.5 shrink-0 rounded-full"
                        style={{
                          background: x.ok ? "var(--boris-green)" : x.ok === false ? "var(--boris-red)" : "rgba(255,255,255,0.25)",
                        }}
                      />
                      <span className="min-w-0">
                        <span className="text-white/80">{label}</span>
                        <span className="ml-1.5 text-[var(--boris-label-tertiary)]">{x.detail || "Not run"}</span>
                      </span>
                    </li>
                  );
                })}
              </ul>
              <div className="mt-3 flex gap-2">
                <button
                  type="button"
                  onClick={onRunAgain}
                  className="h-7 rounded-full bg-white/10 px-3 text-[12px] text-white transition-colors duration-200 hover:bg-white/15"
                >
                  Run again
                </button>
                <button
                  type="button"
                  onClick={copy}
                  className="inline-flex h-7 items-center gap-1.5 rounded-full px-2.5 text-[12px] text-[var(--boris-label-secondary)] transition-colors duration-200 hover:bg-white/[0.07] hover:text-white/80"
                >
                  <Copy className="size-3" />
                  <AnimatePresence mode="wait" initial={false}>
                    <motion.span key={copied ? "y" : "n"} {...swapMotion(reduced)}>
                      {copied ? "Copied" : "Copy report"}
                    </motion.span>
                  </AnimatePresence>
                </button>
              </div>
            </div>
          </motion.div>
        ) : null}
      </AnimatePresence>
    </motion.div>
  );
}

function fixAction(
  id: string,
  key: ApiKeyStatus | null,
  {
    onOpenSettings,
    onRetryKey,
    onRepairModels,
  }: {
    onOpenSettings: (category: SettingsCategory) => void;
    onRetryKey: () => void;
    onRepairModels: () => void;
  },
): { label: string; run: () => void } | null {
  if (id === "microphone") {
    return {
      label: "Open mic settings",
      run: () =>
        void openMicPrivacySettings().catch((e) => {
          logger.error("openMicPrivacySettings failed", String(e));
          onOpenSettings("speech");
        }),
    };
  }
  if (id === "speaker") return { label: "Choose speaker", run: () => onOpenSettings("speech") };
  if (MODEL_STEPS.has(id)) return { label: "Repair voice models", run: onRepairModels };
  if (id === "key") {
    if (key?.state === "no_credit") {
      return {
        label: "Add credit",
        run: () =>
          void openUrl("https://openrouter.ai/settings/credits").catch(() => onOpenSettings("connections")),
      };
    }
    if (key?.state === "unreachable") return { label: "Check key again", run: onRetryKey };
    return { label: "Fix key", run: () => onOpenSettings("connections") };
  }
  return null;
}

/* ── The ring ─────────────────────────────────────────────────────────── */

const RING = 36;
const R = 15;
const GAP_DEG = 9;

function arcPath(index: number, count: number): string {
  const span = 360 / count;
  const start = index * span - 90 + GAP_DEG / 2;
  const end = start + span - GAP_DEG;
  const pt = (deg: number) => {
    const rad = (deg * Math.PI) / 180;
    return `${RING / 2 + R * Math.cos(rad)} ${RING / 2 + R * Math.sin(rad)}`;
  };
  return `M ${pt(start)} A ${R} ${R} 0 0 1 ${pt(end)}`;
}

/**
 * One segment per check. A landed check draws its segment green (or red);
 * the segment under test breathes. On a full pass the ring hands over to a
 * filled disc with a drawn check.
 */
function ChainRing({
  results,
  current,
  passed,
  reduced,
}: {
  results: (boolean | null)[];
  current: string | undefined;
  passed: boolean;
  reduced: boolean;
}) {
  const currentIndex = CHECKS.findIndex((c) => c.id === current);
  return (
    <div className="relative size-9 shrink-0" aria-hidden="true">
      <AnimatePresence initial={false}>
        {passed ? (
          <motion.svg
            key="done"
            viewBox={`0 0 ${RING} ${RING}`}
            className="absolute inset-0"
            initial={reduced ? { opacity: 0 } : { opacity: 0, scale: 0.6 }}
            animate={{ opacity: 1, scale: 1 }}
            transition={reduced ? { duration: 0 } : { ...popSpring, delay: 0.12 }}
          >
            <circle cx={RING / 2} cy={RING / 2} r={R + 1.5} fill="color-mix(in srgb, var(--boris-green) 18%, transparent)" />
            <circle cx={RING / 2} cy={RING / 2} r={R + 1.5} fill="none" stroke="var(--boris-green)" strokeOpacity="0.55" strokeWidth="1.2" />
            <motion.path
              d="M11.5 18.4 L16 22.6 L24.8 13.4"
              fill="none"
              stroke="var(--boris-green)"
              strokeWidth="2.4"
              strokeLinecap="round"
              strokeLinejoin="round"
              initial={reduced ? false : { pathLength: 0 }}
              animate={{ pathLength: 1 }}
              transition={reduced ? { duration: 0 } : { duration: 0.42, ease: softEase, delay: 0.3 }}
            />
          </motion.svg>
        ) : (
          <motion.svg
            key="ring"
            viewBox={`0 0 ${RING} ${RING}`}
            className="absolute inset-0"
            exit={reduced ? { opacity: 0 } : { opacity: 0, scale: 1.12, transition: { duration: 0.22, ease: softEase } }}
          >
            {results.map((ok, i) => (
              <g key={i}>
                <motion.path
                  d={arcPath(i, results.length)}
                  fill="none"
                  stroke="white"
                  strokeWidth="3"
                  strokeLinecap="round"
                  animate={{
                    strokeOpacity:
                      i === currentIndex && !reduced ? [0.1, 0.32, 0.1] : 0.1,
                  }}
                  transition={
                    i === currentIndex && !reduced
                      ? { duration: 1.4, ease: "easeInOut", repeat: Infinity }
                      : { duration: 0.2 }
                  }
                />
                {ok != null ? (
                  <motion.path
                    d={arcPath(i, results.length)}
                    fill="none"
                    stroke={ok ? "var(--boris-green)" : "var(--boris-red)"}
                    strokeWidth="3"
                    strokeLinecap="round"
                    initial={reduced ? false : { pathLength: 0, opacity: 0 }}
                    animate={{ pathLength: 1, opacity: 1 }}
                    transition={reduced ? { duration: 0 } : { duration: 0.34, ease: softEase }}
                  />
                ) : null}
              </g>
            ))}
          </motion.svg>
        )}
      </AnimatePresence>
    </div>
  );
}
