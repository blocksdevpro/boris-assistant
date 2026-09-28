import { parseMarkdown, splitInline, type MdNode } from "@/lib/renderMarkdown";
import { cn } from "@/lib/utils";
import { ArtifactCode } from "./ArtifactCode";

export function ArtifactMarkdown({
  source,
  compact = false,
}: {
  source: string;
  compact?: boolean;
}) {
  const nodes = parseMarkdown(source);
  return (
    <div
      className={cn(
        "artifact-markdown min-w-0 text-white/88 selection:bg-blue-400/25",
        compact
          ? "artifact-markdown--compact space-y-1.5 overflow-hidden break-words text-[12px] leading-[1.45]"
          : "space-y-3.5 text-[14px] leading-[1.7]",
      )}
    >
      {nodes.map((node, i) => (
        <MdBlock key={i} node={node} compact={compact} />
      ))}
    </div>
  );
}

function MdBlock({ node, compact }: { node: MdNode; compact: boolean }) {
  switch (node.type) {
    case "h": {
      const cls =
        node.level === 1
          ? compact
            ? "text-[13px] font-semibold tracking-[-0.02em] text-white"
            : "text-[20px] font-semibold tracking-[-0.03em] text-white"
          : node.level === 2
            ? compact
              ? "text-[12px] font-semibold text-white/90"
              : "text-[16px] font-semibold tracking-[-0.02em] text-white/92"
            : compact
              ? "text-[12px] font-medium text-white/80"
              : "text-[14px] font-semibold text-white/86";
      const content = <Inline text={node.text} />;
      if (compact) return <p className={cls}>{content}</p>;
      if (node.level === 1) return <h4 className={cls}>{content}</h4>;
      if (node.level === 2) return <h5 className={cls}>{content}</h5>;
      return <h6 className={cls}>{content}</h6>;
    }
    case "p":
      return (
        <p className={compact ? "text-white/72" : "text-white/82"}>
          <Inline text={node.text} />
        </p>
      );
    case "ul":
      return (
        <ul
          className={cn(
            "list-disc space-y-1 pl-5 marker:text-white/35",
            compact ? "text-white/72" : "text-white/82",
          )}
        >
          {node.items.map((item, i) => (
            <li key={i}>
              <Inline text={item} />
            </li>
          ))}
        </ul>
      );
    case "ol":
      return (
        <ol
          className={cn(
            "list-decimal space-y-1 pl-5 marker:text-white/40",
            compact ? "text-white/72" : "text-white/82",
          )}
        >
          {node.items.map((item, i) => (
            <li key={i}>
              <Inline text={item} />
            </li>
          ))}
        </ol>
      );
    case "pre":
      return (
        <ArtifactCode
          source={node.text}
          language={node.lang || null}
          compact={compact}
        />
      );
  }
}

function Inline({ text }: { text: string }) {
  return (
    <>
      {splitInline(text).map((part, i) => {
        if (part.t === "code") {
          return (
            <code
              key={i}
              className="rounded-[5px] border border-white/[0.06] bg-white/[0.08] px-1.5 py-0.5 font-mono text-[0.9em] text-white/90"
            >
              {part.v}
            </code>
          );
        }
        if (part.t === "strong")
          return (
            <strong key={i} className="font-semibold text-white/95">
              {part.v}
            </strong>
          );
        if (part.t === "em") return <em key={i}>{part.v}</em>;
        return <span key={i}>{part.v}</span>;
      })}
    </>
  );
}
