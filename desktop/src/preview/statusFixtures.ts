import { normalizeStatus, type StatusPicture } from "@/bridge";

export type StatusFixtureName =
  | "off"
  | "ready"
  | "hearing"
  | "reading"
  | "thinking"
  | "thinking-tool"
  | "confirm"
  | "talking"
  | "fault"
  | "long-caption"
  | "device-faults";

export type StatusFixture = {
  name: StatusFixtureName;
  label: string;
  status: StatusPicture;
};

const healthyDevices = {
  mic: { label: "Studio microphone", ok: true },
  speaker: { label: "Headphones", ok: true },
} as const;

function fixture(
  name: StatusFixtureName,
  label: string,
  status: Partial<StatusPicture>,
): StatusFixture {
  return {
    name,
    label,
    status: normalizeStatus({ ...healthyDevices, ...status }),
  };
}

/** Canonical visual-QA matrix for every meaningful overlay state. */
export const STATUS_FIXTURES: readonly StatusFixture[] = [
  fixture("off", "Off", {
    engine: "Off",
    phase: "Off",
    mic: { label: "Microphone unavailable", ok: false },
    speaker: { label: "Speaker unavailable", ok: false },
  }),
  fixture("ready", "Ready", {
    engine: "On",
    phase: "Armed",
  }),
  fixture("hearing", "Hearing", {
    engine: "On",
    phase: "Hearing",
    turn: "preview-hearing",
  }),
  fixture("reading", "Reading", {
    engine: "On",
    phase: "Reading",
    heard: "Plan a focused work block for this afternoon.",
    turn: "preview-reading",
  }),
  fixture("thinking", "Thinking", {
    engine: "On",
    phase: "Thinking",
    heard: "Summarize my notes from today.",
    activity: "Thinking…",
    turn: "preview-thinking",
  }),
  fixture("thinking-tool", "Thinking · tool", {
    engine: "On",
    phase: "Thinking",
    heard: "Check the forecast before my trip.",
    activity: "tool · web_search · weather in Bengaluru",
    turn: "preview-tool",
  }),
  fixture("confirm", "Confirm", {
    engine: "On",
    phase: "AwaitingConfirm",
    activity: "confirm · open_url",
    said: "Open the booking page in your browser?",
    turn: "preview-confirm",
  }),
  fixture("talking", "Talking", {
    engine: "On",
    phase: "Talking",
    heard: "What is on my schedule?",
    said: "You have a design review at two and a focus block at four.",
    turn: "preview-talking",
  }),
  fixture("fault", "Fault", {
    engine: "Fault",
    phase: "Quiet",
    detail: "The speech model stopped unexpectedly. Open Boris to retry.",
    turn: "preview-fault",
  }),
  fixture("long-caption", "Long caption", {
    engine: "On",
    phase: "Talking",
    said:
      "I found several relevant results and prepared a concise comparison with the trade-offs that matter most for your decision.",
    turn: "preview-long-caption",
  }),
  fixture("device-faults", "Device faults", {
    engine: "On",
    phase: "Armed",
    mic: { label: "USB microphone disconnected", ok: false },
    speaker: { label: "Headphones disconnected", ok: false },
  }),
] as const;

export function getStatusFixture(name: string): StatusPicture | undefined {
  return STATUS_FIXTURES.find((item) => item.name === name)?.status;
}
