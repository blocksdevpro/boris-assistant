/**
 * Invoke error helpers for the desktop bridge.
 *
 * Tauri rejections may be strings, `Error`, or opaque objects depending on
 * the host and serialization path — normalize to a single human message.
 */

/** Pull a human message out of a Tauri invoke rejection. */
export function invokeErrorMessage(err: unknown): string {
  if (err == null) return "Unknown error";
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  if (typeof err === "object") {
    const o = err as Record<string, unknown>;
    if (typeof o.message === "string") return o.message;
    if (typeof o.error === "string") return o.error;
  }
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}
