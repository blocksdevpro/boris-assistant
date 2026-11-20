import { describe, expect, it } from "vitest";
import { releaseNotesNodes } from "./releaseNotes";

const BODY = [
  "## Boris 1.3.0-beta.2",
  "",
  "Intro line.",
  "",
  "### What's new",
  "",
  "- A thing that",
  "  wraps.",
  "",
  "### Developer notes",
  "",
  "- `StatusPicture` gains fields.",
  "",
  "Full changes: [CHANGELOG.md](https://example.com/c).",
].join("\n");

describe("releaseNotesNodes", () => {
  it("drops the title and developer notes for regular users", () => {
    const nodes = releaseNotesNodes(BODY, { advanced: false });
    expect(nodes).toEqual([
      { type: "p", text: "Intro line." },
      { type: "h", level: 3, text: "What's new" },
      { type: "ul", items: ["A thing that wraps."] },
      {
        type: "p",
        text: "Full changes: [CHANGELOG.md](https://example.com/c).",
      },
    ]);
  });

  it("keeps developer notes in advanced mode", () => {
    const nodes = releaseNotesNodes(BODY, { advanced: true });
    expect(nodes.some((n) => n.type === "h" && n.text === "Developer notes")).toBe(true);
    expect(nodes[0]).toEqual({ type: "p", text: "Intro line." });
  });
});
