/** Pipeline activity strings and interruption markers. */
import type { StatusPicture } from "@/bridge";
import type { BargeInStage, OverlayPresence } from "./types";
/**
 * Foreground barge-in state encoded by the pipeline in `activity`.
 * Keep this centralized: the snapshot can intentionally retain fields from
 * the interrupted turn while the replacement is being captured.
 */
export function bargeInStage(
  activity: string | null | undefined,
): BargeInStage | null {
  const match = activity
    ?.trim()
    .match(/^barge-in\s*[·.]\s*(listening|transcribing|switching|stopping)\b/i);
  switch (match?.[1]?.toLowerCase()) {
    case "listening":
      return "listening";
    case "transcribing":
      return "transcribing";
    case "switching":
      return "switching";
    case "stopping":
      return "stopping";
    default:
      return null;
  }
}

export function bargeInPresence(
  activity: string | null | undefined,
): OverlayPresence | null {
  switch (bargeInStage(activity)) {
    case "listening":
      return {
        primary: "Listening",
        secondary: "Interrupting current task",
      };
    case "transcribing":
      return {
        primary: "Transcribing",
        secondary: "Your change",
      };
    case "switching":
      return { primary: "Switching tasks", secondary: "" };
    case "stopping":
      return {
        primary: "Stopping",
        secondary: "Cancelling current task",
      };
    default:
      return null;
  }
}

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
  // Post-turn sticky ("Read 4 files" / "3 tools")
  if (/^\d+\s+tools?$/.test(a)) return true;
  if (
    /^(read|reading|searched|searching|ran|running|listed|listing|fetched|fetching|wrote|writing|edited|editing|found|finding)\b/.test(
      a,
    )
  ) {
    return true;
  }
  // Planning a tool call is still reasoning. The indicator switches to
  // Working only when execution actually starts.
  if (a.startsWith("thinking")) return false;
  return (
    a.startsWith("tool") ||
    a.startsWith("done") ||
    a.startsWith("fail") ||
    /tools?\s+next|calling tools/i.test(a)
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

  const barge = bargeInPresence(raw);
  if (barge) return barge.secondary || barge.primary;

  // Already a Grok-style summary ("Read 4 files, Searched 1 pattern")
  if (looksLikeVerbPhrase(raw) && !/^(tool|done|fail|thinking|confirm)\s*[·.]/i.test(raw)) {
    return clip(raw, 64);
  }

  const multi = raw.match(/^(\d+)\s+tools?$/i);
  if (multi) {
    const n = multi[1];
    return n === "1" ? "Ran 1 tool this turn" : `Ran ${n} tools this turn`;
  }

  const lower = raw.toLowerCase();

  if (lower === "thinking…" || lower === "thinking...") {
    return "Thinking…";
  }

  if (lower.startsWith("thinking")) {
    const rest = raw.replace(/^thinking\s*[·.]\s*/i, "").trim();
    if (!rest) return "Thinking…";
    if (/^thought for\b/i.test(rest) || /^\d+(\.\d+)?s$/.test(rest)) {
      return /^thought for\b/i.test(rest) ? rest : `Thought for ${rest}`;
    }
    if (/^(?:step|round)\s*\d+/i.test(rest)) {
      return "Choosing next action…";
    }
    const nextTools = rest.match(/^(\d+)\s+tools?\s+next$/i);
    if (nextTools) {
      const n = nextTools[1];
      return n === "1" ? "About to run 1 tool…" : `About to run ${n} tools…`;
    }
    if (/^calling tools$/i.test(rest)) return "About to run tools…";
    if (/^next action$/i.test(rest)) return "Choosing next action…";
    if (/^after\s+.+/i.test(rest)) {
      return "Checking tool results…";
    }
    return clip(capitalize(rest), 56);
  }

  if (lower.startsWith("confirm")) {
    const rest = raw.replace(/^confirm\s*[·.]\s*/i, "").trim();
    if (!rest) return "Waiting for your yes";
    if (/yes|no|sure|cancel|listen/i.test(rest)) return "Your turn — say yes or no";
    const stripped = rest.replace(
      /^(Running|Opening|Writing|Editing|Reading|Fetching)\s+/i,
      "",
    );
    return `Approve ${clip(stripped, 48)}?`;
  }

  if (lower.startsWith("input")) {
    const rest = raw.replace(/^input\s*[·.]\s*/i, "").trim();
    return rest ? clip(rest, 48) : "Type or paste";
  }

  const fail = raw.match(/^fail\s*[·.]\s*(.+)$/i);
  if (fail) {
    const rest = fail[1]!.trim();
    if (looksLikeVerbPhrase(rest)) return `${clip(rest, 52)} failed`;
    return `${friendlyTool(rest)} failed`;
  }

  const done = raw.match(/^done\s*[·.]\s*(.+)$/i);
  if (done) {
    const rest = done[1]!.trim();
    if (looksLikeVerbPhrase(rest)) return clip(rest, 64);
    return `Finished ${friendlyTool(rest)}`;
  }

  const tool = raw.match(/^tool\s*[·.]\s*(.+)$/i);
  if (tool) {
    const rest = tool[1]!.trim();
    if (/^via\s+/i.test(rest) || /^research:/i.test(rest) || /^step\s+\d+/i.test(rest)) {
      return clip(rest, 56);
    }
    if (looksLikeVerbPhrase(rest)) return clip(rest, 64);
    const parts = rest.split(/\s*[·.]\s*/);
    const name = friendlyTool(parts[0] ?? rest);
    const msg = parts.slice(1).join(" · ").trim();
    if (msg) return clip(`${name}: ${msg}`, 64);
    return name === "Subagent" ? "Researching…" : `Running ${name}…`;
  }

  return clip(raw.replace(/\s*·\s*/g, " · "), 64);
}

function looksLikeVerbPhrase(s: string): boolean {
  return /^(Read|Reading|Searched|Searching|Ran|Running|Listed|Listing|Fetched|Fetching|Wrote|Writing|Edited|Editing|Found|Finding|Showed|Showing|Loaded|Loading|Checked|Checking|Updated|Updating|Copied|Copying|Recalled|Recalling|Saved|Saving|Opened|Opening|Called|Calling)\b/.test(
    s.trim(),
  );
}

function clip(s: string, max: number): string {
  const t = s.trim();
  if (t.length <= max) return t;
  return `${t.slice(0, max - 1)}…`;
}

function capitalize(s: string): string {
  if (!s) return s;
  return s.charAt(0).toUpperCase() + s.slice(1);
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
