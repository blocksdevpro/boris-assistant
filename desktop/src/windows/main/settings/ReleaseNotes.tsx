import { useMemo, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { MdNode } from "@/lib/renderMarkdown";
import { splitInline } from "@/lib/renderMarkdown";
import { releaseNotesNodes } from "@/lib/releaseNotes";

function openExternal(url: string) {
  void openUrl(url).catch(() =>
    window.open(url, "_blank", "noopener,noreferrer"),
  );
}

/** Rendered release notes for a pending update (markdown subset, no HTML). */
export function ReleaseNotes({
  body,
  advanced,
}: {
  body: string;
  advanced: boolean;
}) {
  const nodes = useMemo(
    () => releaseNotesNodes(body, { advanced }),
    [body, advanced],
  );
  if (nodes.length === 0) return null;
  return (
    <div
      className="max-h-72 overflow-y-auto border-t border-white/[0.06] px-4 py-3"
      aria-label="Release notes"
    >
      <div className="flex flex-col gap-2 text-[12px] leading-relaxed text-white/55">
        {nodes.map((node, i) => (
          <NoteBlock key={i} node={node} first={i === 0} />
        ))}
      </div>
    </div>
  );
}

function NoteBlock({ node, first }: { node: MdNode; first: boolean }) {
  switch (node.type) {
    case "h":
      return (
        <p
          className={`text-[12.5px] font-medium tracking-[-0.01em] text-white/85 ${first ? "" : "mt-2"}`}
        >
          <Inline text={node.text} />
        </p>
      );
    case "p":
      return (
        <p>
          <Inline text={node.text} />
        </p>
      );
    case "ul":
    case "ol":
      return (
        <ListTag ordered={node.type === "ol"}>
          {node.items.map((item, i) => (
            <li key={i}>
              <Inline text={item} />
            </li>
          ))}
        </ListTag>
      );
    case "pre":
      return (
        <pre className="overflow-x-auto rounded-md bg-white/[0.05] px-2.5 py-2 font-mono text-[11px] text-white/70">
          {node.text}
        </pre>
      );
  }
}

function ListTag({
  ordered,
  children,
}: {
  ordered: boolean;
  children: ReactNode;
}) {
  const cls = "space-y-1 pl-4 marker:text-white/30";
  return ordered ? (
    <ol className={`list-decimal ${cls}`}>{children}</ol>
  ) : (
    <ul className={`list-disc ${cls}`}>{children}</ul>
  );
}

function Inline({ text }: { text: string }) {
  return (
    <>
      {splitInline(text).map((part, i) => {
        switch (part.t) {
          case "code":
            return (
              <code
                key={i}
                className="rounded bg-white/[0.07] px-1 py-px font-mono text-[0.92em] text-white/75"
              >
                {part.v}
              </code>
            );
          case "strong":
            return (
              <strong key={i} className="font-medium text-white/80">
                {part.v}
              </strong>
            );
          case "em":
            return <em key={i}>{part.v}</em>;
          case "link":
            return /^https:\/\//i.test(part.href) ? (
              <button
                key={i}
                type="button"
                onClick={() => openExternal(part.href)}
                className="text-white/75 underline decoration-white/25 underline-offset-2 hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
              >
                {part.v}
              </button>
            ) : (
              <span key={i}>{part.v}</span>
            );
          default:
            return <span key={i}>{part.v}</span>;
        }
      })}
    </>
  );
}
