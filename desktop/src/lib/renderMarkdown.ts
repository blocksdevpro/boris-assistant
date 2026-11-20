/**
 * Tiny markdown subset → nodes. No HTML. Safe to render from model text.
 */

export type MdNode =
  | { type: "h"; level: 1 | 2 | 3; text: string }
  | { type: "p"; text: string }
  | { type: "ul"; items: string[] }
  | { type: "ol"; items: string[] }
  | { type: "pre"; lang: string; text: string };

export function parseMarkdown(src: string): MdNode[] {
  const lines = src.replace(/\r\n/g, "\n").split("\n");
  const out: MdNode[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i] ?? "";
    if (!line.trim()) {
      i += 1;
      continue;
    }
    if (line.startsWith("```")) {
      const lang = line.slice(3).trim();
      const buf: string[] = [];
      i += 1;
      while (i < lines.length && !(lines[i] ?? "").startsWith("```")) {
        buf.push(lines[i] ?? "");
        i += 1;
      }
      if (i < lines.length) i += 1;
      out.push({ type: "pre", lang, text: buf.join("\n") });
      continue;
    }
    const heading = /^(#{1,3})\s+(.+)$/.exec(line);
    if (heading) {
      const level = heading[1].length as 1 | 2 | 3;
      out.push({ type: "h", level, text: heading[2].trim() });
      i += 1;
      continue;
    }
    if (/^\s*[-*]\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*[-*]\s+/.test(lines[i] ?? "")) {
        const item = [(lines[i] ?? "").replace(/^\s*[-*]\s+/, "")];
        i += 1;
        i = takeContinuation(lines, i, item);
        items.push(item.join(" "));
      }
      out.push({ type: "ul", items });
      continue;
    }
    if (/^\s*\d+\.\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i] ?? "")) {
        const item = [(lines[i] ?? "").replace(/^\s*\d+\.\s+/, "")];
        i += 1;
        i = takeContinuation(lines, i, item);
        items.push(item.join(" "));
      }
      out.push({ type: "ol", items });
      continue;
    }
    const buf = [line];
    i += 1;
    while (
      i < lines.length &&
      (lines[i] ?? "").trim() &&
      !/^(#{1,3})\s+/.test(lines[i] ?? "") &&
      !/^\s*[-*]\s+/.test(lines[i] ?? "") &&
      !/^\s*\d+\.\s+/.test(lines[i] ?? "") &&
      !(lines[i] ?? "").startsWith("```")
    ) {
      buf.push(lines[i] ?? "");
      i += 1;
    }
    out.push({ type: "p", text: buf.join(" ") });
  }
  return out;
}

/**
 * Indented, non-blank lines after a list item are a hard-wrapped
 * continuation of it (as in release notes wrapped at ~80 columns).
 * Appends them to `item` and returns the next unread index.
 */
function takeContinuation(lines: string[], i: number, item: string[]): number {
  while (i < lines.length) {
    const next = lines[i] ?? "";
    if (
      !/^\s+\S/.test(next) ||
      /^\s*[-*]\s+/.test(next) ||
      /^\s*\d+\.\s+/.test(next)
    ) {
      break;
    }
    item.push(next.trim());
    i += 1;
  }
  return i;
}

export type InlinePart =
  | { t: "text" | "code" | "strong" | "em"; v: string }
  | { t: "link"; v: string; href: string };

/**
 * Split inline `code`, **bold**, *italic*, [label](url) — still text-only,
 * no HTML. Hosts decide whether a link is clickable.
 */
export function splitInline(text: string): InlinePart[] {
  const out: InlinePart[] = [];
  const re = /(`[^`]+`|\[[^\]]+\]\([^)\s]+\)|\*\*[^*]+\*\*|\*[^*]+\*)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push({ t: "text", v: text.slice(last, m.index) });
    const tok = m[0];
    if (tok.startsWith("`")) out.push({ t: "code", v: tok.slice(1, -1) });
    else if (tok.startsWith("[")) {
      const close = tok.indexOf("](");
      out.push({ t: "link", v: tok.slice(1, close), href: tok.slice(close + 2, -1) });
    } else if (tok.startsWith("**")) out.push({ t: "strong", v: tok.slice(2, -2) });
    else out.push({ t: "em", v: tok.slice(1, -1) });
    last = m.index + tok.length;
  }
  if (last < text.length) out.push({ t: "text", v: text.slice(last) });
  return out;
}
