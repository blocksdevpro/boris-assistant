import { useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { overlayContentMotion, overlayFade } from "@/lib/overlayMotion";

/** Replace labels as a unit; append streamed words without replaying old text. */
export function OverlayText({ text, stream = false }: { text: string; stream?: boolean }) {
  const reduced = Boolean(useReducedMotion());
  const [snapshot, setSnapshot] = useState({ text, revision: 0 });
  let revision = snapshot.revision;
  if (snapshot.text !== text) {
    if (!stream || !text.startsWith(snapshot.text)) revision += 1;
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
              <motion.span key={tail + index} initial={{ opacity: 0 }} animate={{ opacity: 1 }} transition={overlayFade}>
                {word}
              </motion.span>
            ))}
          </> : text}
        </motion.span>
      </AnimatePresence>
    </span>
  );
}
