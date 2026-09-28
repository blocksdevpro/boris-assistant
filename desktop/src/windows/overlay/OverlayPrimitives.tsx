import { useLayoutEffect, useRef, useState } from "react";
import { animate, motion } from "framer-motion";
import { AlertCircle, Mic, Volume2 } from "lucide-react";
import type { StatusPicture } from "@/bridge";
import type { Caption } from "@/lib/statusPresentation";
import { cn } from "@/lib/utils";
import { overlayContentMotion, overlayFade } from "@/lib/overlayMotion";
import { OverlayText } from "./OverlayText";

export function OverlayThoughts({
  text,
  reducedMotion,
}: {
  text: string;
  reducedMotion: boolean;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  const [overflowing, setOverflowing] = useState(false);

  useLayoutEffect(() => {
    const element = scroller.current;
    if (!element) return;

    let stopScroll: (() => void) | undefined;
    const followTail = () => {
      const nextOverflowing = element.scrollHeight > element.clientHeight + 1;
      setOverflowing((current) =>
        current === nextOverflowing ? current : nextOverflowing,
      );
      const target = Math.max(0, element.scrollHeight - element.clientHeight);
      stopScroll?.();
      if (reducedMotion) element.scrollTop = target;
      else {
        const animation = animate(element.scrollTop, target, {
          ...overlayFade,
          onUpdate: (value) => { element.scrollTop = value; },
        });
        stopScroll = () => animation.stop();
      }
    };

    followTail();
    const frame = window.requestAnimationFrame(followTail);
    const observer = new ResizeObserver(followTail);
    const content = element.firstElementChild;
    if (content) observer.observe(content);
    return () => {
      window.cancelAnimationFrame(frame);
      observer.disconnect();
      stopScroll?.();
    };
  }, [text, reducedMotion]);

  return (
    <motion.div
      data-tauri-drag-region
      className="overlay-caption overlay-thought mt-2 h-[104px] min-w-0 max-w-[324px] overflow-hidden rounded-[12px] px-2.5 py-2"
    >
      <p
        data-tauri-drag-region
        className="text-[9px] font-semibold uppercase tracking-[0.1em] text-white/48"
      >
        Live reasoning
      </p>
      <div
        ref={scroller}
        data-tauri-drag-region
        data-overflowing={overflowing}
        className="overlay-thought__body mt-1 h-[4.5rem] overflow-hidden"
      >
        <p
          data-tauri-drag-region
          className="whitespace-pre-wrap text-[12px] leading-[1.4] tracking-[-0.012em] text-white/78"
        >
          <OverlayText text={text} stream />
        </p>
      </div>
    </motion.div>
  );
}

export function CaptionBody({
  caption,
  expanded,
}: {
  caption: Caption;
  expanded: boolean;
}) {
  const color =
    caption.kind === "error"
      ? "text-red-200"
      : caption.kind === "said"
        ? "text-white/90"
        : "text-white/80";

  return (
    <p
      data-tauri-drag-region
      className={cn(
        "text-[12px] leading-[1.4] tracking-[-0.012em]",
        expanded ? "line-clamp-2" : "line-clamp-1",
        color,
      )}
    >
      {caption.kind === "error" ? (
        <AlertCircle className="mr-1 inline size-3.5 align-text-bottom" aria-hidden="true" />
      ) : (
        <span className="mr-1.5 text-[9px] font-semibold uppercase tracking-[0.1em] text-white/42">
          {caption.kind === "said" ? "Boris" : "You"}
        </span>
      )}
      <OverlayText text={caption.text} stream />
    </p>
  );
}

export function deviceFaultKey(status: StatusPicture): string | null {
  // Generic engine faults can mark both devices unhealthy even when the
  // actual failure is a model/runtime problem. The actionable fault caption
  // is more trustworthy; device badges are reserved for a running engine.
  if (status.engine !== "On" && status.engine !== "Starting") return null;
  const micBad = !status.mic.ok;
  const speakerBad = !status.speaker.ok;
  if (!micBad && !speakerBad) return null;
  if (micBad && speakerBad) return "audio";
  return micBad ? "mic" : "speaker";
}

/** Visible labels are required because the locked overlay cannot be hovered. */
export function DeviceFaultBadge({
  status,
  reducedMotion,
}: {
  status: StatusPicture;
  reducedMotion: boolean;
}) {
  const micBad = !status.mic.ok;
  const speakerBad = !status.speaker.ok;
  const both = micBad && speakerBad;
  const label = both ? "Audio" : micBad ? "Mic" : "Speaker";
  const detail = both
    ? `${status.mic.label}; ${status.speaker.label}`
    : micBad
      ? status.mic.label
      : status.speaker.label;

  return (
    <motion.span
      layout={reducedMotion ? false : "position"}
      data-tauri-drag-region
      title={detail}
      className="overlay-device-fault flex h-6 shrink-0 items-center gap-1 rounded-full border border-amber-200/25 bg-amber-300/10 px-2 text-[9px] font-semibold text-amber-50/90 shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]"
      aria-label={`${label} unavailable: ${detail}`}
      {...overlayContentMotion(reducedMotion)}
    >
      {speakerBad && !micBad ? (
        <Volume2 className="size-3" strokeWidth={2} aria-hidden="true" />
      ) : (
        <Mic className="size-3" strokeWidth={2} aria-hidden="true" />
      )}
      {label} unavailable
    </motion.span>
  );
}
