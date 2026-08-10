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

/** Shared cool base — product identity, not status carnival. */
const BASE = {
  accent: "oklch(0.78 0.06 250)",
  glow: "oklch(0.5 0.05 250 / 28%)",
} as const;

const READY = {
  accent: "oklch(0.8 0.07 200)",
  glow: "oklch(0.52 0.06 200 / 30%)",
} as const;

const LISTEN = {
  accent: "oklch(0.8 0.09 230)",
  glow: "oklch(0.52 0.07 230 / 32%)",
} as const;

const WORK = {
  accent: "oklch(0.76 0.07 255)",
  glow: "oklch(0.48 0.06 255 / 30%)",
} as const;

const SPEAK = {
  accent: "oklch(0.8 0.08 55)",
  glow: "oklch(0.52 0.06 55 / 28%)",
} as const;

const CONFIRM = {
  accent: "oklch(0.84 0.12 75)",
  glow: "oklch(0.55 0.1 75 / 35%)",
} as const;

const PHASE: Record<Phase, PhaseTone> = {
  Off: {
    label: "Off",
    hint: "Press Start to begin",
    accent: "oklch(0.58 0.02 250)",
    glow: "oklch(0.35 0.02 250 / 20%)",
    alive: false,
    motion: "none",
  },
  Quiet: {
    label: "Ready",
    hint: "Say the wake word to talk",
    accent: READY.accent,
    glow: READY.glow,
    alive: true,
    motion: "breathe",
  },
  Armed: {
    label: "Ready",
    hint: "Say the wake word to talk",
    accent: READY.accent,
    glow: READY.glow,
    alive: true,
    motion: "breathe",
  },
  AwaitingReply: {
    label: "Your turn",
    hint: "Answer freely — no wake word",
    accent: LISTEN.accent,
    glow: LISTEN.glow,
    alive: true,
    motion: "listen",
  },
  AwaitingConfirm: {
    label: "Confirm",
    hint: "Waiting for your yes",
    accent: CONFIRM.accent,
    glow: CONFIRM.glow,
    alive: true,
    motion: "listen",
  },
  Hearing: {
    label: "Listening",
    hint: "Go ahead",
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
      accent: "oklch(0.68 0.18 25)",
      glow: "oklch(0.45 0.14 25 / 40%)",
      alive: true,
      motion: "breathe",
    };
  }
  if (engine === "Starting") {
    return {
      label: "Starting",
      hint: "Loading…",
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
