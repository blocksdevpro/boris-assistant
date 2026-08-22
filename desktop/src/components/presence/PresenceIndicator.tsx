import type { PhaseTone } from "@/lib/phaseVisual";
import { isToolActivity } from "@/lib/statusPresentation";
import { GridPresence, type GridPresenceSize } from "./GridPresence";
import { STATE_ACCENTS, type PresenceState } from "./gridPatterns";
import { legacyStateFromMotion } from "./presenceState";

type LegacyMotion = PhaseTone["motion"];

export type PresenceIndicatorProps = {
  state?: PresenceState;
  motion?: LegacyMotion;
  accent?: string;
  reducedMotion: boolean;
  size?: GridPresenceSize;
  activity?: string | null;
  className?: string;
  ariaLabel?: string;
};

/** 3×3 LED presence. Same public API as the old orb, new grid language. */
export function PresenceIndicator({
  state,
  motion = "none",
  accent,
  reducedMotion,
  size = "sm",
  activity,
  className,
  ariaLabel,
}: PresenceIndicatorProps) {
  const resolvedState =
    state ?? legacyStateFromMotion(motion, isToolActivity(activity));

  return (
    <GridPresence
      state={resolvedState}
      accent={accent ?? STATE_ACCENTS[resolvedState]}
      reducedMotion={reducedMotion}
      size={size}
      className={className}
      ariaLabel={ariaLabel}
    />
  );
}
