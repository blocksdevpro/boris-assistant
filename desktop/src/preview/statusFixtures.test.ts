import { describe, expect, it } from "vitest";
import { toneFor } from "@/lib/phaseVisual";
import {
  isConfirmContext,
  pickCaption,
  pickOverlayPresence,
  showProgressBar,
} from "@/lib/statusPresentation";
import { STATUS_FIXTURES, getStatusFixture } from "./statusFixtures";

describe("overlay preview fixtures", () => {
  it("keeps fixture names unique and required states present", () => {
    const names = STATUS_FIXTURES.map(({ name }) => name);
    expect(new Set(names).size).toBe(names.length);
    expect(names).toEqual([
      "off",
      "ready",
      "hearing",
      "reading",
      "thinking",
      "thinking-tool",
      "confirm",
      "talking",
      "fault",
      "long-caption",
      "device-faults",
      "artifact-card",
    ]);
  });

  it.each(STATUS_FIXTURES)("presents $name without duplicate labels", ({ status }) => {
    const tone = toneFor(status.phase, status.engine);
    const presence = pickOverlayPresence(status, tone.label, tone.hint);

    expect(presence.primary.trim()).not.toBe("");
    expect(presence.secondary.trim().toLowerCase()).not.toBe(
      presence.primary.trim().toLowerCase(),
    );
  });

  it("models tool, confirmation, long-copy, and device-fault edge cases", () => {
    expect(showProgressBar(getStatusFixture("thinking-tool")!)).toBe(true);
    expect(isConfirmContext(getStatusFixture("confirm")!)).toBe(true);
    expect(pickCaption(getStatusFixture("long-caption")!)?.text.length).toBeGreaterThan(
      80,
    );
    const deviceFaults = getStatusFixture("device-faults")!;
    expect(deviceFaults.mic.ok).toBe(false);
    expect(deviceFaults.speaker.ok).toBe(false);
    expect(getStatusFixture("artifact-card")!.artifact?.id).toBe("a1f3c9");
  });
});
