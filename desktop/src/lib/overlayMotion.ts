import type { Transition } from "framer-motion";

// One interruptible spring for the island and its contents. Exits finish
// inside the native host's 220ms hide grace period.
export const overlaySpring = {
  type: "spring", stiffness: 390, damping: 36, mass: 0.85,
} as const satisfies Transition;
export const overlayFade = {
  duration: 0.18, ease: [0.22, 1, 0.36, 1],
} as const satisfies Transition;

export function overlayContentMotion(reduced: boolean) {
  return {
    initial: reduced ? false as const : { opacity: 0, y: 5 },
    animate: { opacity: 1, y: 0 },
    exit: { opacity: 0, y: reduced ? 0 : -3, transition: { duration: reduced ? 0 : 0.12 } },
    transition: reduced ? { duration: 0 } : { ...overlaySpring, opacity: overlayFade },
  };
}
