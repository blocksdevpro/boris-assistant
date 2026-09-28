import { cn } from "@/lib/utils";

export type View = "home" | "settings" | "teach";

export type SettingsCategory =
  | "general"
  | "overlay"
  | "speech"
  | "connections"
  | "advanced";

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
    label: "Full",
    footer: "Shell, web, and files. Approvals still apply.",
  },
  {
    id: "local_power",
    label: "Local only",
    footer: "Files and system helpers. No shell or network.",
  },
  {
    id: "voice_safe",
    label: "Voice-safe",
    footer: "Light tools only — time, notes, profile, skills.",
  },
] as const;

export type CapabilityOption = (typeof CAPABILITY_OPTIONS)[number];

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
