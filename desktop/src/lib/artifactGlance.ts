/** Overlay glance: clip a card so it fits the island (no scroll). */

export const GLANCE_CODE_LINES = 8;
export const GLANCE_MARKDOWN_BLOCKS = 8;

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
  const blocks = raw.split(/\n{2,}/).filter((b) => b.trim().length > 0);
  if (blocks.length <= GLANCE_MARKDOWN_BLOCKS) {
    return clipLines(raw, 18);
  }
  const kept = blocks.slice(0, GLANCE_MARKDOWN_BLOCKS).join("\n\n");
  const hidden = hiddenLineCount(blocks.slice(GLANCE_MARKDOWN_BLOCKS).join("\n\n"));
  return { text: kept.trimEnd(), hiddenLines: hidden, clipped: hidden > 0 };
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
