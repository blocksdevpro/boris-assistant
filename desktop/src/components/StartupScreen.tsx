import { useEffect, useState } from "react";

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
      aria-label="Boris is starting"
    >
      <div className="startup-splash__glow" aria-hidden="true" />
      <div className="startup-splash__lockup">
        <div className="startup-splash__mark" aria-hidden="true">
          <svg viewBox="0 0 64 64" fill="none">
            <g stroke="currentColor" strokeWidth="4" strokeLinecap="round">
              <line className="startup-splash__bar" x1="10" y1="28" x2="10" y2="36" />
              <line className="startup-splash__bar" x1="20" y1="20" x2="20" y2="44" />
              <line className="startup-splash__bar" x1="28" y1="13" x2="28" y2="51" />
              <line className="startup-splash__bar" x1="36" y1="24" x2="36" y2="40" />
              <line className="startup-splash__bar" x1="44" y1="18" x2="44" y2="46" />
              <line className="startup-splash__bar" x1="54" y1="28" x2="54" y2="36" />
            </g>
          </svg>
          <span className="startup-splash__presence" />
        </div>
        <div className="startup-splash__copy">
          <p className="startup-splash__name">Boris</p>
          <p className="startup-splash__message">Starting quietly…</p>
        </div>
      </div>
    </div>
  );
}
