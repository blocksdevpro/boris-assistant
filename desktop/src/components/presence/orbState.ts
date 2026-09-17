import type { OrbState } from "thinking-orbs";
import type { StatusPicture } from "@/bridge";
import { bargeInStage, isToolActivity } from "@/lib/statusPresentation";

export type BorisOrbState =
  | "off" | "starting" | "ready" | "hearing" | "reading" | "thinking"
  | "searching" | "working" | "weaving" | "shaping" | "talking"
  | "awaiting-reply" | "awaiting-input" | "confirm" | "fault";

type OrbPresentation = { animation: OrbState; label: string; speed: number; paused?: boolean };

export const ORB_PRESENTATIONS: Record<BorisOrbState, OrbPresentation> = {
  off: { animation: "breathing", label: "Boris is off", speed: 0.4, paused: true },
  starting: { animation: "connecting", label: "Boris is starting", speed: 0.7 },
  ready: { animation: "breathing", label: "Boris is ready", speed: 0.45 },
  hearing: { animation: "listening", label: "Boris is listening", speed: 0.75 },
  reading: { animation: "composing", label: "Boris is transcribing", speed: 0.7 },
  thinking: { animation: "solving", label: "Boris is thinking", speed: 0.65 },
  searching: { animation: "searching", label: "Boris is searching", speed: 0.7 },
  working: { animation: "working", label: "Boris is using a tool", speed: 0.65 },
  weaving: { animation: "weaving", label: "Boris is combining results", speed: 0.6 },
  shaping: { animation: "shaping", label: "Boris is creating an artifact", speed: 0.6 },
  talking: { animation: "composing", label: "Boris is speaking", speed: 0.45 },
  "awaiting-reply": { animation: "listening", label: "Boris is waiting for your spoken reply", speed: 0.4 },
  "awaiting-input": { animation: "breathing", label: "Boris is waiting for typed input", speed: 0.4, paused: true },
  confirm: { animation: "breathing", label: "Boris needs your confirmation", speed: 0.4, paused: true },
  fault: { animation: "breathing", label: "Boris needs attention", speed: 0.4, paused: true },
};

/** Tool details become a small set of stable visual categories, never audio amplitude. */
export function orbStateFromStatus(
  status: Pick<StatusPicture, "engine" | "phase" | "activity">,
): BorisOrbState {
  if (status.engine === "Fault") return "fault";
  if (status.engine === "Starting") return "starting";
  if (status.engine === "Off") return "off";
  const interruption = bargeInStage(status.activity);
  if (interruption === "listening") return "hearing";
  if (interruption === "transcribing") return "reading";
  if (interruption === "stopping" || interruption === "switching") return "starting";
  switch (status.phase) {
    case "AwaitingConfirm": return "confirm";
    case "AwaitingInput": return "awaiting-input";
    case "AwaitingReply": return "awaiting-reply";
    case "Hearing": return "hearing";
    case "Reading": return "reading";
    case "Talking": return "talking";
    case "Quiet":
    case "Armed": return "ready";
    case "Thinking": {
      const activity = status.activity?.toLowerCase() ?? "";
      if (activity.startsWith("thinking")) return "thinking";
      if (/search|web[_ .:-]|fetch|browse/.test(activity)) return "searching";
      if (/synthesi|coordinat|combine|parallel|multi.source/.test(activity)) return "weaving";
      if (/artifact|file_write|write_file|create_file|document|spreadsheet|presentation/.test(activity)) return "shaping";
      if (isToolActivity(activity)) return "working";
      return "thinking";
    }
    default: return "off";
  }
}

export function isWorkingOrbState(state: BorisOrbState): boolean {
  return ["thinking", "searching", "working", "weaving", "shaping"].includes(state);
}
