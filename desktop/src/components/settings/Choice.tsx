import { useId, type KeyboardEvent } from "react";
import { Check } from "lucide-react";
import { motion, useReducedMotion } from "framer-motion";
import { cn } from "@/lib/utils";

type Option<T extends string> = { id: T; label: string; detail?: string };

/** Arrow keys move the selection like a native radio group. */
function radioKeys<T extends string>(
  event: KeyboardEvent,
  options: readonly Option<T>[],
  value: T,
  onChange: (id: T) => void,
  groupId: string,
) {
  const forwards = event.key === "ArrowRight" || event.key === "ArrowDown";
  const backwards = event.key === "ArrowLeft" || event.key === "ArrowUp";
  if (!forwards && !backwards) return;
  event.preventDefault();
  const current = Math.max(0, options.findIndex((o) => o.id === value));
  const next = options[(current + (forwards ? 1 : -1) + options.length) % options.length]!;
  onChange(next.id);
  document.getElementById(`${groupId}-${next.id}`)?.focus();
}

/** Compact pill switcher for two to four short choices (Performance, Captions). */
export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  disabled,
  "aria-label": ariaLabel,
}: {
  options: readonly Option<T>[];
  value: T;
  onChange: (id: T) => void;
  disabled?: boolean;
  "aria-label": string;
}) {
  const groupId = useId();
  const reduceMotion = useReducedMotion();
  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      onKeyDown={(e) => radioKeys(e, options, value, onChange, groupId)}
      className={cn(
        "settings-segmented grid w-full gap-1 rounded-[11px] border border-white/[0.05] bg-black/20 p-1",
        disabled && "opacity-45",
      )}
      style={{ gridTemplateColumns: `repeat(${options.length}, minmax(0, 1fr))` }}
    >
      {options.map((option) => {
        const active = option.id === value;
        return (
          <button
            key={option.id}
            id={`${groupId}-${option.id}`}
            type="button"
            role="radio"
            aria-checked={active}
            tabIndex={active ? 0 : -1}
            disabled={disabled}
            onClick={() => onChange(option.id)}
            className={cn(
              "relative min-h-8 rounded-[8px] px-2 text-[12.5px] font-medium transition-colors duration-150",
              "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-white/25",
              active ? "text-white" : "text-white/50 hover:text-white/80",
            )}
          >
            {active ? (
              <motion.span
                layoutId={`${groupId}-pill`}
                aria-hidden
                className="absolute inset-0 rounded-[8px] border border-white/[0.07] bg-white/[0.12] shadow-[0_1px_0_rgba(255,255,255,0.05)_inset,0_2px_8px_rgba(0,0,0,0.2)]"
                transition={
                  reduceMotion ? { duration: 0 } : { type: "spring", stiffness: 480, damping: 36 }
                }
              />
            ) : null}
            <span className="relative">{option.label}</span>
          </button>
        );
      })}
    </div>
  );
}

/** Full-width radio rows with a short explanation each (Tool access). */
export function ChoiceList<T extends string>({
  options,
  value,
  onChange,
  disabled,
  "aria-label": ariaLabel,
}: {
  options: readonly Option<T>[];
  value: T;
  onChange: (id: T) => void;
  disabled?: boolean;
  "aria-label": string;
}) {
  const groupId = useId();
  const reduceMotion = useReducedMotion();
  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      onKeyDown={(e) => radioKeys(e, options, value, onChange, groupId)}
    >
      {options.map((option, index) => {
        const active = option.id === value;
        return (
          <button
            key={option.id}
            id={`${groupId}-${option.id}`}
            type="button"
            role="radio"
            aria-checked={active}
            tabIndex={active ? 0 : -1}
            disabled={disabled}
            onClick={() => onChange(option.id)}
            className={cn(
              "flex w-full items-center gap-3 px-4 py-3 text-left transition-colors duration-150",
              "hover:bg-white/[0.03] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-white/20",
              "disabled:cursor-not-allowed disabled:opacity-45",
              index < options.length - 1 && "border-b border-white/[0.06]",
            )}
          >
            <span className="min-w-0 flex-1">
              <span
                className={cn(
                  "block text-[14px] font-medium tracking-[-0.01em]",
                  active ? "text-white" : "text-white/80",
                )}
              >
                {option.label}
              </span>
              {option.detail ? (
                <span className="mt-0.5 block text-[12px] leading-snug text-[var(--boris-label-tertiary)]">
                  {option.detail}
                </span>
              ) : null}
            </span>
            <span
              aria-hidden
              className={cn(
                "flex size-5 shrink-0 items-center justify-center rounded-full border transition-colors duration-150",
                active ? "border-sky-300 bg-sky-300" : "border-white/20",
              )}
            >
              {active ? (
                <motion.span
                  initial={reduceMotion ? false : { scale: 0.4, opacity: 0 }}
                  animate={{ scale: 1, opacity: 1 }}
                  transition={
                    reduceMotion ? { duration: 0 } : { type: "spring", stiffness: 520, damping: 22 }
                  }
                >
                  <Check className="size-3 text-[#101216]" strokeWidth={3} />
                </motion.span>
              ) : null}
            </span>
          </button>
        );
      })}
    </div>
  );
}
