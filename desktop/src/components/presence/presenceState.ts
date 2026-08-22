import type { StatusPicture } from "@/bridge";
import type { PhaseTone } from "@/lib/phaseVisual";
import { isToolActivity } from "@/lib/statusPresentation";
import type { PresenceState } from "./gridPatterns";

export type { PresenceState };

type LegacyMotion = PhaseTone["motion"];

export function presenceStateFromStatus(
  status: Pick<StatusPicture, "engine" | "phase" | "activity">,
): PresenceState {
  if (status.engine === "Fault") return "fault";
  if (status.engine === "Starting") return "starting";
  if (status.engine === "Off") return "off";

  switch (status.phase) {
    case "AwaitingReply":
      return "awaiting-reply";
    case "AwaitingConfirm":
      return "confirm";
    case "Hearing":
      return "hearing";
    case "Reading":
      return "reading";
    case "Thinking":
      return isToolActivity(status.activity) ? "working" : "thinking";
    case "Talking":
      return "talking";
    case "Quiet":
    case "Armed":
      return "ready";
    case "Off":
    default:
      return "off";
  }
}

export function legacyStateFromMotion(
  motion: LegacyMotion,
  hasActivity: boolean,
): PresenceState {
  switch (motion) {
    case "breathe":
      return "ready";
    case "listen":
      return "hearing";
    case "think":
      return hasActivity ? "working" : "thinking";
    case "speak":
      return "talking";
    case "none":
    default:
      return "off";
  }
}
