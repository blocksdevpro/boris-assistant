import { useEffect, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { Code2, FileText } from "lucide-react";
import { getSessionArtifact, type ArtifactCard, type ArtifactPeek } from "@/bridge";
import { artifactKindOf, clipArtifactBody } from "@/lib/artifactGlance";
import { isTauriRuntime } from "@/lib/runtime";
import { ArtifactCode } from "./ArtifactCode";
import { ArtifactMarkdown } from "./ArtifactMarkdown";
import { overlayContentMotion } from "@/lib/overlayMotion";
import { OverlayText } from "@/windows/overlay/OverlayText";

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
export function OverlayArtifactCard({ peek, fallback }: { peek: ArtifactPeek; fallback?: ArtifactCard | null }) {
  const [body, setBody] = useState<string | null>(null);
  const [loadState, setLoadState] = useState<
    "loading" | "ready" | "unavailable"
  >("loading");
  const reduceMotion = Boolean(useReducedMotion());

  useEffect(() => {
    if (fallback?.id === peek.id) {
      setBody(fallback.body);
      setLoadState("ready");
      return;
    }
    let active = true;
    setBody(null);
    setLoadState("loading");
    void getSessionArtifact(peek.id)
      .then((card) => {
        if (!active) return;
        if (card?.body) {
          setBody(card.body);
          setLoadState("ready");
          return;
        }
        if (isTauriRuntime()) {
          setLoadState("unavailable");
        } else {
          setBody(
            artifactKindOf(peek.kind) === "code"
              ? FIXTURE_CODE
              : FIXTURE_MARKDOWN,
          );
          setLoadState("ready");
        }
      })
      .catch(() => {
        if (!active) return;
        if (isTauriRuntime()) {
          setLoadState("unavailable");
        } else {
          setBody(
            artifactKindOf(peek.kind) === "code"
              ? FIXTURE_CODE
              : FIXTURE_MARKDOWN,
          );
          setLoadState("ready");
        }
      });
    return () => {
      active = false;
    };
  }, [peek.id, peek.kind, fallback]);

  const kind = artifactKindOf(peek.kind);
  const clip = loadState === "ready" && body ? clipArtifactBody(kind, body) : null;

  return (
    <motion.article
      data-tauri-drag-region
      aria-labelledby={`overlay-artifact-${peek.id}`}
      className="overlay-card overlay-caption mt-2 flex min-h-0 min-w-0 w-full flex-col overflow-hidden rounded-[12px] border-white/[0.07] px-2.5 py-2 shadow-[inset_0_1px_0_rgba(255,255,255,0.025)]"
      layout={reduceMotion ? false : "position"}
    >
      <div
        data-tauri-drag-region
        className="mb-1.5 flex min-w-0 shrink-0 items-center gap-2"
      >
        <span className="flex size-6 shrink-0 items-center justify-center rounded-[7px] bg-white/[0.07] text-white/55 ring-1 ring-inset ring-white/[0.05]">
          {kind === "code" ? (
            <Code2 className="size-3" strokeWidth={1.9} aria-hidden="true" />
          ) : (
            <FileText className="size-3" strokeWidth={1.9} aria-hidden="true" />
          )}
        </span>
        <div data-tauri-drag-region className="min-w-0 flex-1">
          <p
            data-tauri-drag-region
            className="text-[9px] font-medium uppercase leading-none tracking-[0.1em] text-white/38"
          >
            Created card
          </p>
          <h2
            id={`overlay-artifact-${peek.id}`}
            data-tauri-drag-region
            className="mt-1 truncate text-[12px] font-semibold leading-none tracking-[-0.015em] text-white/90"
          >
            <OverlayText text={peek.title} />
          </h2>
        </div>
      </div>

      <div
        data-tauri-drag-region
        className="overlay-card__body relative min-h-0 min-w-0 flex-1 overflow-hidden break-words"
      >
        <AnimatePresence mode="popLayout" initial={false}>
          {clip ? (
            <motion.div
              key="content"
              data-tauri-drag-region
              {...overlayContentMotion(reduceMotion)}
            >
              {kind === "code" ? (
                <ArtifactCode source={clip.text} language={peek.language} compact />
              ) : (
                <ArtifactMarkdown source={clip.text} compact />
              )}
            </motion.div>
          ) : loadState === "unavailable" ? (
            <motion.p
              key="unavailable"
              className="py-2 text-[11px] leading-relaxed text-white/48"
              {...overlayContentMotion(reduceMotion)}
            >
              Preview unavailable. Open Boris to view this card.
            </motion.p>
          ) : (
            <motion.div
              key="loading"
              role="status"
              aria-label="Loading card preview"
              className="space-y-1.5 py-1"
              {...overlayContentMotion(reduceMotion)}
            >
              <span className="block h-2 w-4/5 animate-pulse rounded-full bg-white/[0.07]" />
              <span className="block h-2 w-3/5 animate-pulse rounded-full bg-white/[0.05]" />
            </motion.div>
          )}
        </AnimatePresence>
      </div>

      <p
        data-tauri-drag-region
        className="mt-1.5 h-3 shrink-0 text-[9px] font-medium leading-tight text-white/38"
      >
        <OverlayText text={clip?.clipped
          ? `+${clip.hiddenLines} more · Open Boris for the full card`
          : clip || loadState === "unavailable"
            ? "Open Boris for the full card"
            : "\u00a0"} />
      </p>
    </motion.article>
  );
}
