import { describe, expect, it } from "vitest";
import { getStatusFixture } from "@/preview/statusFixtures";
import { ORB_PRESENTATIONS, orbStateFromStatus } from "./orbState";

describe("Boris orb semantics", () => {
  it("never implies microphone listening for typed input, confirmation, or faults", () => {
    for (const name of ["typed-input", "confirm", "fault", "off"]) {
      const state = orbStateFromStatus(getStatusFixture(name)!);
      expect(ORB_PRESENTATIONS[state].animation).not.toBe("listening");
      expect(ORB_PRESENTATIONS[state].paused).toBe(true);
    }
    expect(orbStateFromStatus(getStatusFixture("awaiting-reply")!)).toBe("awaiting-reply");
  });
  it("keeps planned tools as reasoning and recognizes actual execution", () => {
    const base = getStatusFixture("thinking")!;
    expect(orbStateFromStatus({ ...base, activity: "thinking · 2 tools next" })).toBe("thinking");
    expect(orbStateFromStatus({ ...base, activity: "tool · web_search" })).toBe("searching");
    expect(orbStateFromStatus({ ...base, activity: "tool · file_write" })).toBe("shaping");
    expect(orbStateFromStatus({ ...base, activity: "tool · bash" })).toBe("working");
    expect(orbStateFromStatus({ ...base, activity: "coordinating parallel results" })).toBe("weaving");
  });
  it("foregrounds an interruption even while the underlying agent is thinking", () => {
    expect(orbStateFromStatus(getStatusFixture("barge-listening")!)).toBe("hearing");
    expect(orbStateFromStatus(getStatusFixture("barge-transcribing")!)).toBe("reading");
  });
});
