/** Short display-label rules shared by the conversation and overlay views. */

/** "Searching" + "Searching weather in Bengaluru" → keep the query as the subtitle. */
export function stripEchoedVerb(primary: string, secondary: string): string {
  const p = primary.trim();
  const s = secondary.trim();
  if (!p || !s) return s;
  const re = new RegExp(`^${p.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s+`, "i");
  const stripped = s.replace(re, "").trim();
  return stripped || s;
}

export function isEchoOfThinking(line: string): boolean {
  const n = normalizeLabel(line);
  return n === "thinking" || n === "working" || n === "on it";
}

/** Strip punctuation / case for title↔subtitle equality checks. */
export function normalizeLabel(s: string): string {
  return s
    .trim()
    .toLowerCase()
    .replace(/[….]+$/g, "")
    .replace(/\s+/g, " ");
}

/**
 * Hide secondary when it only restates the primary
 * (e.g. "Working" + "Working…", "Researching" + "Researching…").
 */
export function dedupeSecondary(primary: string, secondary: string): string {
  const sec = secondary.trim();
  if (!sec) return "";
  const p = normalizeLabel(primary);
  const s = normalizeLabel(sec);
  if (!p) return sec;
  if (s === p) return "";
  // "Working…" / "Working..." vs "Working"
  if (s.replace(/[…\.]+$/g, "") === p) return "";
  // Secondary is only primary with trailing filler
  if (s.startsWith(p) && /^[\s….]*$/.test(sec.slice(primary.trim().length))) {
    return "";
  }
  return sec;
}
