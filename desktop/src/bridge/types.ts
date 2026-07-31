/**
 * UI contract shared by WINDOW and OVERLAY.
 * Mirrors Rust `boris_pipeline::StatusPicture`.
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

/** Device list entry from `list_input_devices` / `list_output_devices`. */
export type DeviceDto = {
  id: string;
  name: string;
  is_default: boolean;
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

/** Normalize partial / missing Option fields from serde. */
export function normalizeStatus(raw: Partial<StatusPicture> | null | undefined): StatusPicture {
  if (!raw) return { ...OFF_STATUS };
  return {
    engine: raw.engine ?? "Off",
    phase: raw.phase ?? "Off",
    detail: raw.detail ?? null,
    heard: raw.heard ?? null,
    said: raw.said ?? null,
    mic: raw.mic ?? OFF_STATUS.mic,
    speaker: raw.speaker ?? OFF_STATUS.speaker,
    turn: raw.turn ?? null,
  };
}
