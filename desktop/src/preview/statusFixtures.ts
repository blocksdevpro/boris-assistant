import { normalizeStatus, type StatusPicture } from "@/bridge";

export type StatusFixtureName =
  | "off"
  | "starting"
  | "ready"
  | "awaiting-reply"
  | "hearing"
  | "reading"
  | "thinking"
  | "thinking-long"
  | "thinking-tool"
  | "tool-failure"
  | "typed-input"
  | "confirm"
  | "talking"
  | "fault"
  | "long-caption"
  | "device-faults"
  | "artifact-card";

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
  fixture("starting", "Starting", {
    engine: "Starting",
    phase: "Off",
    detail: "Initializing voice models…",
  }),
  fixture("ready", "Ready", {
    engine: "On",
    phase: "Armed",
  }),
  fixture("awaiting-reply", "Awaiting reply", {
    engine: "On",
    phase: "AwaitingReply",
    said: "Would you like the shorter version or the detailed one?",
    turn: "preview-awaiting-reply",
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
    thinking:
      "Notes first, then a short spoken brief. Pull the three decisions and skip anything that looks like a raw dump.",
    turn: "preview-thinking",
  }),
  fixture("thinking-long", "Thinking · long stream", {
    engine: "On",
    phase: "Thinking",
    heard: "Turn this messy brief into an ordered launch checklist.",
    activity: "Thinking…",
    thinking:
      "Start with the decisions that can block the launch. Group the remaining work by owner, keep dependencies close to the tasks they affect, and separate required checks from optional polish. The overlay should preserve the newest lines without growing beyond its glanceable reading budget.",
    turn: "preview-thinking-long",
  }),
  fixture("thinking-tool", "Thinking · tool", {
    engine: "On",
    phase: "Thinking",
    heard: "Check the forecast before my trip.",
    activity: "tool · Searching weather in Bengaluru",
    turn: "preview-tool",
  }),
  fixture("tool-failure", "Thinking · tool failed", {
    engine: "On",
    phase: "Thinking",
    heard: "Check the latest train times.",
    activity: "fail · web_search",
    turn: "preview-tool-failure",
  }),
  fixture("typed-input", "Typed input", {
    engine: "On",
    phase: "AwaitingInput",
    said: "Paste the API key on screen. I will not read it back.",
    activity: "input · API key",
    input: {
      id: "preview-input",
      kind: "secret",
      label: "API key",
      spoken: "Paste the API key on screen. I will not read it back.",
      multiline: false,
      max_chars: 2048,
    },
    turn: "preview-input",
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
  fixture("artifact-card", "Artifact card", {
    engine: "On",
    phase: "Talking",
    heard: "Write a script to rename photos.",
    said: "Script's on screen.",
    artifact: {
      id: "a1f3c9",
      title: "Rename photos",
      kind: "code",
      language: "powershell",
      path: "rename-photos-a1f3c9.ps1",
    },
    turn: "preview-artifact",
  }),
] as const;

export function getStatusFixture(name: string): StatusPicture | undefined {
  return STATUS_FIXTURES.find((item) => item.name === name)?.status;
}
