import { OFF_STATUS } from "@/bridge";

/**
 * OVERLAY — read-only live HUD.
 * Shows phase / last heard / last said / device health dots.
 * Never calls choose_input, save_config, or start/stop.
 */
export function OverlayWindow() {
  const status = OFF_STATUS;

  return (
    <div
      data-tauri-drag-region
      className="flex h-screen flex-col items-center justify-center gap-3 overflow-hidden bg-background/95 px-4 text-foreground"
    >
      <div
        className="size-10 rounded-full border-2 border-primary/60 bg-primary/20"
        aria-hidden
      />
      <p className="text-sm font-medium tracking-tight">{status.phase}</p>
      {status.heard ? (
        <p className="max-w-full truncate text-xs text-muted-foreground">
          Heard: {status.heard}
        </p>
      ) : null}
      {status.said ? (
        <p className="max-w-full truncate text-xs text-muted-foreground">
          Said: {status.said}
        </p>
      ) : null}
      <div className="flex gap-3 text-[10px] text-muted-foreground">
        <span>
          Mic {status.mic.ok ? "●" : "○"} {status.mic.label}
        </span>
        <span>
          Spk {status.speaker.ok ? "●" : "○"} {status.speaker.label}
        </span>
      </div>
    </div>
  );
}
