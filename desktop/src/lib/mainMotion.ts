import type { Transition } from "framer-motion";

/**
 * Motion for main-window cards and checklists: calm, settled, never bouncy
 * on layout; a small spring "pop" only on completion marks. Every helper
 * takes `reduced` (prefers-reduced-motion) and drops transforms then.
 */

/** The app's one ease curve: quick out of the gate, long soft landing. */
export const softEase = [0.22, 1, 0.36, 1] as const;

/** Card / banner arrival. Critically damped, so it settles without wobble. */
export const settleSpring = {
  type: "spring",
  stiffness: 260,
  damping: 30,
  mass: 0.9,
} as const satisfies Transition;

/** Completion marks: a brief, satisfying overshoot. */
export const popSpring = {
  type: "spring",
  stiffness: 520,
  damping: 22,
  mass: 0.7,
} as const satisfies Transition;

const none = { duration: 0 } as const;

/**
 * Wrapper that opens space for a card, so content below glides down
 * instead of jumping. Pair with {@link riseMotion} on the card itself.
 */
export function makeRoomMotion(reduced: boolean) {
  return {
    initial: reduced ? { opacity: 0 } : { height: 0, opacity: 0 },
    animate: reduced ? { opacity: 1 } : { height: "auto", opacity: 1 },
    exit: reduced
      ? { opacity: 0, transition: none }
      : {
          height: 0,
          opacity: 0,
          transition: {
            height: { duration: 0.32, ease: softEase, delay: 0.06 },
            opacity: { duration: 0.16, ease: softEase },
          },
        },
    transition: reduced
      ? none
      : {
          height: { duration: 0.42, ease: softEase },
          opacity: { duration: 0.3, ease: softEase },
        },
    style: { overflow: "hidden" as const },
  };
}

/** Card content: rises a few px and comes into focus. */
export function riseMotion(reduced: boolean, delay = 0) {
  return {
    initial: reduced
      ? { opacity: 0 }
      : { opacity: 0, y: 10, scale: 0.985, filter: "blur(6px)" },
    animate: reduced
      ? { opacity: 1 }
      : { opacity: 1, y: 0, scale: 1, filter: "blur(0px)" },
    exit: reduced
      ? { opacity: 0, transition: none }
      : {
          opacity: 0,
          y: -4,
          filter: "blur(4px)",
          transition: { duration: 0.18, ease: softEase },
        },
    transition: reduced
      ? none
      : {
          ...settleSpring,
          delay,
          opacity: { duration: 0.36, ease: softEase, delay },
          filter: { duration: 0.42, ease: softEase, delay },
        },
  };
}

/** Text that changes in place (headlines, details): soft crossfade-lift. */
export function swapMotion(reduced: boolean) {
  return {
    initial: reduced ? { opacity: 0 } : { opacity: 0, y: 6, filter: "blur(3px)" },
    animate: reduced ? { opacity: 1 } : { opacity: 1, y: 0, filter: "blur(0px)" },
    exit: reduced
      ? { opacity: 0, transition: none }
      : { opacity: 0, y: -6, filter: "blur(3px)", transition: { duration: 0.16, ease: softEase } },
    transition: reduced ? none : { duration: 0.32, ease: softEase },
  };
}

/** Delay for item `index` in a staggered list. */
export function stagger(index: number, reduced: boolean, step = 0.05, base = 0.08) {
  return reduced ? 0 : base + index * step;
}
