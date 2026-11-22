import type { Transition } from "framer-motion";

/** Apple's standard "decelerate into place" curve (iOS sheets, Siri). */
export const appleEase = [0.32, 0.72, 0, 1] as const;

// The island's material morph: a soft, slightly bouncy spring like the
// Dynamic Island. `visualDuration` keeps the settle time readable no matter
// how far the shell travels, and springs keep velocity when interrupted.
export const overlaySpring = {
  type: "spring", visualDuration: 0.5, bounce: 0.14,
} as const satisfies Transition;

/** Content inside the island: same feel, no overshoot (text must not wobble). */
export const overlayContentSpring = {
  type: "spring", visualDuration: 0.42, bounce: 0,
} as const satisfies Transition;

export const overlayFade = {
  duration: 0.28, ease: appleEase,
} as const satisfies Transition;

// Exits finish inside the native host's 220ms hide grace period.
export const overlayExit = {
  duration: 0.18, ease: [0.4, 0, 0.6, 1],
} as const satisfies Transition;

/** A new shape is held at least this long so fast phase hops read as motion, not flicker. */
export const OVERLAY_MIN_DWELL_MS = 340;
/** Rapidly changing labels (tool names, hints) hold this long before swapping. */
export const OVERLAY_LABEL_DWELL_MS = 300;

/**
 * Siri-style content swap: incoming content rises out of a soft blur while
 * outgoing content blurs away in place, so a crossfade never shows two sharp
 * overlapping copies.
 */
export function overlayContentMotion(reduced: boolean) {
  if (reduced) {
    return {
      initial: false as const,
      animate: { opacity: 1 },
      exit: { opacity: 0, transition: { duration: 0 } },
      transition: { duration: 0 },
    };
  }
  return {
    initial: { opacity: 0, y: 4, scale: 0.985, filter: "blur(4px)" },
    // Drop the settled filter so resting text is not kept on a filter layer.
    animate: {
      opacity: 1, y: 0, scale: 1, filter: "blur(0px)",
      transitionEnd: { filter: "none" },
    },
    exit: {
      opacity: 0,
      y: -2,
      scale: 0.985,
      filter: "blur(4px)",
      transition: overlayExit,
    },
    transition: {
      ...overlayContentSpring,
      opacity: overlayFade,
      filter: overlayFade,
    },
  };
}
