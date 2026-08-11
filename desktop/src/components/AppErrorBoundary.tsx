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
      <main className="flex min-h-screen items-center justify-center bg-[#0b0b0c] p-6 text-white">
        <div className="w-full max-w-sm rounded-2xl border border-red-300/15 bg-white/[0.04] p-5 shadow-2xl">
          <p className="text-sm font-semibold">Boris could not draw this window</p>
          <p className="mt-2 text-sm leading-relaxed text-white/55">
            Reload the window to try again. If this keeps happening, check the
            Diagnostics logs in Boris.
          </p>
          <button
            type="button"
            className="mt-4 h-9 rounded-lg bg-white px-3 text-sm font-medium text-black transition-colors hover:bg-white/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/70"
            onClick={() => window.location.reload()}
          >
            Reload window
          </button>
        </div>
      </main>
    );
  }
}
