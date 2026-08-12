import { highlightCode } from "@/lib/artifactHighlight";
import { parseMarkdown, splitInline, type MdNode } from "@/lib/renderMarkdown";
import { cn } from "@/lib/utils";

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
        "min-w-0 text-white/88",
        compact ? "space-y-1.5 text-[12px] leading-[1.4]" : "space-y-2.5 text-[15px] leading-relaxed",
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
            : "text-[18px] font-semibold tracking-[-0.02em] text-white"
          : node.level === 2
            ? compact
              ? "text-[12px] font-semibold text-white/90"
              : "text-[16px] font-semibold text-white/92"
            : "text-[12px] font-medium text-white/80";
      return (
        <p className={cls}>
          <Inline text={node.text} />
        </p>
      );
    }
    case "p":
      return (
        <p className={compact ? "text-white/75" : "text-white/85"}>
          <Inline text={node.text} />
        </p>
      );
    case "ul":
      return (
        <ul className={cn("list-disc space-y-0.5 pl-4", compact ? "text-white/75" : "text-white/85")}>
          {node.items.map((item, i) => (
            <li key={i}>
              <Inline text={item} />
            </li>
          ))}
        </ul>
      );
    case "ol":
      return (
        <ol className={cn("list-decimal space-y-0.5 pl-4", compact ? "text-white/75" : "text-white/85")}>
          {node.items.map((item, i) => (
            <li key={i}>
              <Inline text={item} />
            </li>
          ))}
        </ol>
      );
    case "pre":
      return (
        <pre
          className={cn(
            "artifact-code rounded-[8px] bg-[#1e1e1e] px-2 py-1.5 font-mono",
            compact
              ? "overflow-hidden whitespace-pre text-[11px] leading-[1.4]"
              : "overflow-auto whitespace-pre text-[13px] leading-[1.5]",
          )}
        >
          <code
            className="artifact-code__tokens block"
            dangerouslySetInnerHTML={{
              __html: highlightCode(node.text, node.lang || null),
            }}
          />
        </pre>
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
              className="rounded-[4px] bg-white/10 px-1 py-px font-mono text-[0.92em] text-white/90"
            >
              {part.v}
            </code>
          );
        }
        if (part.t === "strong") return <strong key={i}>{part.v}</strong>;
        if (part.t === "em") return <em key={i}>{part.v}</em>;
        return <span key={i}>{part.v}</span>;
      })}
    </>
  );
}
