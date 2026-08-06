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
  /** Progressive tool / confirm chip (compact). */
  activity?: string | null;
  /** Estimated context tokens used (chars/4). */
  context_used?: number | null;
  /** Soft context window for the meter. */
  context_limit?: number | null;
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
  /** Strong / primary OpenRouter model id. */
  openrouter_model: string;
  /** Fast / cheap model for simple turns. */
  openrouter_fast_model?: string;
  /**
   * OpenRouter **model-provider** order for the strong model
   * (e.g. `coreweave` or `coreweave,baseten`) — inference host, not API brand.
   */
  openrouter_model_provider?: string;
  /** Model-provider order for the fast model. */
  openrouter_fast_provider?: string;
  /** When true, do not fall back to other hosts if the preferred list fails. */
  openrouter_pin_provider?: boolean;
  capability_preset?: string;
};

export const EMPTY_SETTINGS: AppSettings = {
  openrouter_api_key: "",
  openrouter_model: "",
  openrouter_fast_model: "",
  openrouter_model_provider: "",
  openrouter_fast_provider: "",
  openrouter_pin_provider: false,
  capability_preset: "",
};

/** Common OpenRouter chat models for the preset dropdown. */
export const MODEL_PRESETS: { id: string; label: string }[] = [
  { id: "google/gemini-2.5-flash-lite", label: "Gemini 2.5 Flash Lite" },
  { id: "google/gemini-2.5-flash", label: "Gemini 2.5 Flash" },
  { id: "google/gemini-2.5-pro", label: "Gemini 2.5 Pro" },
  { id: "openai/gpt-4o-mini", label: "GPT-4o mini" },
  { id: "openai/gpt-4o", label: "GPT-4o" },
  { id: "anthropic/claude-sonnet-4", label: "Claude Sonnet 4" },
  { id: "anthropic/claude-3.5-haiku", label: "Claude 3.5 Haiku" },
  { id: "deepseek/deepseek-chat", label: "DeepSeek Chat" },
  { id: "meta-llama/llama-3.3-70b-instruct", label: "Llama 3.3 70B" },
  { id: "x-ai/grok-3-mini", label: "Grok 3 Mini" },
];

/**
 * Common OpenRouter **model-provider** slugs (inference hosts on a model page).
 * Copy exact slug from OpenRouter when in doubt (`coreweave`, `deepinfra/turbo`).
 */
export const PROVIDER_PRESETS: { id: string; label: string }[] = [
  { id: "", label: "Auto (OpenRouter default)" },
  { id: "coreweave", label: "CoreWeave" },
  { id: "baseten", label: "Baseten" },
  { id: "siliconflow", label: "SiliconFlow" },
  { id: "novita", label: "NovitaAI" },
  { id: "fireworks", label: "Fireworks" },
  { id: "deepinfra", label: "DeepInfra" },
  { id: "phala", label: "Phala" },
  { id: "cloudflare", label: "Cloudflare" },
  { id: "venice", label: "Venice" },
  { id: "atlas-cloud", label: "AtlasCloud" },
  { id: "together", label: "Together" },
  { id: "groq", label: "Groq" },
];

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
  activity: null,
  context_used: null,
  context_limit: null,
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
    activity: raw.activity ?? null,
    context_used: raw.context_used ?? null,
    context_limit: raw.context_limit ?? null,
  };
}

/** Format token counts for the overlay meter: `233K / 500K`. */
export function formatContextMeter(
  used: number | null | undefined,
  limit: number | null | undefined,
): string | null {
  if (used == null || limit == null || limit <= 0) return null;
  const fmt = (n: number) => {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
    if (n >= 1000) return `${Math.round(n / 1000)}K`;
    return `${n}`;
  };
  return `${fmt(used)} / ${fmt(limit)}`;
}
