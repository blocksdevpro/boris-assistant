/**
 * UI DTOs shared by main window and overlay.
 *
 * # Host vs pipeline
 *
 * These shapes mirror **pipeline** serde types (`boris_pipeline::{StatusPicture,
 * DeviceDto, PreflightReport, ModelsStatus, DownloadProgress, AppSettings, …}`).
 * The host only forwards them over Tauri IPC — it does not invent alternate fields.
 *
 * Keep this file aligned with Rust DTO sources in one atomic PR when fields change.
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

/** Mirrors `boris_pipeline::ArtifactPeek` — no body. */
export type ArtifactPeek = {
  id: string;
  title: string;
  kind: string;
  language?: string | null;
  path: string;
};

/** Mirrors `boris_pipeline::ArtifactListItem`. */
export type ArtifactListItem = {
  id: string;
  title: string;
  kind: string;
  language?: string | null;
  path: string;
  pinned: boolean;
  revision: number;
  current: boolean;
};

/** Mirrors `boris_pipeline::ArtifactCard`. */
export type ArtifactCard = {
  id: string;
  title: string;
  kind: string;
  language?: string | null;
  path: string;
  pinned: boolean;
  revision: number;
  body: string;
};

/** Mirrors `boris_pipeline::StatusPicture`. */
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
  /** This-turn overlay glance (cleared on the next utterance). Body is separate. */
  artifact?: ArtifactPeek | null;
  /** Live-mic teach progress (dedicated teach page). */
  wake_enroll?: WakeEnrollPeek | null;
};

/** Mirrors `boris_pipeline::WakeEnrollPeek`. */
export type WakeEnrollPeek = {
  have: number;
  want: number;
  ready: boolean;
  hint?: string | null;
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

/**
 * Prefs + secrets from `~/.boris/config.toml` + `auth.json`.
 * Mirrors Rust `boris_pipeline::AppSettings` (snake_case wire format).
 */
export type AppSettings = {
  openrouter_api_key: string;
  /** Optional Exa key. `web_search` works without it (DuckDuckGo + Wikipedia). */
  exa_api_key: string;
  /** Strong / primary OpenRouter model id. */
  openrouter_model: string;
  /** Fast / cheap model for simple turns. */
  openrouter_fast_model: string;
  /**
   * OpenRouter **model-provider** order for the strong model
   * (e.g. `coreweave` or `coreweave,baseten`) — inference host, not API brand.
   */
  openrouter_model_provider: string;
  /** Model-provider order for the fast model. */
  openrouter_fast_provider: string;
  /** When true, do not fall back to other hosts if the preferred list fails. */
  openrouter_pin_provider: boolean;
  /** `full` | `local_power` | `voice_safe` */
  capability_preset: string;
  /** Preferred mic device id; empty = OS default. */
  input_device: string;
  /** Preferred speaker device id; empty = OS default. */
  output_device: string;
  /** Supertone voice stem (e.g. `M4`). */
  tts_voice_id: string;
  /** STT/TTS RAM policy. */
  model_residency: "low_memory" | "balanced" | "low_latency";
  /** Reserved compatibility setting; hidden until echo-safe barge-in exists. */
  voice_barge_in: boolean;
  /** Ignore TV / Translate / TTS out of a speaker after a live enroll. */
  ignore_speaker_playback: boolean;
  /** Long-term markdown memory. */
  long_term_memory: boolean;
  /**
   * Trusted mode: auto-allow notes and sandbox file writes.
   * Shell and open URL still need yes.
   */
  trusted_auto_moderate: boolean;
  /**
   * Max HITL confirms per user turn before remaining tools are denied.
   * Default 12 (multi-tool friendly). No dedicated UI control yet.
   */
  max_confirms_per_turn: number;
  /** Prefer showing the floating island on wake. */
  show_overlay_on_wake: boolean;
  /** Which spoken text may appear in the floating overlay. */
  overlay_caption_mode: "full" | "assistant" | "hidden";
  /** Preferred overlay anchor on the active display. */
  overlay_position: "top_center" | "top_left" | "top_right";
  /** Overlay size as a percentage, clamped to 75-125. */
  overlay_scale_percent: number;
  /** Start the engine when the app opens. */
  start_engine_on_launch: boolean;
  /** Launch at Windows sign-in (silent, engine on, no main window). */
  start_with_windows: boolean;
  /** App-update feed: GitHub latest (`stable`) or the `beta` pre-release. */
  update_channel: UpdateChannel;
  /** Optional log filter (`info`, `boris=debug`, …). */
  logging_filter: string;
};

/** Which GitHub Releases feed the desktop updater polls. */
export type LivenessStatus = {
  enrolled: boolean;
  takes: number;
};

export type UpdateChannel = "stable" | "beta";

export function normalizeUpdateChannel(
  raw: string | null | undefined,
): UpdateChannel {
  return raw?.trim().toLowerCase() === "beta" ? "beta" : "stable";
}

export const EMPTY_SETTINGS: AppSettings = {
  openrouter_api_key: "",
  exa_api_key: "",
  openrouter_model: "deepseek/deepseek-v4-flash-0731",
  openrouter_fast_model: "",
  openrouter_model_provider: "digitalocean",
  openrouter_fast_provider: "",
  openrouter_pin_provider: false,
  capability_preset: "full",
  input_device: "",
  output_device: "",
  tts_voice_id: "M4",
  model_residency: "balanced",
  voice_barge_in: false,
  ignore_speaker_playback: true,
  long_term_memory: true,
  trusted_auto_moderate: true,
  max_confirms_per_turn: 12,
  show_overlay_on_wake: false,
  overlay_caption_mode: "full",
  overlay_position: "top_center",
  overlay_scale_percent: 100,
  start_engine_on_launch: false,
  start_with_windows: false,
  update_channel: "stable",
  logging_filter: "",
};

/** Common OpenRouter chat models for the preset dropdown (kept current for agents). */
export const MODEL_PRESETS: { id: string; label: string }[] = [
  { id: "deepseek/deepseek-v4-flash-0731", label: "DeepSeek V4 Flash 0731 (recommended)" },
  { id: "google/gemini-3.6-flash", label: "Gemini 3.6 Flash" },
  { id: "google/gemini-3.5-flash-lite", label: "Gemini 3.5 Flash Lite (cheap/fast)" },
  { id: "deepseek/deepseek-v4-flash-latest", label: "DeepSeek V4 Flash Latest" },
  { id: "anthropic/claude-sonnet-5", label: "Claude Sonnet 5" },
  { id: "anthropic/claude-opus-5", label: "Claude Opus 5" },
  { id: "openai/gpt-5.6-sol", label: "GPT-5.6 Sol" },
  { id: "openai/gpt-5.6-terra", label: "GPT-5.6 Terra" },
  { id: "x-ai/grok-4.5", label: "Grok 4.5" },
  { id: "qwen/qwen3.8-max", label: "Qwen3.8 Max" },
  { id: "qwen/qwen3.7-flash", label: "Qwen3.7 Flash" },
  { id: "minimax/minimax-m3", label: "MiniMax M3" },
  { id: "moonshotai/kimi-k2.7-code", label: "Kimi K2.7 Code" },
];

/**
 * Common OpenRouter **model-provider** slugs (inference hosts on a model page).
 * Copy exact slug from OpenRouter when in doubt (`coreweave`, `deepinfra/turbo`).
 */
export const PROVIDER_PRESETS: { id: string; label: string }[] = [
  { id: "", label: "Auto (OpenRouter default)" },
  { id: "digitalocean", label: "DigitalOcean" },
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
  artifact: null,
  wake_enroll: null,
};

/** Normalize partial / missing Option fields from serde. */
export function normalizeStatus(
  raw: Partial<StatusPicture> | null | undefined,
): StatusPicture {
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
    artifact: raw.artifact ?? null,
    wake_enroll: raw.wake_enroll ?? null,
  };
}

/** Merge a partial settings payload into a full [`AppSettings`] with defaults. */
export function normalizeSettings(
  raw: Partial<AppSettings> | null | undefined,
): AppSettings {
  return {
    openrouter_api_key: raw?.openrouter_api_key ?? "",
    exa_api_key: raw?.exa_api_key ?? "",
    openrouter_model:
      raw?.openrouter_model?.trim() || "deepseek/deepseek-v4-flash-0731",
    openrouter_fast_model: raw?.openrouter_fast_model ?? "",
    openrouter_model_provider:
      raw?.openrouter_model_provider?.trim() || "digitalocean",
    openrouter_fast_provider: raw?.openrouter_fast_provider ?? "",
    openrouter_pin_provider: raw?.openrouter_pin_provider ?? false,
    capability_preset: raw?.capability_preset?.trim() || "full",
    input_device: raw?.input_device ?? "",
    output_device: raw?.output_device ?? "",
    tts_voice_id: raw?.tts_voice_id?.trim() || "M4",
    model_residency: normalizeResidency(raw?.model_residency),
    voice_barge_in: raw?.voice_barge_in ?? false,
    ignore_speaker_playback: raw?.ignore_speaker_playback ?? true,
    long_term_memory: raw?.long_term_memory ?? true,
    trusted_auto_moderate: raw?.trusted_auto_moderate ?? true,
    max_confirms_per_turn: normalizeMaxConfirms(raw?.max_confirms_per_turn),
    show_overlay_on_wake: raw?.show_overlay_on_wake ?? false,
    overlay_caption_mode:
      raw?.overlay_caption_mode === "assistant" ||
      raw?.overlay_caption_mode === "hidden"
        ? raw.overlay_caption_mode
        : "full",
    overlay_position:
      raw?.overlay_position === "top_left" || raw?.overlay_position === "top_right"
        ? raw.overlay_position
        : "top_center",
    overlay_scale_percent: normalizeOverlayScale(raw?.overlay_scale_percent),
    start_engine_on_launch: raw?.start_engine_on_launch ?? false,
    start_with_windows: raw?.start_with_windows ?? false,
    update_channel: normalizeUpdateChannel(raw?.update_channel),
    logging_filter: raw?.logging_filter ?? "",
  };
}

function normalizeOverlayScale(raw: number | null | undefined): number {
  if (typeof raw !== "number" || !Number.isFinite(raw)) return 100;
  return Math.min(125, Math.max(75, Math.round(raw / 5) * 5));
}

function normalizeResidency(
  raw: string | null | undefined,
): AppSettings["model_residency"] {
  const t = raw?.trim().toLowerCase();
  if (t === "low_memory" || t === "low_latency") return t;
  return "balanced";
}

function normalizeMaxConfirms(raw: number | null | undefined): number {
  if (typeof raw !== "number" || !Number.isFinite(raw)) return 12;
  const n = Math.floor(raw);
  return n < 1 ? 12 : n;
}

/** Wire shape for `save_app_settings` (always send full struct to Rust). */
export function settingsToWire(settings: AppSettings): AppSettings {
  return normalizeSettings(settings);
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
