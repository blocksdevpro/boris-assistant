import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { cancelInput, getSettings, submitInput, type InputPeek } from "@/bridge";
import { cn } from "@/lib/utils";
import { AnimatePresence, motion, useIsPresent, useReducedMotion } from "framer-motion";
import { overlayContentMotion } from "@/lib/overlayMotion";
import { OverlayText } from "@/windows/overlay/OverlayText";

const GROW_CHARS = 80;

export function CollectInputField({
  input,
  compact,
  hideHeading,
}: {
  input: InputPeek;
  compact?: boolean;
  hideHeading?: boolean;
}) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [submitWith, setSubmitWith] = useState<"enter" | "ctrl_enter">("enter");
  const reduced = Boolean(useReducedMotion());
  const present = useIsPresent();

  useEffect(() => {
    setValue("");
    setError(null);
    setBusy(false);
  }, [input.id]);

  useEffect(() => {
    let active = true;
    void getSettings()
      .then((settings) => {
        if (active) setSubmitWith(settings.typed_input_submit);
      })
      .catch(() => {
        // Browser fixtures have no settings bridge.
      });
    return () => {
      active = false;
    };
  }, [input.id]);

  const secret = input.kind === "secret";
  const blob = input.kind === "blob";
  const multiline =
    !secret && (input.multiline || blob || value.includes("\n") || value.length > GROW_CHARS);
  const max = Math.max(1, input.max_chars || 512);
  const remaining = max - [...value].length;
  const ctrlEnter = submitWith === "ctrl_enter";

  const placeholder = useMemo(() => {
    if (secret) return "Paste here. It stays hidden.";
    if (blob) return "Paste the text.";
    return "Type or paste the exact text.";
  }, [secret, blob]);

  const hint = remaining < 80
    ? `${remaining} left`
    : ctrlEnter
      ? "Ctrl+Enter sends · Esc cancels"
      : "Enter sends · Esc cancels";

  async function send() {
    const clipped = [...value].slice(0, max).join("");
    if (!clipped.trim()) {
      setError("Nothing to send.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await submitInput(input.id, clipped);
    } catch (e) {
      setBusy(false);
      setError(e instanceof Error ? e.message : "Could not send.");
    }
  }

  async function cancel() {
    setBusy(true);
    setError(null);
    try {
      await cancelInput(input.id);
    } catch (e) {
      setBusy(false);
      setError(e instanceof Error ? e.message : "Could not cancel.");
    }
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Escape") {
      e.preventDefault();
      void cancel();
      return;
    }
    if (e.key !== "Enter") return;
    const chord = e.ctrlKey || e.metaKey;
    if (ctrlEnter) {
      if (multiline && !chord) return;
    } else if (multiline && e.shiftKey) {
      return;
    }
    e.preventDefault();
    void send();
  }

  const fieldClass = cn(
    "w-full resize-none rounded-[11px] border border-white/12 bg-black/35 px-2.5 py-2 text-[13px] leading-relaxed text-white/92 outline-none",
    "placeholder:text-white/28 focus:border-white/28",
  );

  return (
    <motion.div
      inert={!present}
      layout={compact && !reduced ? "position" : false}
      data-tauri-drag-region="false"
      className={cn(
        "flex min-h-0 w-full flex-col gap-2",
        compact ? "mt-2" : "mt-3",
        compact && multiline && "flex-1",
      )}
      onMouseDown={(e) => e.stopPropagation()}
    >
      {hideHeading || compact ? null : (
        <p className="text-[11px] font-medium uppercase tracking-[0.08em] text-white/38">
          {input.label}
          {secret ? " · hidden" : null}
        </p>
      )}
      <motion.div layout={compact && !reduced ? true : false} className={cn("relative flex min-h-0 flex-col", compact && multiline && "flex-1")}>
        {multiline ? (
          <textarea
            autoFocus
            disabled={busy}
            value={value}
            maxLength={max}
            rows={compact ? 5 : 8}
            placeholder={placeholder}
            className={cn(
              fieldClass,
              compact ? "min-h-[96px] flex-1" : "min-h-[140px]",
            )}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={onKeyDown}
          />
        ) : (
          <input
            autoFocus
            disabled={busy}
            type={secret ? "password" : "text"}
            value={value}
            maxLength={max}
            placeholder={placeholder}
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            className={cn(fieldClass, "h-9")}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={onKeyDown}
          />
        )}
      </motion.div>
      <motion.div layout={compact && !reduced ? "position" : false} className="flex items-center gap-1.5">
        <span className="text-[10px] text-white/28">{compact ? <OverlayText text={hint} /> : hint}</span>
      </motion.div>
      <AnimatePresence initial={false}>
        {error ? <motion.p key="error" className="text-[11px] text-red-300/80" {...overlayContentMotion(!compact || reduced)}>{compact ? <OverlayText text={error} /> : error}</motion.p> : null}
      </AnimatePresence>
    </motion.div>
  );
}
