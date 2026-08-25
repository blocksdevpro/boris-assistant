import { motion, useReducedMotion } from "framer-motion";
import { cn } from "@/lib/utils";

/**
 * Apple-style switch.
 * Fixed geometry: track 51×31, thumb 27, 2px inset — thumb never escapes.
 */
export function Toggle({
  checked,
  onChange,
  disabled,
  id,
  "aria-label": ariaLabel,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  id?: string;
  "aria-label"?: string;
}) {
  const reduceMotion = useReducedMotion();

  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      aria-disabled={disabled || undefined}
      data-state={checked ? "checked" : "unchecked"}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "settings-toggle relative h-[31px] w-[51px] shrink-0 rounded-full border p-0",
        "shadow-[0_0_0_0_rgba(255,255,255,0)] transition-[background-color,border-color,box-shadow,transform] duration-200",
        "hover:shadow-[0_0_0_3px_rgba(255,255,255,0.045)] active:scale-[0.97]",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 focus-visible:ring-offset-2 focus-visible:ring-offset-[#1c1c1e]",
        "disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:shadow-none disabled:active:scale-100",
        checked
          ? "border-[#49d66a]/80 bg-[#34c759]"
          : "border-white/[0.08] bg-white/[0.16]",
      )}
    >
      <motion.span
        aria-hidden
        initial={false}
        animate={{ x: checked ? 20 : 0, scale: checked ? 1 : 0.96 }}
        transition={
          reduceMotion
            ? { duration: 0 }
            : { type: "spring", stiffness: 520, damping: 34, mass: 0.7 }
        }
        className={cn(
          "pointer-events-none absolute left-[1px] top-[1px] block size-[27px] rounded-full",
          "bg-white shadow-[0_1px_2px_rgba(0,0,0,0.42),0_2px_5px_rgba(0,0,0,0.18)]",
        )}
      />
    </button>
  );
}
