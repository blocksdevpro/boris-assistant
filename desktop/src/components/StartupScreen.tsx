import { useEffect, useState } from "react";
import { useReducedMotion } from "framer-motion";
import { GridPresence } from "@/components/presence";

type StartupStage = "signal" | "presence" | "reveal" | "done";

const SIGNAL_HOLD_MS = 1_350;
const PRESENCE_MORPH_MS = 380;
const REVEAL_MS = 560;

export function StartupScreen({
  preview = false,
  onReveal,
  onComplete,
}: {
  preview?: boolean;
  onReveal?: () => void;
  onComplete?: () => void;
}) {
  const [stage, setStage] = useState<StartupStage>("signal");
  const reduceMotion = Boolean(useReducedMotion());

  useEffect(() => {
    if (preview) return;

    const reduceMotion = window.matchMedia(
      "(prefers-reduced-motion: reduce)",
    ).matches;
    const signalMs = reduceMotion ? 650 : SIGNAL_HOLD_MS;
    const morphMs = reduceMotion ? 0 : PRESENCE_MORPH_MS;
    const revealMs = reduceMotion ? 140 : REVEAL_MS;

    const presenceTimer = window.setTimeout(
      () => setStage("presence"),
      signalMs,
    );
    const revealTimer = window.setTimeout(() => {
      setStage("reveal");
      onReveal?.();
    }, signalMs + morphMs);
    const doneTimer = window.setTimeout(
      () => {
        setStage("done");
        onComplete?.();
      },
      signalMs + morphMs + revealMs,
    );

    return () => {
      window.clearTimeout(presenceTimer);
      window.clearTimeout(revealTimer);
      window.clearTimeout(doneTimer);
    };
  }, [onComplete, onReveal, preview]);

  if (stage === "done") return null;

  return (
    <div
      className="startup-splash"
      data-stage={stage}
      role="status"
      aria-live="polite"
      aria-atomic="true"
      aria-label="Boris is starting"
    >
      <div className="startup-splash__glow" aria-hidden="true" />
      <div className="startup-splash__lockup">
        <div className="startup-splash__mark" aria-hidden="true">
          <GridPresence
            state={stage === "signal" ? "starting" : "ready"}
            reducedMotion={reduceMotion}
            size="xl"
          />
        </div>
        <div className="startup-splash__copy">
          <p className="startup-splash__name">Boris</p>
          <p className="startup-splash__message">Starting quietly…</p>
        </div>
        <div className="startup-splash__progress" aria-hidden="true" />
      </div>
    </div>
  );
}
