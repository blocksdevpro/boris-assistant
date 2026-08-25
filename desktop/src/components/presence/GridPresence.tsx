import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { motion, useReducedMotion } from "framer-motion";
import { cn } from "@/lib/utils";
import {
  playForState,
  STATE_ACCENTS,
  type GridMatrix,
  type PresenceState,
} from "./gridPatterns";
import "./gridPresence.css";

export type GridPresenceSize = "sm" | "md" | "lg" | "xl";

export type GridPresenceProps = {
  state: PresenceState;
  accent?: string;
  reducedMotion: boolean;
  size?: GridPresenceSize;
  className?: string;
  ariaLabel?: string;
};

const CELL_KEYS = ["tl", "tm", "tr", "ml", "mm", "mr", "bl", "bm", "br"] as const;

export function GridPresence({
  state,
  accent,
  reducedMotion,
  size = "sm",
  className,
  ariaLabel,
}: GridPresenceProps) {
  const play = playForState(state);
  const prefersReduce = Boolean(useReducedMotion());
  const freeze = reducedMotion || prefersReduce || play.mode === "static";
  const [frame, setFrame] = useState(0);

  useEffect(() => {
    setFrame(0);
  }, [state]);

  useEffect(() => {
    if (freeze || play.mode !== "sequence" || play.frames.length < 2) return;
    const id = window.setInterval(() => {
      setFrame((current) => (current + 1) % play.frames.length);
    }, play.speed);
    return () => window.clearInterval(id);
  }, [freeze, play.mode, play.frames.length, play.speed, state]);

  const matrix: GridMatrix = play.frames[frame % play.frames.length] ?? play.frames[0];
  const cells = matrix.flat();
  const color = accent ?? STATE_ACCENTS[state];
  const cycle = Math.max(play.speed, 1) / 1000;

  const staggerDelay = useMemo(() => {
    if (play.mode !== "stagger") return cells.map(() => 0);
    const order =
      play.staggerOrder ??
      cells.flatMap((cell, index) => (cell === 1 ? [index] : []));
    const step = cycle / (order.length + 2);
    const rankByCell = new Map(order.map((index, rank) => [index, rank]));

    return cells.map((cell, index) =>
      cell === 1 ? (rankByCell.get(index) ?? order.length) * step : 0,
    );
  }, [cells, cycle, play.mode, play.staggerOrder]);

  return (
    <span
      className={cn("led-grid", className)}
      data-size={size}
      data-state={state}
      data-tauri-drag-region
      style={{ ["--led-accent" as string]: color } as CSSProperties}
      role={ariaLabel ? "img" : undefined}
      aria-label={ariaLabel}
      aria-hidden={ariaLabel ? undefined : true}
    >
      {cells.map((cell, index) => {
        const on = cell === 1;
        return (
          <motion.span
            key={CELL_KEYS[index]}
            className="led-grid__cell"
            data-on={on ? "true" : "false"}
            data-tauri-drag-region
            initial={false}
            animate={
              freeze
                ? { opacity: on ? 1 : 0, scale: 1 }
                : on
                  ? play.mode === "pulse"
                    ? { opacity: [0.38, 1, 0.38], scale: [0.92, 1, 0.92] }
                    : play.mode === "stagger"
                      ? { opacity: [0.15, 1, 1, 0.15], scale: [0.82, 1, 1, 0.82] }
                      : { opacity: 1, scale: 1 }
                  : { opacity: 0, scale: 0.78 }
            }
            transition={
              freeze
                ? { duration: 0 }
                : on && (play.mode === "pulse" || play.mode === "stagger")
                  ? {
                      duration: cycle,
                      ease: [0.645, 0.045, 0.355, 1],
                      repeat: Number.POSITIVE_INFINITY,
                      delay: staggerDelay[index],
                    }
                  : { duration: 0.16, ease: "easeOut" }
            }
          />
        );
      })}
    </span>
  );
}
