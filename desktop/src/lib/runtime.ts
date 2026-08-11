/** True only inside a Tauri webview, never in a regular Vite browser tab. */
export function isTauriRuntime(): boolean {
  if (typeof window === "undefined") return false;

  // Tauri v2 installs __TAURI_INTERNALS__. Keep the legacy marker as a
  // defensive fallback for development builds that expose `withGlobalTauri`.
  return "__TAURI_INTERNALS__" in window || "__TAURI__" in window;
}
