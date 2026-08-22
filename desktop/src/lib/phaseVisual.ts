import type { EngineState, Phase } from "@/bridge";

/**
 * Presence tokens for the voice island + main hero.
 *
 * Design: one cool-neutral accent family that warms/cools slightly by phase.
 * Confirm is the only warm exception; Fault is the only red. No rainbow, no
 * purple “AI working” default.
 */

export type PhaseTone = {
  /** Short UI label (1–2 words) */
  label: string;
  /** One-line idle hint for this phase */
  hint: string;
  /** Accent for orb / glow (CSS color) */
  accent: string;
  /** Soft fill behind orb */
  glow: string;
  /** Whether the orb should animate */
  alive: boolean;
  /** Animation style */
  motion: "none" | "breathe" | "listen" | "think" | "speak";
};

/** Apple system-color family — one product voice, semantic exceptions only. */
const BASE = {
  accent: "#0a84ff",
  glow: "rgb(10 132 255 / 26%)",
} as const;

const READY = {
  accent: "#64d2ff",
  glow: "rgb(100 210 255 / 24%)",
} as const;

const LISTEN = {
  accent: "#0a84ff",
  glow: "rgb(10 132 255 / 30%)",
} as const;

const WORK = {
  accent: "#7d7aff",
  glow: "rgb(94 92 230 / 28%)",
} as const;

const SPEAK = {
  accent: "#64d2ff",
  glow: "rgb(100 210 255 / 26%)",
} as const;

const CONFIRM = {
  accent: "#ff9f0a",
  glow: "rgb(255 159 10 / 30%)",
} as const;

const PHASE: Record<Phase, PhaseTone> = {
  Off: {
    label: "Off",
    hint: "Press Start to begin",
    accent: "#8e8e93",
    glow: "rgb(142 142 147 / 18%)",
    alive: false,
    motion: "none",
  },
  Quiet: {
    label: "Ready",
    hint: "Say the wake word",
    accent: READY.accent,
    glow: READY.glow,
    alive: true,
    motion: "breathe",
  },
  Armed: {
    label: "Ready",
    hint: "Say the wake word",
    accent: READY.accent,
    glow: READY.glow,
    alive: true,
    motion: "breathe",
  },
  AwaitingReply: {
    label: "Your turn",
    hint: "No wake word needed",
    accent: LISTEN.accent,
    glow: LISTEN.glow,
    alive: true,
    motion: "listen",
  },
  AwaitingConfirm: {
    label: "Confirm",
    hint: "Say yes or no",
    accent: CONFIRM.accent,
    glow: CONFIRM.glow,
    alive: true,
    motion: "listen",
  },
  Hearing: {
    label: "Listening",
    hint: "Speak naturally",
    accent: LISTEN.accent,
    glow: LISTEN.glow,
    alive: true,
    motion: "listen",
  },
  Reading: {
    label: "Transcribing",
    hint: "Turning speech into text",
    accent: WORK.accent,
    glow: WORK.glow,
    alive: true,
    motion: "think",
  },
  Thinking: {
    // Overlay refines to Thinking / Working / Researching via pickOverlayPresence
    label: "Thinking",
    hint: "On it…",
    accent: WORK.accent,
    glow: WORK.glow,
    alive: true,
    motion: "think",
  },
  Talking: {
    label: "Speaking",
    // Empty-ish: caption carries the content; avoid "Speaking" + "Playing reply" clutter
    hint: "",
    accent: SPEAK.accent,
    glow: SPEAK.glow,
    alive: true,
    motion: "speak",
  },
};

export function toneFor(phase: Phase, engine: EngineState): PhaseTone {
  if (engine === "Fault") {
    return {
      label: "Error",
      hint: "Something went wrong",
      accent: "#ff453a",
      glow: "rgb(255 69 58 / 32%)",
      alive: true,
      motion: "breathe",
    };
  }
  if (engine === "Starting") {
    return {
      label: "Starting",
      hint: "Getting ready",
      accent: BASE.accent,
      glow: BASE.glow,
      alive: true,
      motion: "breathe",
    };
  }
  if (engine === "Off") {
    return PHASE.Off;
  }
  return PHASE[phase] ?? PHASE.Quiet;
}
