import { highlightCode } from "@/lib/artifactHighlight";
import { cn } from "@/lib/utils";

export function ArtifactCode({
  source,
  language,
  compact = false,
}: {
  source: string;
  language?: string | null;
  compact?: boolean;
}) {
  const html = highlightCode(source, language);

  return (
    <pre
      className={cn(
        "artifact-code min-w-0 rounded-[10px] bg-[#1e1e1e] px-2.5 py-2 font-mono",
        compact
          ? "overflow-hidden whitespace-pre-wrap break-words text-[11px] leading-[1.4]"
          : "max-h-[420px] overflow-auto whitespace-pre text-[13px] leading-[1.5]",
      )}
    >
      <code
        className="artifact-code__tokens block"
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </pre>
  );
}
