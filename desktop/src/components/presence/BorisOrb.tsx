import { Component, useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { ThinkingOrb } from "thinking-orbs";
import { cn } from "@/lib/utils";
import { isWorkingOrbState, ORB_PRESENTATIONS, type BorisOrbState } from "./orbState";
import "./borisOrb.css";

export type BorisOrbProps = {
  state: BorisOrbState;
  size?: "sm" | "md" | "lg" | "xl";
  reducedMotion?: boolean;
  paused?: boolean;
  active?: boolean;
  accent?: string;
  className?: string;
  ariaLabel?: string;
};

class OrbBoundary extends Component<{ children: ReactNode; onFailure: () => void }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  componentDidCatch() { this.props.onFailure(); }
  render() { return this.state.failed ? null : this.props.children; }
}

const ORB_ENTER = { duration: 0.46, ease: [0.32, 0.72, 0, 1] } as const;
const ORB_EXIT = { duration: 0.34, ease: [0.32, 0.72, 0, 1] } as const;

/** Decorative canvas plus a persistent DOM fallback; surrounding UI owns live announcements. */
export function BorisOrb({
  state, size = "sm", reducedMotion = false, paused = false, active = true,
  accent, className, ariaLabel,
}: BorisOrbProps) {
  const prefersReducedMotion = Boolean(useReducedMotion());
  const host = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(true);
  const [pageVisible, setPageVisible] = useState(() => typeof document === "undefined" || !document.hidden);
  const [rendered, setRendered] = useState(false);
  const [failed, setFailed] = useState(false);
  const [stableState, setStableState] = useState(state);

  // Only defer changes within a thinking turn. Listening, privacy, errors and
  // confirmation always update immediately, with current semantic text.
  useEffect(() => {
    if (state === stableState) return;
    if (!isWorkingOrbState(state) || !isWorkingOrbState(stableState)) {
      setStableState(state);
      return;
    }
    const timer = window.setTimeout(() => setStableState(state), 420);
    return () => window.clearTimeout(timer);
  }, [state, stableState]);

  useEffect(() => {
    const updateVisibility = () => setPageVisible(!document.hidden);
    document.addEventListener("visibilitychange", updateVisibility);
    const observer = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver(
      ([entry]) => setVisible(entry.isIntersecting),
    );
    if (host.current) observer?.observe(host.current);
    return () => {
      observer?.disconnect();
      document.removeEventListener("visibilitychange", updateVisibility);
    };
  }, []);

  useEffect(() => {
    // ThinkingOrb can silently return a blank canvas when initialization fails.
    // Inspect painted alpha after initialization, never in the animation loop.
    const canvas = host.current?.querySelector("canvas");
    const onContextLost = () => setFailed(true);
    canvas?.addEventListener("contextlost", onContextLost);
    const timer = window.setTimeout(() => {
      try {
        const context = canvas?.getContext("2d");
        if (!canvas || !context || !canvas.width || !canvas.height) {
          setFailed(true);
          return;
        }
        const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
        const hasInk = pixels.some((value, index) => index % 4 === 3 && value > 0);
        setRendered(hasInk);
        setFailed(!hasInk);
      } catch {
        setFailed(true);
      }
    }, 80);
    return () => {
      window.clearTimeout(timer);
      canvas?.removeEventListener("contextlost", onContextLost);
    };
  }, []);

  const visualState = isWorkingOrbState(state) && isWorkingOrbState(stableState) ? stableState : state;
  const visual = ORB_PRESENTATIONS[visualState];
  const still = reducedMotion || prefersReducedMotion;
  const freeze = paused || !active || !visible || !pageVisible || still || Boolean(visual.paused);

  return (
    <span
      ref={host}
      className={cn("boris-orb", className)}
      data-size={size}
      data-state={state}
      data-paused={freeze}
      data-rendered={rendered && !failed}
      style={accent ? { "--orb-accent": accent } as CSSProperties : undefined}
      role="img"
      aria-label={ariaLabel ?? ORB_PRESENTATIONS[state].label}
    >
      <span className="boris-orb__fallback" aria-hidden="true" />
      {!failed ? (
        <OrbBoundary onFailure={() => setFailed(true)}>
          {/* The canvas cuts between animations and phase-jumps when its
              speed changes, so each look gets its own layer and the two
              dissolve into each other like Siri's orb changing mood. */}
          <AnimatePresence initial={false}>
            <motion.span
              key={`${visual.animation}:${visual.speed}`}
              className="boris-orb__layer"
              initial={still ? false : { opacity: 0, scale: 0.72, filter: "blur(1.5px)" }}
              animate={{ opacity: 1, scale: 1, filter: "blur(0px)", transitionEnd: { filter: "none" } }}
              exit={still
                ? { opacity: 0, transition: { duration: 0 } }
                : { opacity: 0, scale: 1.18, filter: "blur(1.5px)", transition: ORB_EXIT }}
              transition={still ? { duration: 0 } : ORB_ENTER}
            >
              <ThinkingOrb
                state={visual.animation}
                size={size === "sm" || size === "md" ? 20 : 64}
                speed={visual.speed}
                theme="dark"
                paused={freeze}
                aria-hidden="true"
                className="boris-orb__canvas"
              />
            </motion.span>
          </AnimatePresence>
        </OrbBoundary>
      ) : null}
    </span>
  );
}
