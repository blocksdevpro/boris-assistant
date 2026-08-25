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
      aria-label={`${languageKey || "Plain text"} code`}
      tabIndex={compact ? undefined : 0}
      className={cn(
        "artifact-code artifact-code--premium min-w-0 border border-white/[0.07] bg-[#111214] font-mono text-[#f2f2f7] shadow-[inset_0_1px_0_rgba(255,255,255,0.04)] outline-none transition-[border-color,box-shadow] duration-200 focus-visible:border-white/20 focus-visible:ring-2 focus-visible:ring-white/10",
        compact
          ? "overflow-hidden rounded-[8px] px-2.5 py-2 whitespace-pre-wrap break-words text-[11px] leading-[1.5]"
          : "max-h-[440px] overflow-auto rounded-[12px] px-4 py-3.5 whitespace-pre text-[13px] leading-[1.6] selection:bg-blue-400/25",
      )}
    >
      <code
        className="artifact-code__tokens block"
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </pre>
  );
}
