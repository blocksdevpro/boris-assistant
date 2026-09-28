import { Component, type ErrorInfo, type ReactNode } from "react";
import { logger } from "@/lib/logger";

type Props = { children: ReactNode };
type State = { error: Error | null };

/** Last-resort UI so a render failure does not leave a blank desktop window. */
export class AppErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    logger.error("frontend render failed", {
      message: error.message,
      componentStack: info.componentStack,
    });
  }

  render() {
    if (!this.state.error) return this.props.children;

    return (
      <main className="main-console flex min-h-screen items-center justify-center p-6 text-white">
        <section
          className="boris-material boris-material--elevated boris-surface-enter w-full max-w-[390px] rounded-[22px] p-6"
          aria-labelledby="app-error-title"
          aria-describedby="app-error-description"
        >
          <div className="flex size-11 items-center justify-center rounded-[14px] border border-red-300/15 bg-red-400/10 text-red-300 shadow-[inset_0_0.5px_0_rgba(255,255,255,0.08)]">
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.7"
              strokeLinecap="round"
              strokeLinejoin="round"
              className="size-5"
              aria-hidden="true"
            >
              <path d="M12 8v4.5" />
              <path d="M12 16.25h.01" />
              <path d="M10.3 3.8 2.6 17.2A2 2 0 0 0 4.35 20h15.3a2 2 0 0 0 1.74-2.8L13.7 3.8a1.96 1.96 0 0 0-3.4 0Z" />
            </svg>
          </div>

          <p className="mt-5 text-[11px] font-semibold uppercase tracking-[0.09em] text-white/35">
            Window interrupted
          </p>
          <h1
            id="app-error-title"
            className="mt-1.5 text-[20px] font-semibold tracking-[-0.035em] text-white/95"
          >
            Boris couldn’t open this view
          </h1>
          <p
            id="app-error-description"
            className="mt-2 text-[13px] leading-relaxed text-white/50"
          >
            Reload the window to recover. If it happens again, the Diagnostics
            log can help identify what went wrong.
          </p>

          <div className="mt-6 flex items-center justify-end">
            <button
              type="button"
              className="inline-flex h-9 items-center justify-center rounded-[10px] bg-white px-4 text-[13px] font-semibold tracking-[-0.01em] text-black shadow-[inset_0_0.5px_0_rgba(255,255,255,0.7),0_1px_3px_rgba(0,0,0,0.25)] transition-[background-color,transform,box-shadow] duration-200 ease-[var(--boris-ease-standard)] hover:bg-white/90 active:scale-[0.975] focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-white/25"
              onClick={() => window.location.reload()}
            >
              Reload window
            </button>
          </div>
        </section>
      </main>
    );
  }
}
