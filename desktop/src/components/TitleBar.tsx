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

  const onTitleBarDoubleClick = useCallback(
    (event: React.MouseEvent<HTMLElement>) => {
      if (!appWindow || (event.target as HTMLElement).closest("button")) return;
      void appWindow.toggleMaximize();
    },
    [appWindow],
  );

  return (
    <header
      data-tauri-drag-region
      onDoubleClick={onTitleBarDoubleClick}
      className="title-bar relative z-40 flex h-12 shrink-0 select-none items-center border-b border-white/[0.055] bg-[#0b0b0c]/82 shadow-[0_1px_0_rgba(255,255,255,0.015)] backdrop-blur-2xl"
    >
      <div
        data-tauri-drag-region
        className="title-bar__leading flex min-w-0 flex-1 items-center gap-2 px-3"
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
        {trailing ? (
          <div className="ml-auto mr-1 flex shrink-0 items-center">
            {trailing}
          </div>
        ) : null}
      </div>

      <p
        data-tauri-drag-region
        className="title-bar__title pointer-events-none absolute left-1/2 max-w-[38%] -translate-x-1/2 truncate text-center text-[12px] font-semibold tracking-[-0.01em] text-white/72"
      >
        {title}
      </p>

      <div className="title-bar__controls mr-1 flex h-8 shrink-0 items-center gap-0.5 rounded-[10px] border border-white/[0.045] bg-white/[0.025] p-0.5">
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
        "title-bar__button inline-flex size-7 items-center justify-center rounded-[7px] text-white/38",
        "transition-[color,background-color,transform] duration-150 ease-out",
        "hover:bg-white/[0.075] hover:text-white/90 active:scale-[0.94]",
        "focus-visible:bg-white/[0.075] focus-visible:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-white/20",
        variant === "close" &&
          "hover:bg-[#ff5f57] hover:text-white focus-visible:bg-[#ff5f57]",
      )}
    >
      {children}
    </button>
  );
}
