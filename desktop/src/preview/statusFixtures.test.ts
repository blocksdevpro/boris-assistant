import { describe, expect, it } from "vitest";
import { toneFor } from "@/lib/phaseVisual";
import {
  bargeInPresence,
  bargeInStage,
  conversationLines,
  isConfirmContext,
  isToolActivity,
  overlayInputUsesCard,
  overlayStageMode,
  overlayThinkingText,
  pickCaption,
  pickOverlayPresence,
  shouldShowOverlayCard,
} from "@/lib/statusPresentation";
import { STATUS_FIXTURES, getStatusFixture } from "./statusFixtures";
import { presenceStateFromStatus } from "@/windows/overlay/PresenceIndicator";

describe("overlay preview fixtures", () => {
  it("keeps fixture names unique and required states present", () => {
    const names = STATUS_FIXTURES.map(({ name }) => name);
    expect(new Set(names).size).toBe(names.length);
    expect(names).toEqual([
      "off",
      "starting",
      "ready",
      "awaiting-reply",
      "hearing",
      "reading",
      "thinking",
      "thinking-long",
      "thinking-tool",
      "tool-failure",
      "barge-listening",
      "barge-transcribing",
      "barge-switching",
      "barge-stopping",
      "typed-input",
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
    expect(isToolActivity("thinking · 2 tools next")).toBe(false);
    expect(isToolActivity("tool · web_search")).toBe(true);
    expect(isConfirmContext(getStatusFixture("confirm")!)).toBe(true);
    expect(pickCaption(getStatusFixture("long-caption")!)?.text.length).toBeGreaterThan(
      80,
    );
    const deviceFaults = getStatusFixture("device-faults")!;
    expect(deviceFaults.mic.ok).toBe(false);
    expect(deviceFaults.speaker.ok).toBe(false);
    expect(getStatusFixture("artifact-card")!.artifact?.id).toBe("a1f3c9");
  });

  it("uses concise, action-oriented copy at transition states", () => {
    const presenceFor = (name: string) => {
      const status = getStatusFixture(name)!;
      const tone = toneFor(status.phase, status.engine);
      return pickOverlayPresence(status, tone.label, tone.hint);
    };

    expect(presenceFor("starting")).toEqual({
      primary: "Starting",
      secondary: "Getting ready",
    });
    expect(pickCaption(getStatusFixture("starting")!)).toBeNull();
    expect(presenceFor("hearing").secondary).toBe("Speak naturally");
    expect(presenceFor("awaiting-reply").secondary).toBe(
      "No wake word needed",
    );
    expect(presenceFor("confirm").secondary).toBe("Say yes or no");
    expect(presenceFor("typed-input")).toEqual({
      primary: "Your turn",
      secondary: "API key",
    });
    expect(presenceFor("fault").secondary).toBe("");
    expect(presenceFor("thinking")).toEqual({
      primary: "Thinking",
      secondary: "",
    });
    expect(presenceFor("thinking-tool")).toEqual({
      primary: "Searching",
      secondary: "weather in Bengaluru",
    });
    expect(presenceFor("tool-failure")).toEqual({
      primary: "Tool failed",
      secondary: "Search failed",
    });
  });

  it("streams reasoning on the thinking island, not the tool chip", () => {
    const thinking = getStatusFixture("thinking")!;
    expect(overlayStageMode(thinking)).toBe("thought");
    expect(overlayThinkingText(thinking)).toMatch(/spoken brief/i);
    expect(overlayThinkingText(getStatusFixture("thinking-tool")!)).toBeNull();
    expect(overlayStageMode(getStatusFixture("thinking-tool")!)).toBe("thought");
    expect(overlayStageMode(getStatusFixture("ready")!)).toBe("presence");
    expect(overlayStageMode(getStatusFixture("artifact-card")!)).toBe("card");
    const typed = getStatusFixture("typed-input")!;
    expect(overlayStageMode(typed)).toBe("thought");
    expect(overlayInputUsesCard(typed)).toBe(false);
    expect(
      overlayInputUsesCard({
        ...typed,
        input: typed.input
          ? { ...typed.input, kind: "blob", multiline: true }
          : null,
      }),
    ).toBe(true);
  });

  it.each([
    [
      "barge-listening",
      "listening",
      "Listening",
      "Interrupting current task",
      "placeholder",
      "Listening to your change…",
    ],
    [
      "barge-transcribing",
      "transcribing",
      "Transcribing",
      "Your change",
      "placeholder",
      "Transcribing your change…",
    ],
    [
      "barge-switching",
      "switching",
      "Switching tasks",
      "",
      "status",
      "Switching tasks…",
    ],
    [
      "barge-stopping",
      "stopping",
      "Stopping",
      "Cancelling current task",
      "status",
      "Stopping current task…",
    ],
  ] as const)(
    "makes %s an authoritative foreground state",
    (name, stage, primary, secondary, lineKind, lineText) => {
      const status = getStatusFixture(name)!;
      const tone = toneFor(status.phase, status.engine);

      expect(bargeInStage(status.activity)).toBe(stage);
      expect(bargeInPresence(status.activity)).toEqual({ primary, secondary });
      expect(pickOverlayPresence(status, tone.label, tone.hint)).toEqual({
        primary,
        secondary,
      });
      expect(pickCaption(status)).toBeNull();
      expect(overlayThinkingText(status)).toBeNull();
      expect(conversationLines(status)).toEqual([
        { kind: lineKind, text: lineText },
      ]);
    },
  );

  it("shows a card only for this turn and the Ready linger", () => {
    const card = getStatusFixture("artifact-card")!;
    expect(shouldShowOverlayCard(card)).toBe(true);
    expect(shouldShowOverlayCard({ ...card, phase: "Armed" })).toBe(true);
    expect(shouldShowOverlayCard({ ...card, phase: "Hearing" })).toBe(false);
    expect(shouldShowOverlayCard({ ...card, phase: "Reading" })).toBe(false);
    expect(shouldShowOverlayCard({ ...card, engine: "Fault" })).toBe(false);
    expect(shouldShowOverlayCard({ ...card, engine: "Off" })).toBe(false);
    expect(shouldShowOverlayCard(getStatusFixture("thinking")!)).toBe(false);
  });

  it("gives reasoning and tool execution distinct presence shapes", () => {
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
  });
});
