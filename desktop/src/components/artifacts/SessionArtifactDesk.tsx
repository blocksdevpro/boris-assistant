import { useCallback, useEffect, useState } from "react";
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

/**
 * This session's cards — list + reader. Hooked into Home, not a new window.
 */
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

  const refresh = useCallback(async () => {
    const next = await listSessionArtifacts();
    setItems(next);
    setSelected((prev) => {
      if (prev && next.some((i) => i.id === prev)) return prev;
      const current = next.find((i) => i.current);
      return current?.id ?? next[0]?.id ?? null;
    });
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh, peek?.id, engineOn]);

  useEffect(() => {
    if (!selected) {
      setCard(null);
      return;
    }
    let active = true;
    void getSessionArtifact(selected).then((next) => {
      if (active) setCard(next);
    });
    return () => {
      active = false;
    };
  }, [selected]);

  const onCopy = useCallback(async () => {
    if (!card?.body) return;
    try {
      await navigator.clipboard.writeText(card.body);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    } catch {
      setCopied(false);
    }
  }, [card?.body]);

  if (!engineOn && items.length === 0) return null;
  if (items.length === 0) return null;

  const kind = card ? artifactKindOf(card.kind) : "markdown";

  return (
    <section
      aria-labelledby="session-cards-heading"
      className="settings-group flex min-h-0 flex-col gap-3 rounded-[16px] px-5 py-4"
    >
      <div className="flex items-baseline justify-between gap-3">
        <h2
          id="session-cards-heading"
          className="text-[12px] font-medium uppercase tracking-[0.08em] text-white/40"
        >
          Cards · this session
        </h2>
        <span className="text-[11px] text-white/30">
          {items.length} {items.length === 1 ? "card" : "cards"}
        </span>
      </div>

      <ul className="flex flex-col gap-1">
        {items.map((item) => {
          const active = item.id === selected;
          return (
            <li key={item.id}>
              <button
                type="button"
                onClick={() => setSelected(item.id)}
                className={cn(
                  "flex w-full min-w-0 items-baseline gap-2 rounded-[10px] px-2.5 py-1.5 text-left",
                  active ? "bg-white/[0.08]" : "hover:bg-white/[0.04]",
                )}
              >
                <span className="min-w-0 flex-1 truncate text-[13px] text-white/88">
                  {item.title}
                </span>
                <span className="shrink-0 text-[10px] uppercase tracking-[0.04em] text-white/35">
                  {artifactKindOf(item.kind) === "code"
                    ? item.language || "code"
                    : "md"}
                </span>
                {item.current ? (
                  <span className="shrink-0 text-[10px] text-white/45">now</span>
                ) : null}
              </button>
            </li>
          );
        })}
      </ul>

      {card ? (
        <div className="min-w-0 border-t border-white/[0.06] pt-3">
          <div className="mb-2 flex items-center justify-between gap-2">
            <p className="min-w-0 truncate text-[12px] text-white/40">{card.path}</p>
            <button
              type="button"
              onClick={() => void onCopy()}
              className="h-7 shrink-0 rounded-full bg-white/10 px-2.5 text-[12px] text-white/80 hover:bg-white/15"
            >
              {copied ? "Copied" : "Copy"}
            </button>
          </div>
          {kind === "code" ? (
            <ArtifactCode source={card.body} language={card.language} />
          ) : (
            <ArtifactMarkdown source={card.body} />
          )}
        </div>
      ) : null}
    </section>
  );
}
