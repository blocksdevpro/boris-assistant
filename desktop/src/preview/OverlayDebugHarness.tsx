import { useCallback, useEffect, useRef, useState } from "react";
import { OFF_STATUS, type AppSettings, type Phase, type StatusPicture } from "@/bridge";
import { StatusPreviewProvider } from "@/bridge/useStatus";
import type { OverlayDebugPreferences } from "@/windows/overlay/OverlayWindow";
import { OverlayWindow } from "@/windows/overlay/OverlayWindow";

type DebugStep = {
  label: string;
  status: StatusPicture;
};

type TextField = "activity" | "heard" | "said" | "thinking";
type CaptionMode = AppSettings["overlay_caption_mode"];
type Geometry = { x: number; y: number; width: number; height: number };
type MotionSample = {
  label: string;
  from: Geometry;
  to: Geometry;
  peakSampleDelta: number;
  peakAxisDelta: Geometry;
  frames: number;
  durationMs: number;
};
type MotionHistory = { schema: "geometry-v2"; items: MotionSample[] };

const HEALTHY_AUDIO = {
  mic: { label: "Studio microphone", ok: true },
  speaker: { label: "Headphones", ok: true },
};

function debugStatus(overrides: Partial<StatusPicture> = {}): StatusPicture {
  return {
    ...OFF_STATUS,
    engine: "On",
    phase: "Armed",
    ...HEALTHY_AUDIO,
    ...overrides,
  };
}

/** Independent transition path for the overlay debugger; no fixture gallery data. */
const DEBUG_STEPS: readonly DebugStep[] = [
  { label: "Off", status: debugStatus({ engine: "Off", phase: "Off" }) },
  {
    label: "Starting",
    status: debugStatus({
      engine: "Starting",
      phase: "Off",
      detail: "Initializing voice models…",
    }),
  },
  { label: "Ready", status: debugStatus() },
  {
    label: "Hearing",
    status: debugStatus({ phase: "Hearing", turn: "debug-hearing" }),
  },
  {
    label: "Reading",
    status: debugStatus({
      phase: "Reading",
      heard: "Plan a focused work block for this afternoon.",
      turn: "debug-reading",
    }),
  },
  {
    label: "Thinking",
    status: debugStatus({
      phase: "Thinking",
      heard: "Summarize my notes from today.",
      activity: "Thinking…",
      thinking: "Pull out the three decisions and keep the spoken summary short.",
      turn: "debug-thinking",
    }),
  },
  {
    label: "Searching",
    status: debugStatus({
      phase: "Thinking",
      heard: "Check the forecast before my trip.",
      activity: "tool · Searching weather in Bengaluru",
      thinking: "I’ll check the forecast before suggesting a departure time.",
      turn: "debug-searching",
    }),
  },
  {
    label: "Tool failed",
    status: debugStatus({
      phase: "Thinking",
      heard: "Check the latest train times.",
      activity: "fail · web_search",
      turn: "debug-tool-failure",
    }),
  },
  {
    label: "Barge · listening",
    status: debugStatus({
      phase: "Hearing",
      activity: "barge-in · listening",
      heard: "the interrupted request",
      said: "A stale draft response",
      thinking: "Stale reasoning from the interrupted task.",
      turn: "debug-barge-listening",
    }),
  },
  {
    label: "Barge · transcribing",
    status: debugStatus({
      phase: "Reading",
      activity: "barge-in · transcribing",
      heard: "the interrupted request",
      said: "A stale draft response",
      thinking: "Stale reasoning from the interrupted task.",
      turn: "debug-barge-reading",
    }),
  },
  {
    label: "Barge · switching",
    status: debugStatus({
      phase: "Thinking",
      activity: "barge-in · switching",
      heard: "the interrupted request",
      said: "A stale draft response",
      thinking: "Stale reasoning from the interrupted task.",
      turn: "debug-barge-switching",
    }),
  },
  {
    label: "Barge · stopping",
    status: debugStatus({
      phase: "Thinking",
      activity: "barge-in · stopping",
      heard: "the interrupted request",
      said: "A stale draft response",
      thinking: "Stale reasoning from the interrupted task.",
      turn: "debug-barge-stopping",
    }),
  },
  {
    label: "Confirm",
    status: debugStatus({
      phase: "AwaitingConfirm",
      activity: "confirm · open_url",
      said: "Open the booking page in your browser?",
      turn: "debug-confirm",
    }),
  },
  {
    label: "Typed secret",
    status: debugStatus({
      phase: "AwaitingInput",
      activity: "input · API key",
      said: "Paste the API key on screen. I will not read it back.",
      input: {
        id: "debug-secret-input",
        kind: "secret",
        label: "API key",
        spoken: "Paste the API key on screen. I will not read it back.",
        multiline: false,
        max_chars: 2048,
      },
      turn: "debug-secret-input",
    }),
  },
  {
    label: "Typed multiline",
    status: debugStatus({
      phase: "AwaitingInput",
      activity: "input · Notes",
      said: "Paste the notes you want me to organize.",
      input: {
        id: "debug-blob-input",
        kind: "blob",
        label: "Notes",
        spoken: "Paste the notes you want me to organize.",
        multiline: true,
        max_chars: 12000,
      },
      turn: "debug-blob-input",
    }),
  },
  {
    label: "Speaking",
    status: debugStatus({
      phase: "Talking",
      heard: "What is on my schedule?",
      said: "You have a design review at two and a focus block at four.",
      turn: "debug-speaking",
    }),
  },
  {
    label: "Awaiting reply",
    status: debugStatus({
      phase: "AwaitingReply",
      said: "Would you like the shorter version or the detailed one?",
      turn: "debug-awaiting-reply",
    }),
  },
  {
    label: "Mic unavailable",
    status: debugStatus({
      mic: { label: "USB microphone disconnected", ok: false },
    }),
  },
  {
    label: "Audio unavailable",
    status: debugStatus({
      mic: { label: "USB microphone disconnected", ok: false },
      speaker: { label: "Headphones disconnected", ok: false },
    }),
  },
  {
    label: "Engine fault",
    status: debugStatus({
      engine: "Fault",
      phase: "Quiet",
      detail: "The speech model stopped unexpectedly. Open Boris to retry.",
      turn: "debug-fault",
    }),
  },
  {
    label: "Artifact card",
    status: debugStatus({
      phase: "Talking",
      heard: "Write a script to rename photos.",
      said: "The script is on screen.",
      artifact: {
        id: "debug-artifact",
        title: "Rename photos",
        kind: "code",
        language: "powershell",
        path: "rename-photos.ps1",
      },
      turn: "debug-artifact",
    }),
  },
  {
    label: "Ready with last reply",
    status: debugStatus({
      phase: "Armed",
      said: "I found three options and summarized the trade-offs.",
      turn: "debug-ready-linger",
    }),
  },
];

function asCaptionMode(value: string): CaptionMode | null {
  if (value === "full" || value === "assistant" || value === "hidden") {
    return value;
  }
  return null;
}

function asPhase(value: string): Phase | null {
  const phase = DEBUG_STEPS.find((step) => step.status.phase === value)?.status.phase;
  return phase ?? null;
}

function measureGeometry(node: HTMLElement, frame: HTMLElement): Geometry {
  const rect = node.getBoundingClientRect();
  const frameRect = frame.getBoundingClientRect();
  return {
    x: rect.left - frameRect.left,
    y: rect.top - frameRect.top,
    width: rect.width,
    height: rect.height,
  };
}

function frameDelta(from: Geometry, to: Geometry): number {
  return Math.max(
    Math.abs(to.x - from.x),
    Math.abs(to.y - from.y),
    Math.abs(to.width - from.width),
    Math.abs(to.height - from.height),
  );
}

export default function OverlayDebugHarness() {
  const [status, setStatus] = useState<StatusPicture>(DEBUG_STEPS[2].status);
  const [stepIndex, setStepIndex] = useState(2);
  const [stepLabel, setStepLabel] = useState(DEBUG_STEPS[2].label);
  const [playing, setPlaying] = useState(false);
  const [preferences, setPreferences] = useState<OverlayDebugPreferences>({
    overlay_caption_mode: "full",
    overlay_scale_percent: 100,
  });
  const [readyCaptionDelayMs, setReadyCaptionDelayMs] = useState(60_000);
  const [reducedMotion, setReducedMotion] = useState(false);
  const [motionHistory, setMotionHistory] = useState<MotionHistory>({
    schema: "geometry-v2",
    items: [],
  });
  const motionSamples = motionHistory.schema === "geometry-v2" ? motionHistory.items : [];
  const previewRef = useRef<HTMLDivElement>(null);

  const applyStep = useCallback((index: number) => {
    const step = DEBUG_STEPS[index];
    if (!step) return;
    setStepIndex(index);
    setStepLabel(step.label);
    setStatus((previous) => ({ ...step.status, seq: (previous.seq ?? 0) + 1 }));
  }, []);

  useEffect(() => {
    if (!playing || stepIndex < 0) return;
    if (stepIndex >= DEBUG_STEPS.length - 1) {
      setPlaying(false);
      return;
    }
    const timer = window.setTimeout(
      () => applyStep(stepIndex + 1),
      1_050,
    );
    return () => window.clearTimeout(timer);
  }, [applyStep, playing, stepIndex]);

  useEffect(() => {
    const preview = previewRef.current;
    const island = preview?.querySelector<HTMLElement>(".overlay-island");
    if (!preview || !island) return;

    const startedAt = performance.now();
    let first: Geometry | null = null;
    let previous: Geometry | null = null;
    let latest: Geometry | null = null;
    let peakSampleDelta = 0;
    const peakAxisDelta: Geometry = { x: 0, y: 0, width: 0, height: 0 };
    let frames = 0;
    let stableFrames = 0;
    let request = 0;

    const sample = () => {
      const current = measureGeometry(island, preview);
      first ??= current;
      if (previous) {
        const delta = frameDelta(previous, current);
        peakSampleDelta = Math.max(peakSampleDelta, delta);
        peakAxisDelta.x = Math.max(peakAxisDelta.x, Math.abs(current.x - previous.x));
        peakAxisDelta.y = Math.max(peakAxisDelta.y, Math.abs(current.y - previous.y));
        peakAxisDelta.width = Math.max(peakAxisDelta.width, Math.abs(current.width - previous.width));
        peakAxisDelta.height = Math.max(peakAxisDelta.height, Math.abs(current.height - previous.height));
        stableFrames = delta < 0.15 ? stableFrames + 1 : 0;
      }
      previous = current;
      latest = current;
      frames += 1;

      const elapsed = performance.now() - startedAt;
      if (elapsed >= 900 || (elapsed >= 450 && stableFrames >= 8)) {
        if (first && latest) {
          const sample: MotionSample = {
            label: stepLabel,
            from: first,
            to: latest,
            peakSampleDelta,
            peakAxisDelta,
            frames,
            durationMs: elapsed,
          };
          setMotionHistory((history) => ({
            schema: "geometry-v2",
            items: [
              ...(history.schema === "geometry-v2" ? history.items : []),
              sample,
            ].slice(-(DEBUG_STEPS.length + 3)),
          }));
        }
        return;
      }
      request = window.requestAnimationFrame(sample);
    };

    request = window.requestAnimationFrame(sample);
    return () => window.cancelAnimationFrame(request);
  }, [preferences, status, stepLabel]);

  const editText = (field: TextField, value: string) => {
    setPlaying(false);
    setStepIndex(-1);
    setStepLabel("Custom edits");
    setStatus((previous) => ({
      ...previous,
      [field]: value || null,
      seq: (previous.seq ?? 0) + 1,
    }));
  };

  const patchStatus = (patch: Partial<StatusPicture>, label = "Custom state") => {
    setPlaying(false);
    setStepIndex(-1);
    setStepLabel(label);
    setStatus((previous) => ({
      ...previous,
      ...patch,
      seq: (previous.seq ?? 0) + 1,
    }));
  };

  const scale = preferences.overlay_scale_percent / 100;
  const previewSize = {
    width: 464 * scale,
    height: 364 * scale,
  };

  return (
    <main className="min-h-screen bg-[#0a0c10] px-5 py-6 text-white selection:bg-sky-300/25">
      <div className="mx-auto max-w-[1240px]">
        <header className="mb-5 flex flex-wrap items-end justify-between gap-4">
          <div>
            <p className="text-[10px] font-semibold uppercase tracking-[0.16em] text-sky-200/65">
              Development only · Overlay lab
            </p>
            <h1 className="mt-1 text-[24px] font-semibold tracking-[-0.035em] text-white/95">
              Overlay transitions
            </h1>
            <p className="mt-1 max-w-2xl text-[12px] leading-relaxed text-white/50">
              Drive the real island through state and text changes. The preview frame tracks the
              native shadow gutter at each scale.
            </p>
          </div>
          <div className="rounded-lg border border-white/[0.08] bg-white/[0.035] px-3 py-2 text-right text-[10px] text-white/45">
            <p>Current state</p>
            <p className="mt-0.5 text-[12px] font-medium text-white/80">{stepLabel}</p>
          </div>
        </header>

        <div className="grid items-start gap-5 lg:grid-cols-[320px_minmax(0,1fr)]">
          <aside className="space-y-4">
            <section className="rounded-xl border border-white/[0.08] bg-white/[0.035] p-3.5">
              <div className="mb-2 flex items-center justify-between">
                <h2 className="text-[12px] font-semibold text-white/85">State sequence</h2>
                <div className="flex gap-1.5">
                  <button
                    type="button"
                    className="debug-button"
                    onClick={() => {
                      setPlaying(false);
                      applyStep((stepIndex - 1 + DEBUG_STEPS.length) % DEBUG_STEPS.length);
                    }}
                  >
                    Previous
                  </button>
                  <button
                    type="button"
                    className="debug-button"
                    onClick={() => {
                      setPlaying(false);
                      applyStep((stepIndex + 1) % DEBUG_STEPS.length);
                    }}
                  >
                    Next
                  </button>
                </div>
              </div>
              <button
                type="button"
                className="debug-button debug-button--primary mb-2 w-full"
                onClick={() => {
                  if (!playing) applyStep(0);
                  setPlaying((current) => !current);
                }}
              >
                {playing ? "Stop sequence" : "Play all transitions"}
              </button>
              <nav aria-label="Overlay states" className="max-h-[220px] space-y-0.5 overflow-y-auto pr-1">
                {DEBUG_STEPS.map((step, index) => (
                  <button
                    key={step.label}
                    type="button"
                    aria-current={index === stepIndex ? "step" : undefined}
                    onClick={() => {
                      setPlaying(false);
                      applyStep(index);
                    }}
                    className={`flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[10px] transition-colors ${
                      index === stepIndex
                        ? "bg-sky-300/15 text-sky-100"
                        : "text-white/55 hover:bg-white/[0.06] hover:text-white/80"
                    }`}
                  >
                    <span className="w-5 shrink-0 tabular-nums text-white/30">
                      {String(index + 1).padStart(2, "0")}
                    </span>
                    <span className="truncate">{step.label}</span>
                  </button>
                ))}
              </nav>
            </section>

            <section className="rounded-xl border border-white/[0.08] bg-white/[0.035] p-3.5">
              <h2 className="mb-3 text-[12px] font-semibold text-white/85">Text and edge cases</h2>
              <label className="debug-label">
                Phase
                <select
                  className="debug-input"
                  value={status.phase}
                  onChange={(event) => {
                    const phase = asPhase(event.target.value);
                    if (phase) patchStatus({ phase }, `Phase · ${phase}`);
                  }}
                >
                  {[...new Set(DEBUG_STEPS.map((step) => step.status.phase))].map((phase) => (
                    <option key={phase} value={phase}>{phase}</option>
                  ))}
                </select>
              </label>
              {([
                ["activity", "Activity"],
                ["heard", "Heard text"],
                ["said", "Assistant text"],
                ["thinking", "Live reasoning"],
              ] as const).map(([field, label]) => (
                <label key={field} className="debug-label mt-2.5">
                  {label}
                  <textarea
                    aria-label={label}
                    className="debug-input min-h-12 resize-y"
                    rows={field === "thinking" ? 3 : 2}
                    value={status[field] ?? ""}
                    onChange={(event) => editText(field, event.target.value)}
                  />
                </label>
              ))}
              <div className="mt-3 grid grid-cols-2 gap-1.5">
                <button
                  type="button"
                  className="debug-button"
                  onClick={() => patchStatus({
                    phase: "Talking",
                    said: "A long spoken reply tests the caption edge, including punctuation, wrapping, and words at the far end that should clip without moving the island unexpectedly.",
                    heard: null,
                    thinking: null,
                    activity: null,
                    detail: null,
                    input: null,
                    artifact: null,
                    turn: `debug-long-${Date.now()}`,
                  }, "Long caption")}
                >
                  Long caption
                </button>
                <button
                  type="button"
                  className="debug-button"
                  onClick={() => patchStatus({
                    phase: "Talking",
                    said: "こんにちは 🌙 مرحبًا — café, 👩🏽‍💻, नमस्ते",
                    heard: null,
                    thinking: null,
                    activity: null,
                    detail: null,
                    input: null,
                    artifact: null,
                    turn: `debug-unicode-${Date.now()}`,
                  }, "Unicode caption")}
                >
                  Unicode caption
                </button>
                <button
                  type="button"
                  className="debug-button"
                  onClick={() => patchStatus({
                    phase: "Thinking",
                    said: null,
                    thinking: "First line\nSecond line\nThird line\n".repeat(10),
                    activity: "Thinking…",
                    heard: null,
                    input: null,
                    artifact: null,
                    detail: null,
                    turn: `debug-long-thought-${Date.now()}`,
                  }, "Long reasoning")}
                >
                  Long reasoning
                </button>
                <button
                  type="button"
                  className="debug-button"
                  onClick={() => patchStatus({
                    phase: "Armed",
                    heard: null,
                    said: null,
                    thinking: null,
                    activity: null,
                    detail: null,
                    input: null,
                    artifact: null,
                    mic: HEALTHY_AUDIO.mic,
                    speaker: HEALTHY_AUDIO.speaker,
                    turn: `debug-clear-${Date.now()}`,
                  }, "Ready · cleared")}
                >
                  Clear to Ready
                </button>
              </div>
            </section>

            <section className="rounded-xl border border-white/[0.08] bg-white/[0.035] p-3.5">
              <h2 className="mb-3 text-[12px] font-semibold text-white/85">Overlay preferences</h2>
              <label className="debug-label">
                Caption visibility
                <select
                  className="debug-input"
                  value={preferences.overlay_caption_mode}
                  onChange={(event) => {
                    const mode = asCaptionMode(event.target.value);
                    if (mode) setPreferences((current) => ({ ...current, overlay_caption_mode: mode }));
                  }}
                >
                  <option value="full">All speech</option>
                  <option value="assistant">Assistant only</option>
                  <option value="hidden">Hidden</option>
                </select>
              </label>
              <label className="debug-label mt-2.5">
                Ready reply linger
                <select
                  className="debug-input"
                  value={readyCaptionDelayMs}
                  onChange={(event) => {
                    if (event.target.value === "5000") setReadyCaptionDelayMs(5_000);
                    if (event.target.value === "60000") setReadyCaptionDelayMs(60_000);
                  }}
                >
                  <option value="60000">60 seconds · inspect</option>
                  <option value="5000">5 seconds · product behavior</option>
                </select>
              </label>
              <div className="mt-3">
                <div className="mb-1.5 flex items-center justify-between text-[10px] text-white/55">
                  <span>Scale</span>
                  <span className="tabular-nums text-white/80">{preferences.overlay_scale_percent}%</span>
                </div>
                <input
                  aria-label="Overlay scale"
                  className="w-full accent-sky-300"
                  type="range"
                  min={75}
                  max={125}
                  step={1}
                  value={preferences.overlay_scale_percent}
                  onChange={(event) => setPreferences((current) => ({
                    ...current,
                    overlay_scale_percent: Math.min(125, Math.max(75, Number(event.target.value))),
                  }))}
                />
                <div className="mt-1 flex justify-between text-[9px] text-white/35">
                  <span>75%</span><span>100%</span><span>125%</span>
                </div>
              </div>
              <label className="mt-3 flex items-center gap-2 text-[10px] text-white/65">
                <input
                  type="checkbox"
                  checked={reducedMotion}
                  onChange={(event) => setReducedMotion(event.target.checked)}
                />
                Reduced motion
              </label>
            </section>
          </aside>

          <section className="min-w-0 space-y-4">
            <div className="overflow-auto rounded-xl border border-white/[0.08] bg-white/[0.035] p-4">
              <div
                ref={previewRef}
                data-testid="overlay-debug-window"
                className="overlay-debug-window relative mx-auto overflow-hidden rounded-[12px] border border-white/[0.12]"
                style={previewSize}
              >
                <StatusPreviewProvider status={status}>
                  <OverlayWindow
                    debugPreferences={preferences}
                    debugReadyCaptionMs={readyCaptionDelayMs}
                    debugReducedMotion={reducedMotion}
                  />
                </StatusPreviewProvider>
              </div>
              <p className="mt-2 text-center text-[9px] text-white/35">
                Transparent browser canvas · {Math.round(previewSize.width)} × {Math.round(previewSize.height)} px
              </p>
            </div>

            <section className="rounded-xl border border-white/[0.08] bg-white/[0.035] p-3.5">
              <div className="mb-2 flex flex-wrap items-baseline justify-between gap-2">
                <h2 className="text-[12px] font-semibold text-white/85">Layout motion samples</h2>
                <p className="text-[9px] text-white/40">Largest island edge movement between browser samples</p>
              </div>
              {motionSamples.length === 0 ? (
                <p className="text-[10px] text-white/40">Change state or scale to record its geometry transition.</p>
              ) : (
                <div className="overflow-x-auto">
                  <table className="w-full min-w-[620px] border-collapse text-left text-[10px]">
                    <thead className="text-white/35">
                      <tr>
                        <th className="pb-1.5 pr-3 font-medium">Change</th>
                        <th className="pb-1.5 pr-3 font-medium">From → to</th>
                        <th className="pb-1.5 pr-3 font-medium">Peak sampled · x y w h</th>
                        <th className="pb-1.5 pr-3 font-medium">Rate</th>
                        <th className="pb-1.5 font-medium">Settled</th>
                      </tr>
                    </thead>
                    <tbody>
                      {motionSamples.map((sample, index) => (
                        <tr key={`${sample.label}-${index}`} className="border-t border-white/[0.06] text-white/65">
                          <td className="max-w-36 truncate py-1.5 pr-3">{sample.label}</td>
                          <td className="whitespace-nowrap py-1.5 pr-3 tabular-nums">
                            {Math.round(sample.from.width)}×{Math.round(sample.from.height)} → {Math.round(sample.to.width)}×{Math.round(sample.to.height)}
                          </td>
                          <td className={`whitespace-nowrap py-1.5 pr-3 tabular-nums ${sample.peakSampleDelta > 20 && sample.frames >= 24 ? "text-amber-200" : "text-emerald-200/80"}`}>
                            {sample.peakSampleDelta.toFixed(1)} px · {sample.peakAxisDelta.x.toFixed(1)} {sample.peakAxisDelta.y.toFixed(1)} {sample.peakAxisDelta.width.toFixed(1)} {sample.peakAxisDelta.height.toFixed(1)}
                          </td>
                          <td className="whitespace-nowrap py-1.5 pr-3 tabular-nums">{(sample.frames / (sample.durationMs / 1000)).toFixed(0)} fps</td>
                          <td className="whitespace-nowrap py-1.5">{sample.frames < 24 ? "Low sample rate" : sample.peakSampleDelta > 20 ? "Inspect jump" : "No large jump"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </section>
          </section>
        </div>
      </div>

      <style>{`
        .debug-button {
          min-height: 27px;
          border: 1px solid rgba(255, 255, 255, 0.09);
          border-radius: 6px;
          background: rgba(255, 255, 255, 0.035);
          padding: 0 8px;
          color: rgba(255, 255, 255, 0.68);
          font-size: 10px;
          transition: background-color 140ms ease, border-color 140ms ease, color 140ms ease;
        }
        .debug-button:hover { border-color: rgba(255, 255, 255, 0.18); background: rgba(255, 255, 255, 0.075); color: rgba(255, 255, 255, 0.92); }
        .debug-button--primary { border-color: rgba(125, 211, 252, 0.23); background: rgba(56, 189, 248, 0.09); color: rgba(224, 242, 254, 0.88); }
        .debug-label { display: flex; flex-direction: column; gap: 5px; color: rgba(255, 255, 255, 0.48); font-size: 10px; }
        .debug-input { width: 100%; border: 1px solid rgba(255,255,255,0.09); border-radius: 6px; background: rgba(0,0,0,0.2); padding: 6px 7px; color: rgba(255,255,255,0.8); outline: none; }
        .debug-input:focus { border-color: rgba(125,211,252,0.45); box-shadow: 0 0 0 2px rgba(125,211,252,0.12); }
        .overlay-debug-window { background-color: #10131a; background-image: linear-gradient(45deg,rgba(255,255,255,.018) 25%,transparent 25%),linear-gradient(-45deg,rgba(255,255,255,.018) 25%,transparent 25%),linear-gradient(45deg,transparent 75%,rgba(255,255,255,.018) 75%),linear-gradient(-45deg,transparent 75%,rgba(255,255,255,.018) 75%); background-position: 0 0,0 6px,6px -6px,-6px 0; background-size: 12px 12px; }
      `}</style>
    </main>
  );
}
