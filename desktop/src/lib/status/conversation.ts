/** Status snapshots rendered as speech captions and main-window chat rows. */
import type { StatusPicture } from "@/bridge";
import type { BargeInStage, Caption, ConversationLine } from "./types";
import {
  bargeInPresence,
  bargeInStage,
  humanizeActivity,
  isConfirmContext,
} from "./activity";
import { isEchoOfThinking } from "./labels";
/**
 * Speech caption for the island / conversation primary quote.
 * Strict rules kill stale STT during confirm and idle Ready.
 */
export function pickCaption(status: StatusPicture): Caption | null {
  const detail = status.detail?.trim();
  if (detail && status.engine !== "Starting") {
    return { kind: "error", text: detail };
  }

  // During a barge-in these fields may still describe the interrupted turn.
  // The activity marker is authoritative until the replacement turn starts.
  if (bargeInStage(status.activity)) return null;

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
  // Engine lifecycle overrides the phase snapshot. During startup the last
  // known phase can still be Off; during a fault it can still look Ready.
  if (status.engine === "Starting") return phaseHint;
  if (status.engine === "Fault") return "";
  if (status.engine === "Off") return "Engine is off";

  // The error caption carries the actionable detail. Repeating a generic
  // subtitle beside "Error" weakens the hierarchy on the small island.
  if (status.detail?.trim()) return "";

  const barge = bargeInPresence(status.activity);
  if (barge) return barge.secondary;

  const activityLine = humanizeActivity(status.activity);
  const phase = status.phase;
  const confirm = isConfirmContext(status);

  if (confirm) {
    // Prefer plain CTA over “Approve Bash?” when we already show the prompt
    if (activityLine && !activityLine.startsWith("Approve")) {
      return activityLine;
    }
    return "Say yes or no";
  }

  // Tool / subagent / multi-step progress always wins during Thinking
  if (phase === "Thinking") {
    // Plain "Thinking…" / empty → no secondary (primary already says Thinking)
    if (!activityLine || isEchoOfThinking(activityLine)) return "";
    return activityLine;
  }

  switch (phase) {
    case "Off":
      return phaseHint;
    case "Quiet":
    case "Armed":
      return "Say the wake word";
    case "AwaitingReply":
      return "No wake word needed";
    case "AwaitingConfirm":
      return "Say yes or no";
    case "AwaitingInput":
      return "Type or paste";
    case "Hearing":
      return "Speak naturally";
    case "Reading":
      // Primary is "Transcribing" — don't double it
      return "";
    case "Talking":
      return ""; // caption is enough
    default:
      return phaseHint;
  }
}

// ── Main conversation panel lines ──────────────────────────────────────────

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

  const barge = bargeInStage(status.activity);
  if (barge) {
    const text: Record<BargeInStage, string> = {
      listening: "Listening to your change…",
      transcribing: "Transcribing your change…",
      switching: "Switching tasks…",
      stopping: "Stopping current task…",
    };
    lines.push({
      kind: barge === "switching" || barge === "stopping" ? "status" : "placeholder",
      text: text[barge],
    });
    return lines;
  }

  if (status.input) {
    lines.push({
      kind: "input",
      label: status.input.label,
      prompt: said || status.input.spoken || "Type or paste on screen.",
    });
    return lines;
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
    if (activityLine && !isEchoOfThinking(activityLine)) {
      lines.push({ kind: "status", text: activityLine });
    }
    if (thought) {
      lines.push({ kind: "thought", text: thought });
    } else if (!activityLine || isEchoOfThinking(activityLine)) {
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
