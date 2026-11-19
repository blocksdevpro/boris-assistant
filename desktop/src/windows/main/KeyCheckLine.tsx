import { AlertCircle, CheckCircle2, LoaderCircle } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { cn } from "@/lib/utils";
import { softEase } from "@/lib/mainMotion";
import { keyCheckMessage, type KeyCheck } from "./useApiKeyCheck";

/** Animated one-line verdict under an API key field. */
export function KeyCheckLine({ check, className }: { check: KeyCheck; className?: string }) {
  const reduceMotion = useReducedMotion();
  const message = keyCheckMessage(check);
  const tone =
    check.state === "ok"
      ? "text-emerald-300/90"
      : check.state === "rejected"
        ? "text-red-300/90"
        : check.state === "no_credit" || check.state === "unreachable"
          ? "text-amber-200/85"
          : "text-[var(--boris-label-tertiary)]";
  const Icon =
    check.state === "checking"
      ? LoaderCircle
      : check.state === "ok"
        ? CheckCircle2
        : check.state === "incomplete"
          ? null
          : AlertCircle;

  return (
    <div className={cn("min-h-[20px]", className)} aria-live="polite">
      <AnimatePresence mode="wait" initial={false}>
        {message ? (
          <motion.p
            key={check.state}
            initial={reduceMotion ? { opacity: 0 } : { opacity: 0, y: 4, filter: "blur(2px)" }}
            animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
            exit={reduceMotion ? { opacity: 0 } : { opacity: 0, y: -3 }}
            transition={{ duration: reduceMotion ? 0 : 0.2, ease: softEase }}
            className={cn("flex items-start gap-1.5 text-[12.5px] leading-snug", tone)}
          >
            {Icon ? (
              <Icon
                className={cn(
                  "mt-px size-3.5 shrink-0",
                  check.state === "checking" && "animate-spin",
                )}
                strokeWidth={2.2}
              />
            ) : null}
            <span>{message}</span>
          </motion.p>
        ) : null}
      </AnimatePresence>
    </div>
  );
}
