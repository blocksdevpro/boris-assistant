/**
 * UI contract shared by WINDOW and OVERLAY.
 * Mirrors the StatusPicture shape from the system architecture.
 * Rust will own the real source of truth later; these are the TS mirrors.
 */

export type EngineState = "Off" | "Starting" | "On" | "Fault";

export type Phase =
  | "Off"
  | "Quiet"
  | "Armed"
  | "Hearing"
  | "Reading"
  | "Thinking"
  | "Talking";

export type DeviceHealth = {
  label: string;
  ok: boolean;
};

export type StatusPicture = {
  engine: EngineState;
  phase: Phase;
  detail?: string | null;
  heard?: string | null;
  said?: string | null;
  mic: DeviceHealth;
  speaker: DeviceHealth;
  turn?: string | null;
};

/** Safe default before Rust emits anything. */
export const OFF_STATUS: StatusPicture = {
  engine: "Off",
  phase: "Off",
  detail: null,
  heard: null,
  said: null,
  mic: { label: "—", ok: false },
  speaker: { label: "—", ok: false },
  turn: null,
};
