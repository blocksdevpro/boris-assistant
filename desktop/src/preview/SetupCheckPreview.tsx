import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence } from "framer-motion";
import {
  OFF_STATUS,
  type ApiKeyStatus,
  type ListenHint,
  type SelfCheckReport,
  type SelfCheckStep,
  type StatusPicture,
} from "@/bridge";
import { SetupCheckToast } from "@/windows/main/SetupCheckToast";
import { ListenHintPill } from "@/windows/main/ListenHintPill";

/**
 * Dev-only motion lab for the setup toast and listen pill
 * (`?preview=setup-check`). Plays a scripted check so transitions can be
 * watched without a running engine.
 */

const STEPS: SelfCheckStep[] = [
  { id: "microphone", ok: true, detail: "Hearing Microphone (USB headset)", ms: 1510 },
  { id: "speaker", ok: true, detail: "Ready on Speakers (USB headset)", ms: 0 },
  { id: "voice", ok: true, detail: "Made 2.0 s of speech", ms: 2025 },
  { id: "wake", ok: true, detail: "Heard “Hey Boris” (score 0.98)", ms: 837 },
  { id: "speech_detect", ok: true, detail: "Found 1344 ms of speech", ms: 24 },
  { id: "transcribe", ok: true, detail: "Heard “Hey Boris, what time is it?”", ms: 2370 },
];
const STEP_INTERVAL_MS = 650;

type Scenario = "pass" | "mic-fail" | "model-fail";

export default function SetupCheckPreview() {
  const [open, setOpen] = useState(true);
  const [report, setReport] = useState<SelfCheckReport | null>(null);
  const [keyStatus, setKeyStatus] = useState<ApiKeyStatus | null>(null);
  const [hint, setHint] = useState<ListenHint | null>(null);
  const [waiting, setWaiting] = useState(false);
  const [repairLabel, setRepairLabel] = useState<string | null>(null);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);
  const dismiss = useCallback(() => setOpen(false), []);

  const play = useCallback((scenario: Scenario) => {
    timers.current.forEach(clearTimeout);
    timers.current = [];
    setOpen(true);
    setReport(null);
    setKeyStatus(null);
    setWaiting(false);
    setRepairLabel(null);
    const steps = STEPS.map((s) =>
      scenario === "mic-fail" && s.id === "microphone"
        ? {
            ...s,
            ok: false,
            detail: "Only silence from Microphone (Camo): blocked by Windows privacy, muted, or the wrong mic",
          }
        : scenario === "model-fail" && s.id === "transcribe"
          ? { ...s, ok: false, detail: "Speech-to-text model failed to load: invalid protobuf" }
          : s,
    );
    const at = (ms: number, fn: () => void) => timers.current.push(setTimeout(fn, ms));
    at(300, () => setReport({ running: true, ok: false, steps: [] }));
    steps.forEach((_, i) =>
      at(300 + (i + 1) * STEP_INTERVAL_MS, () =>
        setReport({ running: true, ok: false, steps: steps.slice(0, i + 1) }),
      ),
    );
    at(300 + (steps.length + 1) * STEP_INTERVAL_MS, () =>
      setKeyStatus({ state: "ok", detail: "Key works, $1.00 left on this key" }),
    );
    at(300 + (steps.length + 1) * STEP_INTERVAL_MS + 80, () =>
      setReport({ running: false, ok: steps.every((s) => s.ok), steps }),
    );
  }, []);

  /** Busy engine: the toast waits, then the check runs. */
  const playWaiting = useCallback(() => {
    timers.current.forEach(clearTimeout);
    timers.current = [];
    setOpen(true);
    setReport(null);
    setKeyStatus(null);
    setWaiting(true);
    timers.current.push(setTimeout(() => play("pass"), 2400));
  }, [play]);

  /** Scripted repair: progress lines, then the check runs again and passes. */
  const playRepair = useCallback(() => {
    timers.current.forEach(clearTimeout);
    const lines = [
      "Stopping Boris…",
      "Checking encoder-model.int8.onnx…",
      "Downloading encoder-model.int8.onnx · 12%",
      "Downloading encoder-model.int8.onnx · 58%",
      "Downloading encoder-model.int8.onnx · 100%",
    ];
    timers.current = lines.map((line, i) => setTimeout(() => setRepairLabel(line), i * 700));
    timers.current.push(setTimeout(() => play("pass"), lines.length * 700));
  }, [play]);

  useEffect(() => {
    play("pass");
    const pending = timers.current;
    return () => pending.forEach(clearTimeout);
  }, [play]);

  const status: StatusPicture = {
    ...OFF_STATUS,
    engine: "On",
    phase: "Armed",
    mic: { label: "Microphone (Razer BlackShark V2 X USB)", ok: true },
    listen_hint: hint,
  };

  const button = "h-8 rounded-full bg-white/10 px-3.5 text-[13px] text-white hover:bg-white/15";

  return (
    <div className="main-console min-h-screen text-white antialiased">
      <div className="mx-auto flex w-full max-w-4xl flex-col gap-4 px-6 py-6">
        <div className="flex flex-wrap gap-2">
          <button type="button" className={button} onClick={() => play("pass")}>
            Play: all pass
          </button>
          <button type="button" className={button} onClick={() => play("mic-fail")}>
            Play: mic fails
          </button>
          <button type="button" className={button} onClick={() => play("model-fail")}>
            Play: model fails (then Repair)
          </button>
          <button type="button" className={button} onClick={playWaiting}>
            Play: Boris busy
          </button>
          <button type="button" className={button} onClick={() => setHint((h) => (h === "mic_silent" ? null : "mic_silent"))}>
            Toggle mic pill
          </button>
          <button type="button" className={button} onClick={() => setHint("ignored_voice")}>
            Show &ldquo;didn&apos;t sound like you&rdquo;
          </button>
          <button type="button" className={button} onClick={() => setHint("ignored_playback")}>
            Show &ldquo;ignored speaker&rdquo;
          </button>
          <button type="button" className={button} onClick={() => setHint(null)}>
            Clear pill
          </button>
        </div>

        <section className="home-status-panel rounded-[22px] border border-white/[0.065] bg-white/[0.04] px-5 py-5">
          <p className="text-[25px] font-semibold tracking-[-0.035em]">Ready</p>
          <p className="mt-1.5 text-[13px] text-[var(--boris-label-secondary)]">
            Say &ldquo;Hey Boris&rdquo;
          </p>
          <ListenHintPill status={status} onChooseMic={() => {}} onTeachVoice={() => {}} />
        </section>

        <section className="rounded-[14px] border border-white/[0.06] bg-white/[0.03] px-4 py-6 text-[13px] text-white/50">
          Conversation (should glide down when the pill appears, never jump)
        </section>
      </div>

      <AnimatePresence>
        {open ? (
          <SetupCheckToast
            key="setup-check"
            report={report}
            keyStatus={keyStatus}
            appVersion="preview"
            waiting={waiting}
            repairLabel={repairLabel}
            onRunAgain={() => play("pass")}
            onRetryKey={() => {
              setKeyStatus(null);
              setTimeout(() => setKeyStatus({ state: "ok", detail: "Key works" }), 900);
            }}
            onRepairModels={playRepair}
            onDismiss={dismiss}
            onOpenSettings={() => {}}
          />
        ) : null}
      </AnimatePresence>
    </div>
  );
}
