import { useEffect, useState } from "react";
import { getSessionArtifact, type ArtifactPeek } from "@/bridge";
import { artifactKindOf, clipArtifactBody } from "@/lib/artifactGlance";
import { ArtifactCode } from "./ArtifactCode";
import { ArtifactMarkdown } from "./ArtifactMarkdown";

const FIXTURE_MARKDOWN = `# Weekly meal plan

- Mon — rice and eggs
- Tue — leftovers, trust me
- Wed — something with garlic`;

const FIXTURE_CODE = `Get-ChildItem .\\photos -File |
  ForEach-Object { $_.Name }`;

/**
 * Glance of the current card inside the overlay island.
 * Click-through: no buttons, no scroll. Full card lives in the main window.
 */
export function OverlayArtifactCard({ peek }: { peek: ArtifactPeek }) {
  const [body, setBody] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setBody(null);
    void getSessionArtifact(peek.id).then((card) => {
      if (!active) return;
      if (card?.body) {
        setBody(card.body);
        return;
      }
      setBody(artifactKindOf(peek.kind) === "code" ? FIXTURE_CODE : FIXTURE_MARKDOWN);
    });
    return () => {
      active = false;
    };
  }, [peek.id, peek.kind]);

  const kind = artifactKindOf(peek.kind);
  const clip = body ? clipArtifactBody(kind, body) : null;

  return (
    <div className="overlay-caption mt-1.5 min-w-0 max-w-[372px] overflow-hidden rounded-[10px] px-2 py-1.5">
      <div className="mb-1 flex min-w-0 items-baseline gap-1.5">
        <span className="shrink-0 text-[10px] font-semibold uppercase tracking-[0.06em] text-white/55">
          Card
        </span>
        <span className="min-w-0 truncate text-[12px] font-medium text-white/90">
          {peek.title}
        </span>
      </div>
      <div className="min-w-0 overflow-hidden">
        {clip ? (
          kind === "code" ? (
            <ArtifactCode source={clip.text} language={peek.language} compact />
          ) : (
            <ArtifactMarkdown source={clip.text} compact />
          )
        ) : (
          <p className="text-[11px] text-white/40">Loading card…</p>
        )}
      </div>
      {clip?.clipped ? (
        <p className="mt-1.5 text-[10px] leading-tight text-white/40">
          +{clip.hiddenLines} more · full card in Boris
        </p>
      ) : (
        <p className="mt-1.5 text-[10px] leading-tight text-white/35">
          Full card in Boris
        </p>
      )}
    </div>
  );
}
