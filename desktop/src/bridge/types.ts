/**
 * UI contract shared by WINDOW and OVERLAY.
 * Mirrors Rust `boris_pipeline::StatusPicture`.
 */

export type EngineState = "Off" | "Starting" | "On" | "Fault";

export type Phase =
  | "Off"
  | "Quiet"
  | "Armed"
  | "AwaitingReply"
  | "AwaitingConfirm"
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

/** Result of `preflight_check` — model readiness under `~/.boris`. */
export type PreflightReport = {
  parakeet_ready: boolean;
  supertone_ready: boolean;
  boris_home: string;
  parakeet_dir: string;
  supertone_onnx_dir: string;
  supertone_voices_dir: string;
  ok: boolean;
  messages: string[];
};

/** Local model install status from `models_status`. */
export type ModelsStatus = {
  home: string;
  models_dir: string;
  parakeet_ready: boolean;
  parakeet_dir: string;
  supertone_ready: boolean;
  supertone_onnx_dir: string;
  supertone_voices_dir: string;
  missing: string[];
  base_url_override: string | null;
};

export type ModelComponent = "parakeet" | "supertone";

export type DownloadFileStatus =
  | "starting"
  | "downloading"
  | "skipped"
  | "done"
  | "failed";

/** Progress event payload for `models-progress`. */
export type DownloadProgress = {
  component: ModelComponent;
  file_name: string;
  relative_path: string;
  bytes_downloaded: number;
  total_bytes: number | null;
  status: DownloadFileStatus;
  message?: string | null;
};

export type ModelsInstallReport = {
  ok: boolean;
  parakeet_ready: boolean;
  supertone_ready: boolean;
  files_downloaded: number;
  files_skipped: number;
  files_failed: number;
  errors: string[];
};

/** Persisted under `~/.boris/settings.json` (Rust `AppSettings`). */
export type AppSettings = {
  openrouter_api_key: string;
  openrouter_model: string;
};

export const EMPTY_SETTINGS: AppSettings = {
  openrouter_api_key: "",
  openrouter_model: "",
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
