import { describe, expect, it } from "vitest";
import { OFF_STATUS, type StatusPicture } from "./types";
import { latestStatus } from "./useStatus";

function snapshot(seq: number, patch: Partial<StatusPicture>): StatusPicture {
  return { ...OFF_STATUS, seq, ...patch };
}

describe("latestStatus", () => {
  it("ignores delayed events and keeps full phase, device, input, and artifact snapshots", () => {
    const current = snapshot(12, {
      engine: "On",
      phase: "AwaitingInput",
      mic: { label: "USB mic", ok: false },
      input: {
        id: "prompt",
        kind: "secret",
        label: "API key",
        spoken: "Paste it",
        multiline: false,
        max_chars: 100,
      },
      artifact: { id: "card", title: "Notes", kind: "markdown", path: "notes.md" },
    });
    const oldReasoning = snapshot(11, { engine: "On", phase: "Thinking" });
    expect(latestStatus(current, oldReasoning)).toBe(current);

    const stopped = snapshot(13, { engine: "Off", phase: "Off" });
    expect(latestStatus(current, stopped)).toBe(stopped);
    const restarted = snapshot(14, { engine: "Starting", phase: "Off" });
    expect(latestStatus(stopped, restarted)).toBe(restarted);
    const fault = snapshot(15, { engine: "Fault", detail: "model failed" });
    expect(latestStatus(restarted, fault)).toBe(fault);
  });
});
