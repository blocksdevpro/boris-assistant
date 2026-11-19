import { useRef, useState, type ReactNode, type Ref } from "react";
import {
  AlertCircle,
  Check,
  ClipboardPaste,
  Cpu,
  ExternalLink,
  Eye,
  EyeOff,
  Feather,
  Gauge,
  Mic,
  ShieldCheck,
  Volume2,
  Zap,
} from "lucide-react";
import { motion, useReducedMotion, type Variants } from "framer-motion";
import { Toggle } from "@/components/settings";
import type { AppSettings, DeviceDto } from "@/bridge";
import { popSpring, settleSpring, softEase } from "@/lib/mainMotion";
import { cn } from "@/lib/utils";
import { KeyCheckLine } from "../KeyCheckLine";
import {
  RESIDENCY_OPTIONS,
  SYSTEM_DEFAULT_DEVICE,
  selectDeviceClass,
  shortDeviceName,
  type Residency,
} from "../mainWindowShared";
import type { KeyCheck } from "../useApiKeyCheck";

/* ── Shared motion ───────────────────────────────────────────────────────── */

/** Parent that reveals its <Item> children one after another. */
export function Stagger({
  children,
  className,
  delay = 0.06,
}: {
  children: ReactNode;
  className?: string;
  delay?: number;
}) {
  const reduce = useReducedMotion();
  const variants: Variants = {
    hidden: {},
    show: {
      transition: reduce
        ? { duration: 0 }
        : { staggerChildren: 0.07, delayChildren: delay },
    },
  };
  return (
    <motion.div variants={variants} initial="hidden" animate="show" className={className}>
      {children}
    </motion.div>
  );
}

export function Item({ children, className }: { children: ReactNode; className?: string }) {
  const reduce = useReducedMotion();
  const variants: Variants = reduce
    ? { hidden: { opacity: 0 }, show: { opacity: 1, transition: { duration: 0 } } }
    : {
        hidden: { opacity: 0, y: 12, filter: "blur(6px)" },
        show: {
          opacity: 1,
          y: 0,
          filter: "blur(0px)",
          transition: {
            ...settleSpring,
            opacity: { duration: 0.4, ease: softEase },
            filter: { duration: 0.45, ease: softEase },
          },
        },
      };
  return (
    <motion.div variants={variants} className={className}>
      {children}
    </motion.div>
  );
}

function StepHeading({
  eyebrow,
  title,
  children,
  headingRef,
}: {
  eyebrow?: string;
  title: string;
  children?: ReactNode;
  headingRef?: Ref<HTMLHeadingElement>;
}) {
  return (
    <>
      {eyebrow ? (
        <Item>
          <p className="text-center text-[11px] font-semibold uppercase tracking-[0.14em] text-[var(--boris-label-tertiary)]">
            {eyebrow}
          </p>
        </Item>
      ) : null}
      <Item>
        <h1
          ref={headingRef}
          tabIndex={-1}
          className="mt-2 text-balance text-center text-[26px] font-semibold tracking-[-0.035em] text-white outline-none sm:text-[28px]"
        >
          {title}
        </h1>
      </Item>
      {children ? (
        <Item>
          <p className="mx-auto mt-2.5 max-w-[30rem] text-pretty text-center text-[14px] leading-relaxed text-[var(--boris-label-secondary)]">
            {children}
          </p>
        </Item>
      ) : null}
    </>
  );
}

/* ── 1. Welcome ──────────────────────────────────────────────────────────── */

const WELCOME_WORDS = ["Hi,", "I'm", "Boris."];

const FEATURES = [
  { icon: Mic, title: "Just talk", text: "Say “Hey Boris” and ask." },
  { icon: Cpu, title: "Lives on your PC", text: "Notes, files, quick tasks." },
  { icon: ShieldCheck, title: "Asks first", text: "Nothing risky without a yes." },
];

export function WelcomeStep({ headingRef }: { headingRef: Ref<HTMLHeadingElement> }) {
  const reduce = useReducedMotion();
  return (
    <Stagger delay={0.1}>
      <h1
        ref={headingRef}
        tabIndex={-1}
        aria-label="Hi, I'm Boris."
        className="flex justify-center gap-[0.28em] text-[34px] font-semibold tracking-[-0.045em] text-white outline-none sm:text-[38px]"
      >
        {WELCOME_WORDS.map((word, i) => (
          <motion.span
            key={word}
            aria-hidden
            initial={reduce ? { opacity: 0 } : { opacity: 0, y: 14, filter: "blur(10px)" }}
            animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
            transition={
              reduce
                ? { duration: 0 }
                : { ...settleSpring, delay: 0.15 + i * 0.12, filter: { duration: 0.5, delay: 0.15 + i * 0.12 } }
            }
          >
            {word}
          </motion.span>
        ))}
      </h1>
      <Item>
        <p className="mx-auto mt-3 max-w-[27rem] text-pretty text-center text-[15px] leading-relaxed text-[var(--boris-label-secondary)]">
          Your voice assistant for Windows. Let's get you set up. It takes about two minutes.
        </p>
      </Item>
      <Item className="mx-auto mt-7 grid max-w-[34rem] grid-cols-3 gap-2.5">
        {FEATURES.map(({ icon: Icon, title, text }, i) => (
          <motion.div
            key={title}
            initial={reduce ? { opacity: 0 } : { opacity: 0, y: 10, scale: 0.97 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            transition={reduce ? { duration: 0 } : { ...settleSpring, delay: 0.55 + i * 0.08 }}
            className="rounded-[16px] border border-white/[0.06] bg-white/[0.035] px-3 py-3.5 text-center"
          >
            <span className="mx-auto flex size-8 items-center justify-center rounded-full bg-white/[0.07]">
              <Icon className="size-4 text-white/75" strokeWidth={1.9} />
            </span>
            <p className="mt-2.5 text-[13px] font-medium text-white/90">{title}</p>
            <p className="mt-0.5 text-[11.5px] leading-snug text-[var(--boris-label-tertiary)]">{text}</p>
          </motion.div>
        ))}
      </Item>
    </Stagger>
  );
}

/* ── 2. Connect (OpenRouter key) ─────────────────────────────────────────── */

export function ConnectStep({
  headingRef,
  apiKey,
  keyCheck,
  onKeyChange,
  onOpenExternal,
}: {
  headingRef: Ref<HTMLHeadingElement>;
  apiKey: string;
  keyCheck: KeyCheck;
  onKeyChange: (key: string) => void;
  onOpenExternal: (url: string) => void;
}) {
  const [shown, setShown] = useState(false);
  const [pasteHint, setPasteHint] = useState(false);
  const input = useRef<HTMLInputElement>(null);

  const paste = async () => {
    try {
      const text = (await navigator.clipboard.readText()).trim();
      if (text) {
        onKeyChange(text);
        setPasteHint(false);
        return;
      }
    } catch {
      // Clipboard read blocked: fall back to Ctrl+V in the field.
    }
    setPasteHint(true);
    input.current?.focus();
  };

  const steps: { text: ReactNode }[] = [
    {
      text: (
        <>
          Sign in to OpenRouter.{" "}
          <button
            type="button"
            onClick={() => onOpenExternal("https://openrouter.ai/keys")}
            className="inline-flex items-center gap-1 rounded text-sky-300/85 underline-offset-2 hover:text-sky-200 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-300/40"
          >
            Open OpenRouter <ExternalLink className="size-3" />
          </button>
        </>
      ),
    },
    { text: "Add a little credit. You only pay for what Boris uses." },
    { text: "Create a key, copy it, and paste it below." },
  ];

  return (
    <Stagger>
      <StepHeading eyebrow="Step 1 of 4" title="Connect Boris's brain" headingRef={headingRef}>
        Boris uses OpenRouter to understand you and come up with answers.
      </StepHeading>

      <Item className="mx-auto mt-6 w-full max-w-[28rem]">
        <ol className="flex flex-col gap-2.5">
          {steps.map((step, i) => (
            <li key={i} className="flex items-start gap-3 text-[13px] leading-snug text-white/75">
              <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-white/[0.08] text-[11px] font-semibold text-white/70">
                {i + 1}
              </span>
              <span className="pt-px">{step.text}</span>
            </li>
          ))}
        </ol>
      </Item>

      <Item className="mx-auto mt-5 w-full max-w-[28rem]">
        <label htmlFor="onboarding-key" className="sr-only">
          OpenRouter API key
        </label>
        <div
          className={cn(
            "flex items-center gap-1 rounded-[14px] border bg-white/[0.05] p-1 pl-3.5 transition-[border-color,box-shadow] duration-200",
            "focus-within:border-white/15 focus-within:shadow-[0_0_0_4px_rgba(255,255,255,0.04)]",
            keyCheck.state === "ok"
              ? "border-emerald-300/30"
              : keyCheck.state === "rejected"
                ? "border-red-300/30"
                : "border-white/[0.08]",
          )}
        >
          <input
            ref={input}
            id="onboarding-key"
            type={shown ? "text" : "password"}
            value={apiKey}
            autoComplete="off"
            spellCheck={false}
            placeholder="sk-or-v1-…"
            onChange={(e) => onKeyChange(e.target.value)}
            className="h-10 min-w-0 flex-1 bg-transparent font-mono text-[13.5px] text-white outline-none placeholder:font-sans placeholder:text-white/30"
          />
          <button
            type="button"
            onClick={() => setShown((v) => !v)}
            aria-label={shown ? "Hide key" : "Show key"}
            className="inline-flex size-9 items-center justify-center rounded-[10px] text-white/45 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
          >
            {shown ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
          </button>
          <button
            type="button"
            onClick={() => void paste()}
            className="inline-flex h-9 items-center gap-1.5 rounded-[10px] bg-white/[0.09] px-3 text-[12.5px] font-medium text-white/85 transition-colors hover:bg-white/[0.14] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
          >
            <ClipboardPaste className="size-3.5" /> Paste
          </button>
        </div>
        <div className="mt-2 px-1">
          {pasteHint && !apiKey ? (
            <p className="text-[12.5px] text-[var(--boris-label-tertiary)]">
              Click the box above and press Ctrl+V.
            </p>
          ) : (
            <KeyCheckLine check={keyCheck} />
          )}
        </div>
      </Item>
    </Stagger>
  );
}

/* ── 3. Speed vs memory ──────────────────────────────────────────────────── */

const RESIDENCY_ICONS: Record<Residency, typeof Zap> = {
  low_latency: Zap,
  balanced: Gauge,
  low_memory: Feather,
};

export function SpeedStep({
  headingRef,
  value,
  recommended,
  onChange,
}: {
  headingRef: Ref<HTMLHeadingElement>;
  value: Residency;
  recommended: Residency;
  onChange: (id: Residency) => void;
}) {
  const reduce = useReducedMotion();
  return (
    <Stagger>
      <StepHeading eyebrow="Step 2 of 4" title="Fast replies, or a lighter PC?" headingRef={headingRef}>
        Boris can keep his voice ready so he answers instantly, or free up memory for
        your other apps. You can change this later.
      </StepHeading>

      <Item className="mx-auto mt-7 w-full max-w-[38rem]">
        <div role="radiogroup" aria-label="Speed or memory" className="grid grid-cols-3 gap-3">
          {RESIDENCY_OPTIONS.map((option) => {
            const Icon = RESIDENCY_ICONS[option.id];
            const active = option.id === value;
            return (
              <motion.button
                key={option.id}
                type="button"
                role="radio"
                aria-checked={active}
                onClick={() => onChange(option.id)}
                whileHover={reduce ? undefined : { y: -3 }}
                whileTap={reduce ? undefined : { scale: 0.975 }}
                transition={settleSpring}
                className={cn(
                  "relative flex flex-col items-start rounded-[18px] border px-4 pb-4 pt-4 text-left transition-colors duration-200",
                  "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25",
                  active
                    ? "border-white/20"
                    : "border-white/[0.06] bg-white/[0.025] hover:border-white/[0.1]",
                )}
              >
                {active ? (
                  <motion.span
                    layoutId="onboarding-speed-active"
                    aria-hidden
                    className="absolute inset-0 rounded-[18px] bg-white/[0.08] shadow-[0_1px_0_rgba(255,255,255,0.06)_inset,0_18px_40px_rgba(0,0,0,0.25)]"
                    transition={reduce ? { duration: 0 } : { type: "spring", stiffness: 380, damping: 34 }}
                  />
                ) : null}
                <span className="relative flex w-full items-start justify-between">
                  <span
                    className={cn(
                      "flex size-9 items-center justify-center rounded-[11px] transition-colors duration-200",
                      active ? "bg-white text-[#101216]" : "bg-white/[0.07] text-white/70",
                    )}
                  >
                    <Icon className="size-[18px]" strokeWidth={2} />
                  </span>
                  <span
                    className={cn(
                      "flex size-5 items-center justify-center rounded-full border transition-colors duration-200",
                      active ? "border-sky-300 bg-sky-300" : "border-white/20",
                    )}
                  >
                    {active ? (
                      <motion.span
                        initial={reduce ? false : { scale: 0.3, opacity: 0 }}
                        animate={{ scale: 1, opacity: 1 }}
                        transition={reduce ? { duration: 0 } : popSpring}
                      >
                        <Check className="size-3 text-[#101216]" strokeWidth={3} />
                      </motion.span>
                    ) : null}
                  </span>
                </span>
                <span className="relative mt-4 text-[16px] font-semibold tracking-[-0.02em] text-white">
                  {option.label}
                </span>
                <span className="relative mt-0.5 text-[12.5px] font-medium text-white/70">
                  {option.tagline}
                </span>
                <span className="relative mt-2 text-[12px] leading-snug text-[var(--boris-label-tertiary)]">
                  {option.detail}
                </span>
                {option.id === recommended ? (
                  <span className="relative mt-3 rounded-full bg-emerald-300/12 px-2 py-0.5 text-[10.5px] font-semibold uppercase tracking-[0.08em] text-emerald-200/90">
                    Best for this PC
                  </span>
                ) : null}
              </motion.button>
            );
          })}
        </div>
      </Item>
    </Stagger>
  );
}

/* ── 4. Microphone + speakers ────────────────────────────────────────────── */

export function DevicesStep({
  headingRef,
  inputs,
  outputs,
  selectedInput,
  selectedOutput,
  onInputChange,
  onOutputChange,
}: {
  headingRef: Ref<HTMLHeadingElement>;
  inputs: DeviceDto[];
  outputs: DeviceDto[];
  selectedInput: string;
  selectedOutput: string;
  onInputChange: (id: string) => void;
  onOutputChange: (id: string) => void;
}) {
  const reduce = useReducedMotion();
  const defaultMic = inputs.find((d) => d.is_default);
  const defaultSpeaker = outputs.find((d) => d.is_default);
  const mics = [
    {
      id: SYSTEM_DEFAULT_DEVICE,
      name: "Same as Windows",
      hint: defaultMic ? shortDeviceName(defaultMic.name) : "Your default microphone",
    },
    ...inputs.map((d) => ({
      id: d.id,
      name: shortDeviceName(d.name),
      hint: d.is_default ? "Windows default right now" : "",
    })),
  ];

  return (
    <Stagger>
      <StepHeading eyebrow="Step 3 of 4" title="Which microphone will you use?" headingRef={headingRef}>
        Pick the one you'll talk into. Not sure? Keep “Same as Windows”.
      </StepHeading>

      <Item className="mx-auto mt-6 w-full max-w-[28rem]">
        <div
          role="radiogroup"
          aria-label="Microphone"
          className="flex max-h-[15.5rem] flex-col gap-1.5 overflow-y-auto pr-1"
        >
          {mics.map((mic, i) => {
            const active = mic.id === selectedInput;
            return (
              <motion.button
                key={mic.id}
                type="button"
                role="radio"
                aria-checked={active}
                onClick={() => onInputChange(mic.id)}
                initial={reduce ? false : { opacity: 0, x: -8 }}
                animate={{ opacity: 1, x: 0 }}
                transition={reduce ? { duration: 0 } : { ...settleSpring, delay: 0.18 + i * 0.045 }}
                className={cn(
                  "relative flex min-h-[52px] items-center gap-3 rounded-[14px] border px-3.5 py-2.5 text-left transition-colors duration-150",
                  "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25",
                  active
                    ? "border-white/18 bg-white/[0.08]"
                    : "border-white/[0.05] bg-white/[0.02] hover:bg-white/[0.045]",
                )}
              >
                <span
                  className={cn(
                    "flex size-8 shrink-0 items-center justify-center rounded-full transition-colors duration-150",
                    active ? "bg-white text-[#101216]" : "bg-white/[0.07] text-white/60",
                  )}
                >
                  <Mic className="size-4" strokeWidth={2} />
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13.5px] font-medium text-white/90">
                    {mic.name}
                  </span>
                  {mic.hint ? (
                    <span className="block truncate text-[11.5px] text-[var(--boris-label-tertiary)]">
                      {mic.hint}
                    </span>
                  ) : null}
                </span>
                {active ? (
                  <motion.span
                    initial={reduce ? false : { scale: 0.3, opacity: 0 }}
                    animate={{ scale: 1, opacity: 1 }}
                    transition={reduce ? { duration: 0 } : popSpring}
                    className="flex size-5 items-center justify-center rounded-full bg-sky-300"
                  >
                    <Check className="size-3 text-[#101216]" strokeWidth={3} />
                  </motion.span>
                ) : null}
              </motion.button>
            );
          })}
        </div>
      </Item>

      <Item className="mx-auto mt-4 flex w-full max-w-[28rem] items-center gap-3 rounded-[14px] border border-white/[0.05] bg-white/[0.02] px-3.5 py-2.5">
        <Volume2 className="size-4 shrink-0 text-white/50" strokeWidth={2} />
        <label htmlFor="onboarding-speaker" className="shrink-0 text-[13px] text-white/70">
          Speakers
        </label>
        <select
          id="onboarding-speaker"
          value={selectedOutput}
          onChange={(e) => onOutputChange(e.target.value)}
          className={cn(selectDeviceClass, "h-9")}
        >
          <option value={SYSTEM_DEFAULT_DEVICE} className="bg-[#1c1c1e] text-white">
            Same as Windows
            {defaultSpeaker ? ` · ${shortDeviceName(defaultSpeaker.name)}` : ""}
          </option>
          {outputs.map((d) => (
            <option key={d.id} value={d.id} className="bg-[#1c1c1e] text-white">
              {shortDeviceName(d.name)}
            </option>
          ))}
        </select>
      </Item>
    </Stagger>
  );
}

/* ── 5. Ready ────────────────────────────────────────────────────────────── */

const TRY_SAYING = [
  "Hey Boris, what can you do?",
  "Hey Boris, take a note: call the dentist",
  "Hey Boris, what's in the news today?",
];

export type VoiceDownload =
  | { state: "checking" }
  | { state: "downloading"; percent: number | null; label: string }
  | { state: "failed"; message: string }
  | { state: "ready" };

export function ReadyStep({
  headingRef,
  download,
  settings,
  onPatch,
  onRetry,
}: {
  headingRef: Ref<HTMLHeadingElement>;
  download: VoiceDownload;
  settings: AppSettings;
  onPatch: (p: Partial<AppSettings>) => void;
  onRetry: () => void;
}) {
  const reduce = useReducedMotion();

  if (download.state !== "ready") {
    const failed = download.state === "failed";
    return (
      <Stagger key="waiting">
        <StepHeading
          eyebrow="Step 4 of 4"
          title={failed ? "The download didn't finish" : "Getting Boris's voice ready"}
          headingRef={headingRef}
        >
          {failed
            ? "Check your internet connection, then try again. Anything already downloaded is kept."
            : "Boris listens and speaks right here on your PC, so he needs a one-time download of about 900 MB."}
        </StepHeading>
        <Item className="mx-auto mt-7 w-full max-w-[24rem]">
          {failed ? (
            <div className="flex flex-col items-center gap-3">
              <p className="flex items-start gap-2 text-[12.5px] leading-snug text-red-200/85">
                <AlertCircle className="mt-px size-3.5 shrink-0" />
                <span className="line-clamp-3">{download.message}</span>
              </p>
              <button
                type="button"
                onClick={onRetry}
                className="h-9 rounded-full bg-white/[0.1] px-4 text-[13px] font-medium text-white hover:bg-white/[0.15] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
              >
                Try again
              </button>
            </div>
          ) : (
            <DownloadBar download={download} />
          )}
        </Item>
      </Stagger>
    );
  }

  return (
    <Stagger key="ready">
      <StepHeading eyebrow="All done" title="You're all set" headingRef={headingRef}>
        Say “Hey Boris”, then ask your question. Try one of these:
      </StepHeading>
      <Item className="mx-auto mt-5 flex w-full max-w-[26rem] flex-col gap-2">
        {TRY_SAYING.map((line, i) => (
          <motion.p
            key={line}
            initial={reduce ? { opacity: 0 } : { opacity: 0, y: 8, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            transition={reduce ? { duration: 0 } : { ...settleSpring, delay: 0.3 + i * 0.1 }}
            className="rounded-[14px] border border-white/[0.06] bg-white/[0.035] px-4 py-2.5 text-center text-[13.5px] text-white/85"
          >
            “{line}”
          </motion.p>
        ))}
      </Item>
      <Item className="mx-auto mt-5 flex w-full max-w-[26rem] items-center justify-between gap-3 rounded-[14px] border border-white/[0.05] px-4 py-3">
        <span>
          <label htmlFor="onboarding-autostart" className="block text-[13.5px] font-medium text-white/90">
            Start Boris with Windows
          </label>
          <span className="block text-[12px] text-[var(--boris-label-tertiary)]">
            He'll be ready whenever you are
          </span>
        </span>
        <Toggle
          id="onboarding-autostart"
          checked={settings.start_with_windows}
          onChange={(v) => onPatch({ start_with_windows: v })}
          aria-label="Start Boris with Windows"
        />
      </Item>
    </Stagger>
  );
}

function DownloadBar({ download }: { download: VoiceDownload }) {
  const reduce = useReducedMotion();
  const percent = download.state === "downloading" ? download.percent : null;
  const label =
    download.state === "downloading" ? download.label : "Checking what's needed…";
  return (
    <div role="status" aria-live="polite">
      <div
        className="relative h-2 overflow-hidden rounded-full bg-white/[0.08]"
        role="progressbar"
        aria-label="Voice download"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent ?? undefined}
      >
        <motion.div
          className="absolute inset-y-0 left-0 overflow-hidden rounded-full bg-white"
          initial={false}
          animate={{ width: `${Math.max(4, percent ?? 4)}%` }}
          transition={reduce ? { duration: 0 } : { duration: 0.5, ease: softEase }}
        >
          <span className="onboarding__sheen" aria-hidden />
        </motion.div>
      </div>
      <div className="mt-2.5 flex items-center justify-between gap-3 text-[12.5px]">
        <span className="truncate text-[var(--boris-label-secondary)]">{label}</span>
        {percent != null ? (
          <span className="shrink-0 font-medium tabular-nums text-white/80">{percent}%</span>
        ) : null}
      </div>
    </div>
  );
}
