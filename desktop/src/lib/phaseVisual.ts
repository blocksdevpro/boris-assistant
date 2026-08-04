import type { EngineState, Phase } from "@/bridge";

/**
 * Design tokens for the always-on-top voice island.
 * Phase is emotional state of the assistant — not a tech status dump.
 */

export type PhaseTone = {
  /** Short UI label */
  label: string;
  /** One-line hint when idle for that phase */
  hint: string;
  /** Accent for orb / glow (CSS color) */
  accent: string;
  /** Soft fill behind orb */
  glow: string;
  /** Whether the orb should animate (rings / pulse) */
  alive: boolean;
  /** Animation style */
  motion: "none" | "breathe" | "listen" | "think" | "speak";
};

const PHASE: Record<Phase, PhaseTone> = {
  Off: {
    label: "Standby",
    hint: "Engine is off",
    accent: "oklch(0.55 0.02 260)",
    glow: "oklch(0.35 0.02 260 / 35%)",
    alive: false,
    motion: "none",
  },
  Quiet: {
    label: "Quiet",
    hint: "Waiting…",
    accent: "oklch(0.65 0.04 250)",
    glow: "oklch(0.4 0.04 250 / 40%)",
    alive: false,
    motion: "none",
  },
  Armed: {
    label: "Ready",
    hint: "Say the wake word when you need me",
    accent: "oklch(0.78 0.14 165)",
    glow: "oklch(0.55 0.12 165 / 40%)",
    alive: true,
    motion: "breathe",
  },
  AwaitingReply: {
    label: "Your turn",
    hint: "He asked — answer freely, no wake word",
    accent: "oklch(0.8 0.15 200)",
    glow: "oklch(0.55 0.14 200 / 50%)",
    alive: true,
    motion: "listen",
  },
  AwaitingConfirm: {
    label: "Confirm?",
    hint: "Yes / no / sure / cancel — no wake word",
    accent: "oklch(0.82 0.16 55)",
    glow: "oklch(0.55 0.14 55 / 50%)",
    alive: true,
    motion: "listen",
  },
  Hearing: {
    label: "Listening",
    hint: "Hearing you…",
    accent: "oklch(0.78 0.16 220)",
    glow: "oklch(0.55 0.14 220 / 50%)",
    alive: true,
    motion: "listen",
  },
  Reading: {
    label: "Reading",
    hint: "Turning speech into text",
    accent: "oklch(0.82 0.14 85)",
    glow: "oklch(0.55 0.12 85 / 45%)",
    alive: true,
    motion: "think",
  },
  Thinking: {
    label: "Thinking",
    hint: "Working…",
    accent: "oklch(0.72 0.18 295)",
    glow: "oklch(0.5 0.16 295 / 50%)",
    alive: true,
    motion: "think",
  },
  Talking: {
    label: "Speaking",
    hint: "Audio is playing now",
    accent: "oklch(0.78 0.16 35)",
    glow: "oklch(0.55 0.14 35 / 50%)",
    alive: true,
    motion: "speak",
  },
};

export function toneFor(phase: Phase, engine: EngineState): PhaseTone {
  if (engine === "Fault") {
    return {
      label: "Fault",
      hint: "Something went wrong",
      accent: "oklch(0.68 0.2 25)",
      glow: "oklch(0.45 0.16 25 / 50%)",
      alive: true,
      motion: "breathe",
    };
  }
  if (engine === "Starting") {
    return {
      label: "Starting",
      hint: "Waking up…",
      accent: "oklch(0.75 0.1 250)",
      glow: "oklch(0.5 0.08 250 / 45%)",
      alive: true,
      motion: "breathe",
    };
  }
  if (engine === "Off") {
    return PHASE.Off;
  }
  return PHASE[phase] ?? PHASE.Quiet;
}
