import { describe, expect, it } from "vitest";
import { PRESENCE_PLAY, playForState, type PresenceState } from "./gridPatterns";
import { presenceStateFromStatus } from "./presenceState";
import { getStatusFixture } from "@/preview/statusFixtures";

const ALL_STATES: PresenceState[] = [
  "off",
  "starting",
  "ready",
  "awaiting-reply",
  "confirm",
  "hearing",
  "reading",
  "thinking",
  "working",
  "talking",
  "fault",
];

describe("LED presence playbook", () => {
  it("covers every semantic state with at least one 3×3 frame", () => {
    for (const state of ALL_STATES) {
      const play = playForState(state);
      expect(play.frames.length).toBeGreaterThan(0);
      expect(play.frames[0]).toHaveLength(3);
      expect(play.frames[0][0]).toHaveLength(3);
    }
  });

  it("keeps thinking and tool-working on different shapes", () => {
    expect(PRESENCE_PLAY.thinking.frames).not.toEqual(
      PRESENCE_PLAY.working.frames,
    );
    expect(PRESENCE_PLAY.hearing.mode).toBe("sequence");
    expect(PRESENCE_PLAY.fault.mode).toBe("static");
    expect(PRESENCE_PLAY.ready.mode).toBe("pulse");
  });

  it("maps fixtures onto the playbook", () => {
    expect(presenceStateFromStatus(getStatusFixture("thinking")!)).toBe(
      "thinking",
    );
    expect(presenceStateFromStatus(getStatusFixture("thinking-tool")!)).toBe(
      "working",
    );
    expect(presenceStateFromStatus(getStatusFixture("hearing")!)).toBe(
      "hearing",
    );
    expect(presenceStateFromStatus(getStatusFixture("confirm")!)).toBe(
      "confirm",
    );
    expect(presenceStateFromStatus(getStatusFixture("fault")!)).toBe("fault");
    expect(presenceStateFromStatus(getStatusFixture("ready")!)).toBe("ready");
  });
});
