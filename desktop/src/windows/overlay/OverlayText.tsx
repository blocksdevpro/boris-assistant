import { useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import {
  OVERLAY_LABEL_DWELL_MS,
  overlayContentMotion,
  overlayFade,
} from "@/lib/overlayMotion";
import { usePacedValue } from "@/lib/usePacedValue";

const identity = (text: string) => text;

/** Replace labels as a unit; streamed text keeps its container across rolling tails. */
export function OverlayText({ text: liveText, stream = false }: { text: string; stream?: boolean }) {
  const reduced = Boolean(useReducedMotion());
  // Labels (tool names, hints) can change several times a second. Each one
  // stays up long enough for its crossfade to finish instead of stacking.
  const pacedText = usePacedValue(liveText, OVERLAY_LABEL_DWELL_MS, identity);
  const text = stream || reduced ? liveText : pacedText;
  const [snapshot, setSnapshot] = useState({ text, revision: 0 });
  let revision = snapshot.revision;
  if (snapshot.text !== text) {
    if (!stream) revision += 1;
    setSnapshot({ text, revision });
  }
  if (reduced) return <>{text}</>;

  // Keep the animated tail bounded, even for a long reasoning stream.
  const words = stream ? text.match(/\s+|\S+/g) ?? [] : [];
  const tail = Math.max(0, words.length - 48);
  return (
    <span className="overlay-text" data-stream={stream || undefined}>
      <AnimatePresence initial={false} mode="popLayout">
        <motion.span key={revision} className="overlay-text__value" {...overlayContentMotion(false)}>
          {stream ? <>
            {words.slice(0, tail).join("")}
            {words.slice(tail).map((word, index) => (
              <motion.span
                key={tail + index}
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                transition={{ ...overlayFade, duration: 0.36 }}
              >
                {word}
              </motion.span>
            ))}
          </> : text}
        </motion.span>
      </AnimatePresence>
    </span>
  );
}
