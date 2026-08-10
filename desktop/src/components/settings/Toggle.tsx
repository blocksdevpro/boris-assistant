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
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative h-[31px] w-[51px] shrink-0 rounded-full p-0 transition-colors duration-200",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 focus-visible:ring-offset-2 focus-visible:ring-offset-[#1c1c1e]",
        "disabled:cursor-not-allowed disabled:opacity-45",
        checked ? "bg-[#34c759]" : "bg-white/20",
      )}
    >
      <span
        aria-hidden
        className={cn(
          "pointer-events-none absolute top-[2px] left-[2px] block size-[27px] rounded-full",
          "bg-white shadow-[0_1px_2px_rgba(0,0,0,0.35),0_1px_3px_rgba(0,0,0,0.15)]",
          "transition-transform duration-200 ease-[cubic-bezier(0.25,0.1,0.25,1)]",
          // 51 − 27 − 2 − 2 = 20px travel
          checked && "translate-x-[20px]",
        )}
      />
    </button>
  );
}
