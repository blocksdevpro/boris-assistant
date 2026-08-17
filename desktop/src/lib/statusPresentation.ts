/**
 * Shared status → UI presentation for main window + overlay.
 *
 * Pipeline keeps last-turn `heard`/`said` and free-string `activity` for the host.
 * These helpers phase-gate captions so the UI never shows stale STT as “current”.
 */

import type { StatusPicture } from "@/bridge";

export type CaptionKind = "heard" | "said" | "error";

export type Caption = {
  kind: CaptionKind;
  text: string;
};

/** Confirm path: phase or activity string from pipeline (`confirm · …`). */
export function isConfirmContext(status: StatusPicture): boolean {
  if (status.phase === "AwaitingConfirm") return true;
  const a = status.activity?.trim().toLowerCase() ?? "";
  return a.startsWith("confirm");
}

/** Tool-ish activity (not plain “thinking…”). Multi-tool counts count as busy. */
export function isToolActivity(activity: string | null | undefined): boolean {
  const a = activity?.trim().toLowerCase() ?? "";
  if (!a) return false;
  if (a === "thinking…" || a === "thinking...") return false;
  if (a.startsWith("confirm")) return false;
  // "3 tools" / multi-tool sticky — still tool progress while working
  if (/^\d+\s+tools?$/.test(a)) return true;
  // Pure LLM "next action" thinking is not tool activity
  if (/^thinking\s*[·.]\s*(next action|step\s*\d+|round\s*\d+)$/i.test(a)) {
    return false;
  }
  return (
    a.startsWith("tool") ||
    a.startsWith("done") ||
    a.startsWith("fail") ||
    /tools?\s+next|calling tools/i.test(a) ||
    /^thinking\s*[·.]\s*after\b/i.test(a) ||
    a.startsWith("thinking ·") ||
    a.startsWith("thinking.")
  );
}

/**
 * Map pipeline activity strings to short human secondary lines.
 * Prefer *what* is happening (tool + query) over empty “Planning step N”.
 */
export function humanizeActivity(
  activity: string | null | undefined,
): string | null {
  const raw = activity?.trim();
  if (!raw) return null;

  // Multi-tool batch count (pipeline sticky or host summary)
  const multi = raw.match(/^(\d+)\s+tools?$/i);
  if (multi) {
    const n = multi[1];
    return n === "1" ? "Ran 1 tool this turn" : `Ran ${n} tools this turn`;
  }

  const lower = raw.toLowerCase();

  if (lower === "thinking…" || lower === "thinking...") {
    return "Thinking…";
  }

  // thinking · …
  if (lower.startsWith("thinking")) {
    const rest = raw.replace(/^thinking\s*[·.]\s*/i, "").trim();
    if (!rest) return "Thinking…";

    // Legacy step/round numbers → meaningful copy (no more "Planning step 3")
    if (/^(?:step|round)\s*\d+/i.test(rest)) {
      return "Choosing next action…";
    }
    // "3 tools next" / "1 tool next"
    const nextTools = rest.match(/^(\d+)\s+tools?\s+next$/i);
    if (nextTools) {
      const n = nextTools[1];
      return n === "1" ? "About to run 1 tool…" : `About to run ${n} tools…`;
    }
    if (/^calling tools$/i.test(rest)) return "About to run tools…";
    if (/^next action$/i.test(rest)) return "Choosing next action…";
    // "after web_search, web_fetch"
    const after = rest.match(/^after\s+(.+)$/i);
    if (after) {
      const names = after[1]!
        .split(",")
        .map((s) => friendlyTool(s.trim()))
        .filter(Boolean)
        .join(", ");
      return names ? `Thinking after ${names}…` : "Choosing next action…";
    }
    // drafting / other hints
    const short = rest.length > 48 ? `${rest.slice(0, 46)}…` : rest;
    return short.charAt(0).toUpperCase() + short.slice(1);
  }

  // confirm · name | confirm · say yes or no | confirm · listening
  if (lower.startsWith("confirm")) {
    const rest = raw.replace(/^confirm\s*[·.]\s*/i, "").trim();
    if (!rest) return "Waiting for your yes";
    if (/yes|no|sure|cancel|listen/i.test(rest)) return "Your turn — say yes or no";
    return `Approve ${friendlyTool(rest)}?`;
  }

  // fail · name
  const fail = raw.match(/^fail\s*[·.]\s*(.+)$/i);
  if (fail) return `${friendlyTool(fail[1]!.trim())} failed`;

  // done · name
  const done = raw.match(/^done\s*[·.]\s*(.+)$/i);
  if (done) return `Finished ${friendlyTool(done[1]!.trim())}`;

  // tool · name · msg | tool · name
  const tool = raw.match(/^tool\s*[·.]\s*([^·.]+)(?:\s*[·.]\s*(.+))?$/i);
  if (tool) {
    const name = friendlyTool(tool[1]!.trim());
    const msg = tool[2]?.trim();
    if (msg) {
      // Subagent nests "via bash: …" — keep readable
      if (/^via\s+/i.test(msg) || /^research:/i.test(msg) || /^step\s+\d+/i.test(msg)) {
        const short = msg.length > 52 ? `${msg.slice(0, 50)}…` : msg;
        return name === "Subagent" ? short : `${name}: ${short}`;
      }
      const short = msg.length > 52 ? `${msg.slice(0, 50)}…` : msg;
      return `${name}: ${short}`;
    }
    return name === "Subagent" ? "Researching…" : `Running ${name}…`;
  }

  // Unknown — soft-sanitize mid-dots
  return raw.replace(/\s*·\s*/g, " · ").slice(0, 64);
}

function friendlyTool(name: string): string {
  const n = name.trim().toLowerCase();
  if (!n) return "tool";
  // Product names for common tools (avoid Title Case noise)
  const known: Record<string, string> = {
    spawn_subagent: "Subagent",
    web_fetch: "Web",
    web_search: "Search",
    bash: "Bash",
    read_file: "Read",
    file_read: "Read",
    write_file: "Write",
    file_write: "Write",
    file_edit: "Edit",
    list_dir: "List",
    grep: "Grep",
    glob: "Find files",
    open: "Open",
    open_url: "Open link",
    open_path: "Open file",
    load_skill: "Skill",
    list_skills: "Skills",
    todo_write: "Todos",
    todo_read: "Todos",
    remember_note: "Note",
    recall_notes: "Notes",
    memory_search: "Memory",
    memory_get: "Memory",
    get_user_context: "Profile",
    save_user_fact: "Profile",
    update_user_profile: "Profile",
    get_time: "Time",
    get_date: "Date",
    get_system_info: "System",
    clipboard_get: "Clipboard",
    clipboard_set: "Clipboard",
  };
  if (known[n]) return known[n];
  // bash, web_fetch, write_file → readable
  return name
    .trim()
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (c) => c.toUpperCase())
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * Speech caption for the island / conversation primary quote.
 * Strict rules kill stale STT during confirm and idle Ready.
 */
export function pickCaption(status: StatusPicture): Caption | null {
  const detail = status.detail?.trim();
  if (detail) {
    return { kind: "error", text: detail };
  }

  const heard = status.heard?.trim() || "";
  const said = status.said?.trim() || "";
  const phase = status.phase;
  const confirm = isConfirmContext(status);

  // Confirm path: Boris prompt only — never prior-turn You
  if (confirm) {
    if (said) return { kind: "said", text: said };
    return null;
  }

  // Listening for a new utterance: hide residual last-turn text
  if (phase === "Hearing") {
    return null;
  }

  // STT in progress: show new transcript only (pipeline clears heard on new capture)
  if (phase === "Reading") {
    if (heard) return { kind: "heard", text: heard };
    return null;
  }

  // Working: prefer draft reply once it exists (pre-Talking synth)
  if (phase === "Thinking") {
    if (said) return { kind: "said", text: said };
    if (heard) return { kind: "heard", text: heard };
    return null;
  }

  if (phase === "Talking") {
    if (said) return { kind: "said", text: said };
    return null;
  }

  if (phase === "AwaitingReply") {
    if (said) return { kind: "said", text: said };
    return null;
  }

  // Ready: last Boris line only (soft history for Conversation/island)
  if (phase === "Armed" || phase === "Quiet") {
    if (said) return { kind: "said", text: said };
    return null;
  }

  return null;
}

/**
 * One-line “what is happening now” secondary text.
 * Activity is humanized; may still echo the primary — use {@link pickOverlayPresence}.
 */
export function pickSecondary(
  status: StatusPicture,
  phaseHint: string,
): string {
  if (status.detail?.trim()) return "Something went wrong";

  const activityLine = humanizeActivity(status.activity);
  const phase = status.phase;
  const confirm = isConfirmContext(status);

  if (confirm) {
    // Prefer plain CTA over “Approve Bash?” when we already show the prompt
    if (activityLine && !activityLine.startsWith("Approve")) {
      return activityLine;
    }
    return "Waiting for your yes";
  }

  // Tool / subagent / multi-step progress always wins during Thinking
  if (phase === "Thinking") {
    // Plain "Thinking…" / empty → no secondary (primary already says Thinking)
    if (!activityLine || isEchoOfThinking(activityLine)) return "";
    return activityLine;
  }

  switch (phase) {
    case "Off":
      return "Engine is off";
    case "Quiet":
    case "Armed":
      return "Say the wake word";
    case "AwaitingReply":
      return "Answer freely — no wake word";
    case "AwaitingConfirm":
      return "Waiting for your yes";
    case "Hearing":
      return "Go ahead";
    case "Reading":
      // Primary is "Transcribing" — don't double it
      return "";
    case "Talking":
      return ""; // caption is enough
    default:
      return phaseHint;
  }
}

function isEchoOfThinking(line: string): boolean {
  const n = normalizeLabel(line);
  return n === "thinking" || n === "working" || n === "on it";
}

/** Strip punctuation / case for title↔subtitle equality checks. */
function normalizeLabel(s: string): string {
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

export type OverlayPresence = {
  /** 1–2 word phase title */
  primary: string;
  /** Complementary detail — never a synonym of primary */
  secondary: string;
};

/**
 * Single source of truth for overlay title + subtitle.
 * Guarantees no Working/Working, Thinking/Thinking, etc.
 */
export function pickOverlayPresence(
  status: StatusPicture,
  toneLabel: string,
  phaseHint: string,
): OverlayPresence {
  const activity = status.activity ?? "";
  const phase = status.phase;
  let primary = toneLabel;

  // Refine primary for work phases (tools vs pure LLM vs research)
  if (status.engine === "On" || status.engine === "Starting") {
    if (phase === "Thinking") {
      if (/spawn_subagent/i.test(activity)) {
        primary = "Researching";
      } else if (/web_search|web_fetch/i.test(activity)) {
        primary = "Searching";
      } else if (isToolActivity(activity)) {
        primary = "Working";
      } else if (/thinking\s*[·.]\s*after/i.test(activity)) {
        primary = "Thinking";
      } else if (/tools?\s+next|calling tools/i.test(activity)) {
        primary = "Working";
      } else {
        primary = "Thinking";
      }
    } else if (phase === "Reading") {
      primary = "Transcribing";
    }
  }

  let secondary = pickSecondary(status, phaseHint);
  secondary = dedupeSecondary(primary, secondary);

  // Subagent: if secondary still says "Researching…", drop it
  if (primary === "Researching" && normalizeLabel(secondary) === "researching") {
    secondary = "";
  }

  return { primary, secondary };
}

/** Whether the thin progress hairline should show. */
export function showProgressBar(status: StatusPicture): boolean {
  if (status.phase !== "Thinking") return false;
  const a = status.activity?.trim() ?? "";
  if (!a) return false;
  // Multi-tool counts and tool chips keep the bar alive so rapid starts don't look stuck
  return true;
}

/** Live reasoning tail for the island. Hidden once a spoken reply exists. */
export function overlayThinkingText(status: StatusPicture): string | null {
  if (status.phase !== "Thinking") return null;
  if (isConfirmContext(status)) return null;
  if (status.said?.trim()) return null;
  const text = status.thinking?.trim() ?? "";
  return text || null;
}

/** Host HWND + CSS stage must use the same mode (`overlay_win::layout_for`). */
export type OverlayStageMode = "presence" | "thought" | "card";

export function overlayStageMode(status: StatusPicture): OverlayStageMode {
  if (shouldShowOverlayCard(status)) return "card";
  if (status.phase === "Thinking") return "thought";
  return "presence";
}

// ── Main conversation panel lines ──────────────────────────────────────────

export type ConversationLine =
  | { kind: "you"; text: string; muted?: boolean }
  | { kind: "boris"; text: string }
  | { kind: "status"; text: string }
  | { kind: "thought"; text: string }
  | { kind: "confirm"; activity: string | null; prompt: string }
  | { kind: "error"; text: string }
  | { kind: "placeholder"; text: string };

/**
 * Phase-aware conversation rows for the main window.
 * Aligns with overlay caption rules so both surfaces feel in sync.
 */
export function conversationLines(status: StatusPicture): ConversationLine[] {
  const lines: ConversationLine[] = [];
  const heard = status.heard?.trim() || "";
  const said = status.said?.trim() || "";
  const activityLine = humanizeActivity(status.activity);
  const phase = status.phase;
  const confirm = isConfirmContext(status);

  if (status.engine === "Off" || phase === "Off") {
    lines.push({
      kind: "placeholder",
      text: "Nothing yet. Press Start, then say the wake word.",
    });
    return lines;
  }

  if (status.detail?.trim()) {
    lines.push({ kind: "error", text: status.detail.trim() });
  }

  if (confirm) {
    lines.push({
      kind: "confirm",
      activity: activityLine,
      prompt: said || "Needs your approval",
    });
    // Only show short confirm answers as You (yes/no), not the original command
    if (
      (phase === "Hearing" || phase === "Reading" || phase === "Thinking") &&
      heard &&
      isShortConfirmAnswer(heard)
    ) {
      lines.push({ kind: "you", text: heard });
    } else if (phase === "Hearing") {
      lines.push({ kind: "placeholder", text: "Waiting for your yes…" });
    }
    return lines;
  }

  if (phase === "Hearing") {
    lines.push({ kind: "placeholder", text: "Listening…" });
    return lines;
  }

  if (phase === "Reading") {
    if (heard) lines.push({ kind: "you", text: heard });
    else lines.push({ kind: "placeholder", text: "Transcribing…" });
    return lines;
  }

  if (phase === "Thinking") {
    if (heard) lines.push({ kind: "you", text: heard });
    const thought = status.thinking?.trim() ?? "";
    if (thought) {
      lines.push({ kind: "thought", text: thought });
    } else if (activityLine) {
      lines.push({ kind: "status", text: activityLine });
    } else {
      lines.push({ kind: "status", text: "Working…" });
    }
    if (said) lines.push({ kind: "boris", text: said });
    return lines;
  }

  if (phase === "Talking") {
    if (heard) lines.push({ kind: "you", text: heard });
    if (said) lines.push({ kind: "boris", text: said });
    return lines;
  }

  if (phase === "AwaitingReply") {
    if (said) lines.push({ kind: "boris", text: said });
    lines.push({
      kind: "placeholder",
      text: "Your turn — answer freely, no wake word.",
    });
    return lines;
  }

  // Armed / Quiet — soft last-turn history
  if (heard) lines.push({ kind: "you", text: heard, muted: true });
  if (said) lines.push({ kind: "boris", text: said });
  if (!heard && !said) {
    lines.push({
      kind: "placeholder",
      text: "Nothing yet. Say the wake word to talk.",
    });
  }
  return lines;
}

function isShortConfirmAnswer(text: string): boolean {
  const t = text.trim().toLowerCase();
  if (t.length > 48) return false;
  return /^(y|n|yes|no|yeah|yep|yup|nah|nope|sure|ok|okay|cancel|stop|go\s*ahead|do\s*it|please|affirmative|negative)\b/.test(
    t,
  );
}

/**
 * Overlay glance: show the card presented this turn, plus the Ready linger.
 * Pipeline clears `artifact` when the next utterance starts, so a later
 * Thinking/Talking snapshot only has a card if this turn presented one.
 */
export function shouldShowOverlayCard(status: StatusPicture): boolean {
  if (!status.artifact) return false;
  if (status.phase === "Off" || status.phase === "Hearing" || status.phase === "Reading") {
    return false;
  }
  return true;
}

/** Phases that should keep the island expanded (not orb-only). */
export function shouldStayExpanded(status: StatusPicture): boolean {
  if (status.engine === "Fault" || status.engine === "Starting") return true;
  if (status.detail?.trim()) return true;
  if (isConfirmContext(status)) return true;
  const p = status.phase;
  return (
    p === "Hearing" ||
    p === "Reading" ||
    p === "Thinking" ||
    p === "Talking" ||
    p === "AwaitingReply" ||
    p === "AwaitingConfirm"
  );
}

export function canCollapseToOrb(status: StatusPicture): boolean {
  if (status.engine !== "On") return false;
  if (shouldStayExpanded(status)) return false;
  if (status.phase !== "Armed" && status.phase !== "Quiet") return false;
  // Ignore sticky post-turn tool counts on Ready so the island can idle
  const a = status.activity?.trim() ?? "";
  if (/^\d+\s+tools?$/i.test(a)) return true;
  const sticky = humanizeActivity(status.activity);
  if (sticky) return false;
  return true;
}
