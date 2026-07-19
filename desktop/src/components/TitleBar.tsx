import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X } from "lucide-react";
import { cn } from "@/lib/utils";

function RestoreIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 12 12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.25"
      className={className}
      aria-hidden
    >
      <rect x="3" y="1.5" width="7.5" height="7.5" rx="0.5" />
      <path d="M1.5 3.5v7h7" />
    </svg>
  );
}

export function TitleBar() {
  const appWindow = useMemo(() => getCurrentWindow(), []);
  const [maximized, setMaximized] = useState(false);

  const refreshMaximized = useCallback(async () => {
    try {
      setMaximized(await appWindow.isMaximized());
    } catch {
      // Not running under Tauri (plain Vite) — ignore.
    }
  }, [appWindow]);

  useEffect(() => {
    void refreshMaximized();
    let unlisten: (() => void) | undefined;

    void appWindow
      .onResized(() => {
        void refreshMaximized();
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {
        // plain browser
      });

    return () => {
      unlisten?.();
    };
  }, [appWindow, refreshMaximized]);

  return (
    <header
      data-tauri-drag-region
      className="flex h-10 shrink-0 select-none items-center border-b border-border bg-background"
    >
      <div
        data-tauri-drag-region
        className="flex min-w-0 flex-1 items-center gap-2 px-3"
      >
        <img
          src="/icons/boris-mark.svg"
          alt=""
          className="pointer-events-none size-4 shrink-0"
          draggable={false}
        />
        <span
          data-tauri-drag-region
          className="truncate text-xs font-medium tracking-tight text-foreground"
        >
          Boris
        </span>
      </div>

      <div className="flex h-full shrink-0">
        <TitleBarButton
          aria-label="Minimize"
          onClick={() => void appWindow.minimize()}
        >
          <Minus className="size-3.5" strokeWidth={1.75} />
        </TitleBarButton>
        <TitleBarButton
          aria-label={maximized ? "Restore" : "Maximize"}
          onClick={() => void appWindow.toggleMaximize()}
        >
          {maximized ? (
            <RestoreIcon className="size-3" />
          ) : (
            <Square className="size-3" strokeWidth={1.75} />
          )}
        </TitleBarButton>
        <TitleBarButton
          aria-label="Close"
          variant="close"
          onClick={() => void appWindow.close()}
        >
          <X className="size-3.5" strokeWidth={1.75} />
        </TitleBarButton>
      </div>
    </header>
  );
}

function TitleBarButton({
  children,
  onClick,
  "aria-label": ariaLabel,
  variant = "default",
}: {
  children: ReactNode;
  onClick: () => void;
  "aria-label": string;
  variant?: "default" | "close";
}) {
  return (
    <button
      type="button"
      aria-label={ariaLabel}
      onClick={onClick}
      className={cn(
        "inline-flex h-full w-11 items-center justify-center text-muted-foreground transition-colors",
        "hover:bg-muted hover:text-foreground",
        "focus-visible:bg-muted focus-visible:text-foreground focus-visible:outline-none",
        variant === "close" &&
          "hover:bg-destructive hover:text-white focus-visible:bg-destructive focus-visible:text-white",
      )}
    >
      {children}
    </button>
  );
}
