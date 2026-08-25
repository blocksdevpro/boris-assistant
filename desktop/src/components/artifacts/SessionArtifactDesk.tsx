import { useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import {
  Check,
  Clipboard,
  Code2,
  FileText,
  Layers3,
  Pin,
} from "lucide-react";
import {
  getSessionArtifact,
  listSessionArtifacts,
  type ArtifactCard,
  type ArtifactListItem,
  type ArtifactPeek,
} from "@/bridge";
import { artifactKindOf } from "@/lib/artifactGlance";
import { cn } from "@/lib/utils";
import { ArtifactCode } from "./ArtifactCode";
import { ArtifactMarkdown } from "./ArtifactMarkdown";

const soft = [0.22, 1, 0.36, 1] as const;

/** This session's cards — a compact picker and full reader. */
export function SessionArtifactDesk({
  peek,
  engineOn,
}: {
  peek: ArtifactPeek | null | undefined;
  engineOn: boolean;
}) {
  const [items, setItems] = useState<ArtifactListItem[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [card, setCard] = useState<ArtifactCard | null>(null);
  const [copied, setCopied] = useState(false);
  const [loading, setLoading] = useState(false);
  const copyTimer = useRef<number | null>(null);
  const reduceMotion = Boolean(useReducedMotion());

  const refresh = useCallback(async () => {
    const next = await listSessionArtifacts();
    setItems(next);
    setSelected((previous) => {
      if (previous && next.some((item) => item.id === previous)) return previous;
      const current = next.find((item) => item.current);
      return current?.id ?? next[0]?.id ?? null;
    });
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh, peek?.id, engineOn]);

  useEffect(() => {
    if (!selected) {
      setCard(null);
      setLoading(false);
      return;
    }

    let active = true;
    setCard(null);
    setLoading(true);
    void getSessionArtifact(selected)
      .then((next) => {
        if (active) setCard(next);
      })
      .catch(() => {
        if (active) setCard(null);
      })
      .finally(() => {
        if (active) setLoading(false);
      });

    return () => {
      active = false;
    };
  }, [selected]);

  useEffect(
    () => () => {
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
    },
    [],
  );

  const onCopy = useCallback(async () => {
    if (!card?.body) return;
    try {
      await navigator.clipboard.writeText(card.body);
      setCopied(true);
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => {
        setCopied(false);
        copyTimer.current = null;
      }, 1600);
    } catch {
      setCopied(false);
    }
  }, [card?.body]);

  if (items.length === 0) return null;

  const kind = card ? artifactKindOf(card.kind) : "markdown";

  return (
    <section
      aria-labelledby="session-cards-heading"
      className="settings-group artifact-desk relative flex min-h-0 flex-col gap-4 overflow-hidden rounded-[20px] border border-white/[0.07] bg-[#1c1c1e]/95 px-5 py-4 shadow-[0_16px_48px_rgba(0,0,0,0.2),inset_0_1px_0_rgba(255,255,255,0.04)]"
    >
      <div className="flex items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-3">
          <span className="flex size-8 shrink-0 items-center justify-center rounded-[10px] bg-white/[0.07] text-white/72 ring-1 ring-inset ring-white/[0.06]">
            <Layers3 className="size-4" strokeWidth={1.8} aria-hidden="true" />
          </span>
          <div className="min-w-0">
            <p className="text-[10px] font-medium uppercase tracking-[0.11em] text-white/35">
              This session
            </p>
            <h2
              id="session-cards-heading"
              className="mt-0.5 text-[15px] font-semibold tracking-[-0.02em] text-white/92"
            >
              Created cards
            </h2>
          </div>
        </div>
        <span className="flex h-6 min-w-6 shrink-0 items-center justify-center rounded-full bg-white/[0.06] px-2 text-[11px] font-medium tabular-nums text-white/48 ring-1 ring-inset ring-white/[0.05]">
          {items.length}
        </span>
      </div>

      <ul
        className="artifact-desk__list flex flex-col gap-1 rounded-[13px] bg-black/10 p-1 ring-1 ring-inset ring-white/[0.045]"
        aria-label="Cards created in this session"
      >
        {items.map((item) => {
          const active = item.id === selected;
          const itemKind = artifactKindOf(item.kind);
          return (
            <li key={item.id}>
              <motion.button
                type="button"
                onClick={() => setSelected(item.id)}
                aria-pressed={active}
                className={cn(
                  "group relative flex min-h-10 w-full min-w-0 items-center gap-2.5 rounded-[10px] px-2.5 py-2 text-left outline-none transition-colors focus-visible:ring-2 focus-visible:ring-blue-400/55",
                  active
                    ? "bg-white/[0.09] shadow-[0_1px_6px_rgba(0,0,0,0.12),inset_0_1px_0_rgba(255,255,255,0.05)]"
                    : "text-white/70 hover:bg-white/[0.045]",
                )}
                whileHover={reduceMotion ? undefined : { x: active ? 0 : 2 }}
                whileTap={reduceMotion ? undefined : { scale: 0.99 }}
                transition={{ duration: reduceMotion ? 0 : 0.18, ease: soft }}
              >
                <span
                  className={cn(
                    "flex size-7 shrink-0 items-center justify-center rounded-[8px] transition-colors",
                    active
                      ? "bg-white/[0.08] text-white/85"
                      : "bg-white/[0.035] text-white/42 group-hover:text-white/60",
                  )}
                >
                  {itemKind === "code" ? (
                    <Code2 className="size-3.5" strokeWidth={1.8} aria-hidden="true" />
                  ) : (
                    <FileText className="size-3.5" strokeWidth={1.8} aria-hidden="true" />
                  )}
                </span>
                <span className="min-w-0 flex-1">
                  <span
                    className={cn(
                      "block truncate text-[13px]",
                      active ? "font-medium text-white/92" : "text-white/72",
                    )}
                  >
                    {item.title}
                  </span>
                  <span className="mt-0.5 block truncate text-[10px] text-white/30">
                    {itemKind === "code" ? item.language || "Code" : "Markdown"}
                    {item.revision > 1 ? ` · Revision ${item.revision}` : ""}
                  </span>
                </span>
                {item.pinned ? (
                  <Pin
                    className="size-3 shrink-0 text-white/32"
                    strokeWidth={1.8}
                    aria-label="Pinned"
                  />
                ) : null}
                {item.current ? (
                  <span className="shrink-0 rounded-full bg-emerald-400/10 px-1.5 py-0.5 text-[9px] font-medium uppercase tracking-[0.06em] text-emerald-200/72 ring-1 ring-inset ring-emerald-300/10">
                    Live
                  </span>
                ) : null}
              </motion.button>
            </li>
          );
        })}
      </ul>

      <div className="artifact-desk__reader min-w-0 border-t border-white/[0.065] pt-4">
        <AnimatePresence mode="wait" initial={false}>
          {card ? (
            <motion.article
              key={card.id}
              aria-labelledby={`artifact-${card.id}-title`}
              initial={reduceMotion ? false : { opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={reduceMotion ? undefined : { opacity: 0, y: -4 }}
              transition={{ duration: reduceMotion ? 0 : 0.28, ease: soft }}
            >
              <div className="mb-3 flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <h3
                    id={`artifact-${card.id}-title`}
                    className="truncate text-[14px] font-semibold tracking-[-0.015em] text-white/88"
                  >
                    {card.title}
                  </h3>
                  <p
                    className="mt-0.5 truncate text-[10px] text-white/32"
                    title={card.path}
                  >
                    {card.path}
                  </p>
                </div>
                <motion.button
                  type="button"
                  onClick={() => void onCopy()}
                  aria-label={copied ? "Copied card contents" : "Copy card contents"}
                  className={cn(
                    "flex h-8 shrink-0 items-center gap-1.5 rounded-full border px-3 text-[11px] font-medium outline-none transition-colors focus-visible:ring-2 focus-visible:ring-blue-400/55",
                    copied
                      ? "border-emerald-300/12 bg-emerald-400/10 text-emerald-100/85"
                      : "border-white/[0.07] bg-white/[0.07] text-white/72 hover:bg-white/[0.11] hover:text-white/90",
                  )}
                  whileTap={reduceMotion ? undefined : { scale: 0.96 }}
                  transition={{ duration: reduceMotion ? 0 : 0.16 }}
                >
                  <AnimatePresence mode="wait" initial={false}>
                    {copied ? (
                      <motion.span
                        key="copied"
                        className="flex items-center gap-1.5"
                        initial={reduceMotion ? false : { opacity: 0, scale: 0.9 }}
                        animate={{ opacity: 1, scale: 1 }}
                        exit={reduceMotion ? undefined : { opacity: 0 }}
                      >
                        <Check className="size-3" strokeWidth={2.2} aria-hidden="true" />
                        Copied
                      </motion.span>
                    ) : (
                      <motion.span
                        key="copy"
                        className="flex items-center gap-1.5"
                        initial={reduceMotion ? false : { opacity: 0 }}
                        animate={{ opacity: 1 }}
                        exit={reduceMotion ? undefined : { opacity: 0 }}
                      >
                        <Clipboard className="size-3" strokeWidth={1.9} aria-hidden="true" />
                        Copy
                      </motion.span>
                    )}
                  </AnimatePresence>
                </motion.button>
              </div>
              {kind === "code" ? (
                <ArtifactCode source={card.body} language={card.language} />
              ) : (
                <ArtifactMarkdown source={card.body} />
              )}
            </motion.article>
          ) : loading ? (
            <motion.div
              key="loading"
              role="status"
              aria-label="Loading card"
              className="space-y-2.5"
              initial={reduceMotion ? false : { opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={reduceMotion ? undefined : { opacity: 0 }}
            >
              <div className="h-3 w-32 animate-pulse rounded-full bg-white/[0.07]" />
              <div className="h-24 animate-pulse rounded-[12px] bg-white/[0.035] ring-1 ring-inset ring-white/[0.04]" />
            </motion.div>
          ) : (
            <p
              key="unavailable"
              role="status"
              className="py-5 text-center text-[12px] text-white/38"
            >
              This card is no longer available.
            </p>
          )}
        </AnimatePresence>
      </div>
      <span className="sr-only" aria-live="polite">
        {copied ? "Card contents copied to clipboard." : ""}
      </span>
    </section>
  );
}
