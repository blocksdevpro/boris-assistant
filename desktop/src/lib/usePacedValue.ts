import { useEffect, useRef, useState } from "react";

const now = () =>
  typeof performance === "undefined" ? Date.now() : performance.now();

/**
 * Show each distinct *shape* of a value for at least `minDwellMs`.
 *
 * Values whose `keyOf` matches the shape on screen pass straight through, so
 * streamed text inside one state stays live. A new shape waits until the
 * current one has been visible for `minDwellMs`; if several arrive meanwhile,
 * only the latest is shown, and one that flips back to the visible shape is
 * never shown at all. `urgent` values skip the wait.
 */
export function usePacedValue<T>(
  value: T,
  minDwellMs: number,
  keyOf: (value: T) => string,
  urgent?: (value: T) => boolean,
): T {
  const key = keyOf(value);
  const [shownKey, setShownKey] = useState(key);
  const shownAt = useRef(now());
  const lastOfShown = useRef(value);
  const isUrgent = urgent?.(value) ?? false;

  if (key === shownKey) lastOfShown.current = value;

  useEffect(() => {
    if (key === shownKey) return;
    const wait = isUrgent
      ? 0
      : Math.max(0, minDwellMs - (now() - shownAt.current));
    const timer = window.setTimeout(() => {
      shownAt.current = now();
      setShownKey(key);
    }, wait);
    return () => window.clearTimeout(timer);
  }, [key, shownKey, isUrgent, minDwellMs]);

  return key === shownKey ? value : lastOfShown.current;
}
