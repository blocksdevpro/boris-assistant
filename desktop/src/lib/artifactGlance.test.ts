import { describe, expect, it } from "vitest";
import {
  artifactKindOf,
  clipArtifactBody,
  GLANCE_CODE_LINES,
} from "./artifactGlance";

describe("artifactGlance", () => {
  it("treats unknown kinds as markdown", () => {
    expect(artifactKindOf("markdown")).toBe("markdown");
    expect(artifactKindOf("code")).toBe("code");
    expect(artifactKindOf("nope")).toBe("markdown");
  });

  it("keeps short code whole", () => {
    const clip = clipArtifactBody("code", "Get-ChildItem\nWrite-Host hi");
    expect(clip.clipped).toBe(false);
    expect(clip.hiddenLines).toBe(0);
    expect(clip.text).toContain("Get-ChildItem");
  });

  it("clips long code to a fixed line budget", () => {
    const body = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join("\n");
    const clip = clipArtifactBody("code", body);
    expect(clip.clipped).toBe(true);
    expect(clip.hiddenLines).toBe(30 - GLANCE_CODE_LINES);
    expect(clip.text.split("\n")).toHaveLength(GLANCE_CODE_LINES);
  });

  it("clips long markdown by blocks", () => {
    const blocks = Array.from({ length: 12 }, (_, i) => `## H${i + 1}\npara ${i + 1}`);
    const clip = clipArtifactBody("markdown", blocks.join("\n\n"));
    expect(clip.clipped).toBe(true);
    expect(clip.hiddenLines).toBeGreaterThan(0);
    expect(clip.text).toContain("## H1");
    expect(clip.text).not.toContain("## H12");
  });
});
