import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { isTauriRuntime } from "@/lib/runtime";

type NativeWindow = ReturnType<typeof getCurrentWindow>;

function currentNativeWindow(): NativeWindow | null {
  if (!isTauriRuntime()) return null;
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

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

export function TitleBar({
  title = "Boris",
  leading,
  trailing,
}: {
  title?: string;
  leading?: ReactNode;
  trailing?: ReactNode;
}) {
  // Never construct a Tauri window handle in a plain Vite tab. Tauri's API
  // object can be imported in a browser, but calling it requires IPC globals.
  const appWindow = useMemo(currentNativeWindow, []);
  const [maximized, setMaximized] = useState(false);

  const refreshMaximized = useCallback(async () => {
    if (!appWindow) return;
    try {
      setMaximized(await appWindow.isMaximized());
    } catch {
      // plain Vite
    }
  }, [appWindow]);

  useEffect(() => {
    if (!appWindow) return;
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
      className="flex h-12 shrink-0 select-none items-center border-b border-white/[0.06] bg-[#0b0b0c]/90 backdrop-blur-xl"
    >
      <div
        data-tauri-drag-region
        className="flex min-w-0 flex-1 items-center gap-2 px-3"
      >
        {leading ? (
          <div className="shrink-0">{leading}</div>
        ) : (
          <div
            data-tauri-drag-region
            className="flex size-6 items-center justify-center rounded-md bg-white/[0.06]"
          >
            <img
              src="/icons/boris-mark.svg"
              alt=""
              className="pointer-events-none size-3.5"
              draggable={false}
            />
          </div>
        )}
        <p
          data-tauri-drag-region
          className="truncate text-[13px] font-semibold tracking-tight text-white/90"
        >
          {title}
        </p>
        {trailing ? (
          <div className="ml-auto mr-1 flex shrink-0 items-center">{trailing}</div>
        ) : null}
      </div>

      <div className="flex h-full shrink-0">
        <TitleBarButton
          aria-label="Minimize"
          onClick={() => void appWindow?.minimize()}
        >
          <Minus className="size-3.5" strokeWidth={1.75} />
        </TitleBarButton>
        <TitleBarButton
          aria-label={maximized ? "Restore" : "Maximize"}
          onClick={() => void appWindow?.toggleMaximize()}
        >
          {maximized ? (
            <RestoreIcon className="size-3" />
          ) : (
            <Square className="size-3" strokeWidth={1.75} />
          )}
        </TitleBarButton>
        <TitleBarButton
          aria-label="Close to tray"
          title="Close to system tray (app keeps running)"
          variant="close"
          onClick={() => void appWindow?.close()}
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
  title,
  variant = "default",
}: {
  children: ReactNode;
  onClick: () => void;
  "aria-label": string;
  title?: string;
  variant?: "default" | "close";
}) {
  return (
    <button
      type="button"
      aria-label={ariaLabel}
      title={title ?? ariaLabel}
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
