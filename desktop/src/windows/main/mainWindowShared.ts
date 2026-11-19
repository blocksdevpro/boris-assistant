import { cn } from "@/lib/utils";

export type View = "home" | "settings" | "teach" | "debug";

/**
 * Settings pages, in sidebar order. `advanced` is the Developer page and only
 * shows when "Show advanced options" is on.
 */
export type SettingsCategory =
  | "general"
  | "speech"
  | "overlay"
  | "privacy"
  | "connections"
  | "advanced";

/** Device id meaning "use the Windows default, and follow it when it changes". */
export const SYSTEM_DEFAULT_DEVICE = "default";

/** Empty or "default" settings follow the OS default device. */
export function followsDefaultDevice(id: string | null | undefined): boolean {
  const t = id?.trim() ?? "";
  return t === "" || t.toLowerCase() === SYSTEM_DEFAULT_DEVICE;
}

export type ModelCheckState = "loading" | "ready" | "missing" | "error";

export type SaveState = "idle" | "saving" | "saved" | "error";

export type UpdateUiState =
  | "idle"
  | "checking"
  | "up_to_date"
  | "available"
  | "downloading"
  | "error";

export const CAPABILITY_OPTIONS = [
  {
    id: "full",
    label: "Everything",
    footer: "Files, apps, the web, and commands. Boris asks before anything risky.",
  },
  {
    id: "local_power",
    label: "This PC only",
    footer: "Files and apps on this computer. No internet or commands.",
  },
  {
    id: "voice_safe",
    label: "Just the basics",
    footer: "Time, notes, and what he remembers about you. Nothing else.",
  },
] as const;

export type CapabilityOption = (typeof CAPABILITY_OPTIONS)[number];

export type Residency = "low_memory" | "balanced" | "low_latency";

/** How long Boris keeps his voice models in memory, in plain words. */
export const RESIDENCY_OPTIONS: readonly {
  id: Residency;
  label: string;
  tagline: string;
  detail: string;
}[] = [
  {
    id: "low_latency",
    label: "Fastest",
    tagline: "Answers right away",
    detail: "Keeps his voice loaded while Boris is on. Uses the most memory.",
  },
  {
    id: "balanced",
    label: "Balanced",
    tagline: "Quick, then tidies up",
    detail: "Stays quick during a conversation, then frees memory when you're done.",
  },
  {
    id: "low_memory",
    label: "Light",
    tagline: "Easy on your PC",
    detail: "Frees memory after every reply. Each reply takes a moment longer to start.",
  },
];

/**
 * Best guess for this PC. WebView2 reports RAM in coarse buckets capped at
 * 8 GB, so this can only spot small machines, which is the case that matters.
 */
export function recommendedResidency(): Residency {
  const gb = (navigator as Navigator & { deviceMemory?: number }).deviceMemory;
  return typeof gb === "number" && gb > 0 && gb <= 4 ? "low_memory" : "balanced";
}

/** Compact trailing control (Tools, Voice, short picks). */
export const selectCompactClass = cn(
  "h-9 max-w-[11rem] appearance-none rounded-[10px] border border-white/[0.055]",
  "bg-white/[0.075] px-3 text-[13px] font-medium text-white/88 outline-none",
  "shadow-[0_1px_0_rgba(255,255,255,0.03)_inset] transition-[background-color,border-color,box-shadow] duration-150 hover:bg-white/[0.1]",
  "focus-visible:border-white/10 focus-visible:ring-2 focus-visible:ring-white/15",
  "disabled:cursor-not-allowed disabled:opacity-40",
);

/** Device / long labels — take available trailing width, ellipsize. */
export const selectDeviceClass = cn(
  "h-10 w-full min-w-0 max-w-full appearance-none rounded-[10px] border border-white/[0.055]",
  "bg-white/[0.075] px-3 text-[13px] text-white/88 outline-none",
  "shadow-[0_1px_0_rgba(255,255,255,0.03)_inset] transition-[background-color,border-color,box-shadow] duration-150 hover:bg-white/[0.1]",
  "focus-visible:border-white/10 focus-visible:ring-2 focus-visible:ring-white/15",
  "disabled:cursor-not-allowed disabled:opacity-40",
);

export const fieldInputClass = cn(
  "h-10 rounded-[10px] border border-white/[0.055] bg-white/[0.075] text-[14px] text-white shadow-[0_1px_0_rgba(255,255,255,0.03)_inset]",
  "placeholder:text-white/30 focus-visible:border-transparent",
  "focus-visible:ring-2 focus-visible:ring-white/15",
);

export function shortDeviceName(name: string): string {
  const match = name.match(
    /^\s*(?:Microphone|Speakers?|Headset)\s*\((.+)\)\s*$/i,
  );
  if (match?.[1]) return match[1].trim();
  return name;
}
