import { useEffect, useState } from "react";
import { escapeHtml, highlightCodeAsync } from "@/lib/artifactHighlight";
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
  const languageKey = language ?? null;
  const [rendered, setRendered] = useState(() => ({
    source,
    language: languageKey,
    html: escapeHtml(source),
  }));
  // Prop changes render escaped current source immediately; never flash the
  // previous artifact while its dynamically imported grammar is loading.
  const html =
    rendered.source === source && rendered.language === languageKey
      ? rendered.html
      : escapeHtml(source);
  useEffect(() => {
    let live = true;
    void highlightCodeAsync(source, languageKey).then((next) => {
      if (live) setRendered({ source, language: languageKey, html: next });
    });
    return () => {
      live = false;
    };
  }, [source, languageKey]);

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
