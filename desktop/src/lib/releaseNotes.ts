import { parseMarkdown, type MdNode } from "./renderMarkdown";

/**
 * GitHub release body → nodes for the Updates panel.
 *
 * Drops the leading title heading (the row already says "vX ready") and,
 * unless advanced settings are shown, the "Developer notes" section. The
 * closing "Full changes: …" link line is kept even when it trails that section.
 */
export function releaseNotesNodes(
  body: string,
  { advanced }: { advanced: boolean },
): MdNode[] {
  const nodes = parseMarkdown(body);
  const first = nodes[0];
  const start = first?.type === "h" && first.level <= 2 ? 1 : 0;
  const out: MdNode[] = [];
  let skipLevel: number | null = null;
  for (const node of nodes.slice(start)) {
    if (node.type === "h") {
      if (skipLevel != null && node.level <= skipLevel) skipLevel = null;
      if (!advanced && /^developer notes$/i.test(node.text.trim())) {
        skipLevel = node.level;
      }
    }
    const fullChanges = node.type === "p" && /^full changes\b/i.test(node.text);
    if (skipLevel == null || fullChanges) out.push(node);
  }
  return out;
}
