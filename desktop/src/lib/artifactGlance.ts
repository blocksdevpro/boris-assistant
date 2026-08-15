/** Overlay glance: clip a card so it fits the island (no scroll). */

export const GLANCE_CODE_LINES = 6;
export const GLANCE_MARKDOWN_LINES = 8;
export const GLANCE_MARKDOWN_BLOCKS = 4;

export type ArtifactKindName = "markdown" | "code";

export type GlanceClip = {
  text: string;
  hiddenLines: number;
  clipped: boolean;
};

export function artifactKindOf(kind: string | null | undefined): ArtifactKindName {
  return kind?.trim().toLowerCase() === "code" ? "code" : "markdown";
}

export function clipArtifactBody(
  kind: ArtifactKindName,
  body: string,
): GlanceClip {
  const raw = body.replace(/\r\n/g, "\n");
  if (kind === "code") {
    return clipLines(raw, GLANCE_CODE_LINES);
  }
  // Dense READMEs (lists, headings, few blank lines) still have to fit the
  // 300px island. Always enforce a line budget; also stop after a few blocks
  // so rendered headings/lists cannot balloon the pill.
  const byLines = clipLines(raw, GLANCE_MARKDOWN_LINES);
  const blocks = raw.split(/\n{2,}/).filter((b) => b.trim().length > 0);
  if (blocks.length <= GLANCE_MARKDOWN_BLOCKS) {
    return byLines;
  }
  const kept = blocks.slice(0, GLANCE_MARKDOWN_BLOCKS).join("\n\n");
  const byBlocks = clipLines(kept, GLANCE_MARKDOWN_LINES);
  const hidden = hiddenLineCount(raw.slice(byBlocks.text.length));
  return {
    text: byBlocks.text,
    hiddenLines: Math.max(byBlocks.hiddenLines, hidden),
    clipped: true,
  };
}

function clipLines(raw: string, max: number): GlanceClip {
  const lines = raw.split("\n");
  if (lines.length <= max) {
    return { text: raw, hiddenLines: 0, clipped: false };
  }
  return {
    text: lines.slice(0, max).join("\n").trimEnd(),
    hiddenLines: lines.length - max,
    clipped: true,
  };
}

function hiddenLineCount(rest: string): number {
  if (!rest.trim()) return 0;
  return rest.split("\n").filter((l) => l.length > 0).length;
}
