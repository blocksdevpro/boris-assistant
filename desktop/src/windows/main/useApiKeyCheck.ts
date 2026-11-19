import { useEffect, useState } from "react";
import { checkApiKey, type ApiKeyStatus } from "@/bridge";

export type KeyCheck =
  | { state: "empty" }
  | { state: "incomplete" }
  | { state: "checking" }
  | { state: "ok" }
  | { state: "no_credit" | "rejected" | "unreachable"; detail: string };

const CHECK_SETTLE_MS = 550;

/** OpenRouter keys look like `sk-or-v1-<64 hex>`; anything short is mid-paste. */
function looksComplete(key: string): boolean {
  return key.length >= 20;
}

function fromStatus(status: ApiKeyStatus): KeyCheck {
  if (status.state === "ok") return { state: "ok" };
  if (status.state === "no_credit" || status.state === "rejected") {
    return { state: status.state, detail: status.detail };
  }
  return { state: "unreachable", detail: status.detail };
}

/** Debounced live check of an OpenRouter key. Never throws. */
export function useApiKeyCheck(rawKey: string, enabled = true): KeyCheck {
  const key = rawKey.trim();
  const [result, setResult] = useState<{ key: string; check: KeyCheck } | null>(null);

  useEffect(() => {
    if (!enabled || !key || !looksComplete(key)) return;
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void checkApiKey(key).then((status) => {
        if (!cancelled) setResult({ key, check: fromStatus(status) });
      });
    }, CHECK_SETTLE_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [key, enabled]);

  if (!key) return { state: "empty" };
  if (!looksComplete(key)) return { state: "incomplete" };
  if (result?.key === key) return result.check;
  return { state: "checking" };
}

/** One plain-language line for a key check. */
export function keyCheckMessage(check: KeyCheck): string {
  switch (check.state) {
    case "empty":
      return "";
    case "incomplete":
      return "That looks too short. Paste the whole key.";
    case "checking":
      return "Checking your key…";
    case "ok":
      return "Key works. You're connected.";
    case "no_credit":
      return "This key works, but the account has no credit yet. Add a few dollars on OpenRouter.";
    case "rejected":
      return "OpenRouter didn't accept this key. Copy it again and paste the whole thing.";
    case "unreachable":
      return "Couldn't reach OpenRouter to check. You can still continue.";
  }
}
