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

export function TitleBar({ trailing }: { trailing?: ReactNode }) {
  const appWindow = useMemo(() => getCurrentWindow(), []);
  const [maximized, setMaximized] = useState(false);

  const refreshMaximized = useCallback(async () => {
    try {
      setMaximized(await appWindow.isMaximized());
    } catch {
      // plain Vite
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
      .catch(() => {});
    return () => unlisten?.();
  }, [appWindow, refreshMaximized]);

  return (
    <header
      data-tauri-drag-region
      className="flex h-11 shrink-0 select-none items-center border-b border-white/[0.06] bg-[#0c0d10]/80 backdrop-blur-xl"
    >
      <div
        data-tauri-drag-region
        className="flex min-w-0 flex-1 items-center gap-2.5 px-4"
      >
        <div
          data-tauri-drag-region
          className="flex size-6 items-center justify-center rounded-md bg-white/[0.06] ring-1 ring-white/10"
        >
          <img
            src="/icons/boris-mark.svg"
            alt=""
            className="pointer-events-none size-3.5 opacity-90"
            draggable={false}
          />
        </div>
        <div data-tauri-drag-region className="min-w-0">
          <p
            data-tauri-drag-region
            className="truncate text-[13px] font-semibold tracking-tight text-white"
          >
            Boris
          </p>
          <p
            data-tauri-drag-region
            className="truncate text-[10px] tracking-wide text-white/35"
          >
            Voice console
          </p>
        </div>
        {trailing ? (
          <div className="ml-3 hidden min-w-0 sm:block">{trailing}</div>
        ) : null}
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
        "inline-flex h-full w-11 items-center justify-center text-white/40 transition-colors",
        "hover:bg-white/[0.06] hover:text-white",
        "focus-visible:bg-white/[0.06] focus-visible:text-white focus-visible:outline-none",
        variant === "close" &&
          "hover:bg-red-500/90 hover:text-white focus-visible:bg-red-500/90",
      )}
    >
      {children}
    </button>
  );
}
