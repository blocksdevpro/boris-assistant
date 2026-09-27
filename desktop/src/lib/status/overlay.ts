/** Status rules that decide the floating overlay's content and size. */
import type { StatusPicture } from "@/bridge";
import type { OverlayPresence, OverlayStageMode } from "./types";
import { bargeInPresence, bargeInStage, humanizeActivity, isConfirmContext, isToolActivity } from "./activity";
import { dedupeSecondary, normalizeLabel, stripEchoedVerb } from "./labels";
import { pickSecondary } from "./conversation";
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

  // Barge-in is a foreground interaction, even when the phase is still
  // Thinking for the interrupted work. Never let its old tool label win.
  const barge =
    status.engine === "On" ? bargeInPresence(status.activity) : null;
  if (barge) return barge;

  // Typed input wins even if the snapshot still says Thinking for a frame.
  if (status.input) {
    return {
      primary: "Your turn",
      secondary: status.input.label.trim() || "Type or paste",
    };
  }

  // Refine primary for work phases (tools vs pure LLM vs research)
  if (status.engine === "On" || status.engine === "Starting") {
    if (phase === "Thinking") {
      if (/^fail\s*[·.]/i.test(activity)) {
        primary = "Tool failed";
      } else if (/spawn_subagent|subagent/i.test(activity)) {
        primary = "Researching";
      } else if (
        /web_search|web_fetch/i.test(activity) ||
        /\b(Searching|Searched|Fetching|Fetched)\b/.test(activity)
      ) {
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
  secondary = stripEchoedVerb(primary, secondary);
  secondary = dedupeSecondary(primary, secondary);

  // Subagent: if secondary still says "Researching…", drop it
  if (primary === "Researching" && normalizeLabel(secondary) === "researching") {
    secondary = "";
  }

  return { primary, secondary };
}

/** Live thought or tool note for the island. Hidden once a spoken reply exists. */
export function overlayThinkingText(status: StatusPicture): string | null {
  if (bargeInStage(status.activity)) return null;
  if (status.phase !== "Thinking") return null;
  if (isConfirmContext(status)) return null;
  if (status.said?.trim()) return null;
  const text = status.thinking?.trim() ?? "";
  return text || null;
}

/** Island chrome mode. The HWND stays on the card budget; this only
 *  changes max size / clipping of the painted pill. */
export function overlayStageMode(status: StatusPicture): OverlayStageMode {
  if (shouldShowOverlayCard(status) || overlayInputUsesCard(status)) {
    return "card";
  }
  if (status.input || status.phase === "Thinking") return "thought";
  return "presence";
}

/** Blob / multiline paste uses the glance card. A short exact/secret field
 *  stays thought-sized so the island does not jump to 264px empty. */
export function overlayInputUsesCard(status: StatusPicture): boolean {
  const input = status.input;
  if (!input) return false;
  return input.multiline || input.kind.toLowerCase() === "blob";
}
/**
 * Overlay glance: show the card presented this turn, plus the Ready linger.
 * Pipeline clears `artifact` when the next utterance starts, so a later
 * Thinking/Talking snapshot only has a card if this turn presented one.
 */
export function shouldShowOverlayCard(status: StatusPicture): boolean {
  // Keep this predicate byte-for-byte equivalent in meaning to the native
  // host's `wants_card_layout`; otherwise React can render into the wrong HWND.
  if (!status.artifact || status.engine !== "On") return false;
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
    p === "AwaitingConfirm" ||
    p === "AwaitingInput"
  );
}

export function canCollapseToOrb(status: StatusPicture): boolean {
  if (status.engine !== "On") return false;
  if (shouldStayExpanded(status)) return false;
  if (status.phase !== "Armed" && status.phase !== "Quiet") return false;
  // Ignore sticky post-turn tool counts on Ready so the island can idle
  const a = status.activity?.trim() ?? "";
  if (/^\d+\s+tools?$/i.test(a)) return true;
  if (
    /^(Read|Searched|Ran|Listed|Fetched|Wrote|Edited|Found|Showed)\b/.test(a.trim())
  ) {
    return true;
  }
  const sticky = humanizeActivity(status.activity);
  if (sticky) return false;
  return true;
}
