/**
 * Typed bridge stubs for status.
 * Later: invoke("get_status") + listen("status", …).
 * For structure only — no Rust wiring yet.
 */

import { OFF_STATUS, type StatusPicture } from "./types";

/** Pull once (overlay mount, window focus, etc.). */
export async function getStatus(): Promise<StatusPicture> {
  // TODO: return invoke<StatusPicture>("get_status");
  return OFF_STATUS;
}

/**
 * Subscribe to status pushes from Rust.
 * Returns an unsubscribe function.
 */
export async function onStatus(
  _handler: (picture: StatusPicture) => void,
): Promise<() => void> {
  // TODO: return listen<StatusPicture>("status", (e) => handler(e.payload));
  return () => {};
}
