import { describe, expect, it } from "vitest";
import {
  artifactKindOf,
  clipArtifactBody,
  GLANCE_CODE_LINES,
  GLANCE_MARKDOWN_LINES,
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

  it("clips a dense README to the line budget so the pill cannot grow", () => {
    const body = [
      "# Hey, I'm Uttam",
      "India · Student",
      "## About Me",
      "A long paragraph about software development that would stretch the island.",
      "## Links",
      "- GitHub: https://github.com/BlocksDevPro",
      "- GitHub (alt): https://github.com/Blockswave%20Pro",
      "## Tech Stack",
      "- Languages: JavaScript, TypeScript, Rust",
      "- Frontend: React, Next.js",
      "- Backend: Node.js, Express",
      "- Databases: MongoDB, PostgreSQL",
      "- Blockchain: Solana",
    ].join("\n");
    const clip = clipArtifactBody("markdown", body);
    expect(clip.clipped).toBe(true);
    expect(clip.text.split("\n").length).toBeLessThanOrEqual(GLANCE_MARKDOWN_LINES);
    expect(clip.text).toContain("# Hey, I'm Uttam");
    expect(clip.text).not.toContain("Blockchain: Solana");
  });
});
