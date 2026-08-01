import {
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { Mic, Volume2 } from "lucide-react";
import { useStatus, type StatusPicture } from "@/bridge";
import { cn } from "@/lib/utils";
import { toneFor } from "@/lib/phaseVisual";

/**
 * OVERLAY — always-on-top voice island.
 *
 * Transparency follows Tauri: `transparent: true` + CSS/html transparent
 * backgrounds + webview default color alpha 0. Empty chrome must stay
 * transparent — never solid black.
 *
 * Phase changes crossfade label/hint/caption and morph orb accent so
 * transitions don't pop.
 */
export function OverlayWindow() {
  const status = useStatus();
  const tone = useMemo(
    () => toneFor(status.phase, status.engine),
    [status.phase, status.engine],
  );

  const caption = pickCaption(status);
  const showCaption = Boolean(caption);

  // Keep last caption mounted briefly so exit can fade via grid collapse.
  const [displayCaption, setDisplayCaption] = useState(caption);
  useEffect(() => {
    if (caption) {
      setDisplayCaption(caption);
      return;
    }
    // Delay clear so CSS can collapse; content stays until shell closes.
    const t = window.setTimeout(() => setDisplayCaption(null), 280);
    return () => window.clearTimeout(t);
  }, [caption]);

  useEffect(() => {
    document.documentElement.classList.add("overlay-mode");
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    const meta = document.querySelector('meta[name="color-scheme"]');
    const prev = meta?.getAttribute("content") ?? null;
    meta?.setAttribute("content", "only light");
    return () => {
      if (prev != null) meta?.setAttribute("content", prev);
    };
  }, []);

  return (
    <div className="overlay-surface flex h-full w-full items-center justify-center bg-transparent p-3">
      <div
        data-tauri-drag-region
        className={cn(
          "overlay-island group relative inline-flex max-w-[min(360px,100%)] select-none flex-col gap-2",
          "rounded-[22px] px-3.5 py-2.5",
          "w-max min-w-[260px]",
        )}
        style={
          {
            "--island-accent": tone.accent,
            "--island-glow": tone.glow,
            borderColor: `color-mix(in oklch, ${tone.accent} 28%, rgba(255,255,255,0.14))`,
            boxShadow: `
              0 10px 28px rgba(12, 14, 20, 0.45),
              inset 0 1px 0 rgba(255, 255, 255, 0.1),
              0 0 24px color-mix(in oklch, ${tone.glow} 55%, transparent)
            `,
          } as CSSProperties
        }
      >
        <div data-tauri-drag-region className="flex items-center gap-3">
          <PresenceOrb motion={tone.motion} accent={tone.accent} />

          <div data-tauri-drag-region className="min-w-0 flex-1 pr-1">
            <div data-tauri-drag-region className="flex items-baseline gap-2">
              <FadeText
                textKey={`${status.phase}-${status.engine}-${tone.label}`}
                className="text-[13px] font-semibold tracking-tight text-white"
              >
                {tone.label}
              </FadeText>
              {status.turn ? (
                <span
                  data-tauri-drag-region
                  className="font-mono text-[10px] tabular-nums text-white/35 transition-opacity duration-300"
                >
                  #{status.turn}
                </span>
              ) : null}
            </div>
            {!showCaption ? (
              <FadeText
                textKey={`${status.phase}-${tone.hint}`}
                className="truncate text-[11px] leading-snug text-white/45"
              >
                {tone.hint}
              </FadeText>
            ) : (
              // Reserve one line so the island doesn't jump when caption opens.
              <p
                data-tauri-drag-region
                className="h-[1.125rem] truncate text-[11px] leading-snug text-transparent"
                aria-hidden
              >
                .
              </p>
            )}
          </div>

          <DevicePips status={status} />
        </div>

        <div
          data-tauri-drag-region
          className="overlay-caption-shell"
          data-open={showCaption ? "true" : "false"}
        >
          <div className="overlay-caption-inner">
            {displayCaption ? (
              <div
                data-tauri-drag-region
                className="overlay-caption overlay-caption-body min-w-0 max-w-[320px] rounded-xl px-2.5 py-1.5"
                key={`${displayCaption.kind}-${displayCaption.text.slice(0, 48)}`}
              >
                <p
                  data-tauri-drag-region
                  className={cn(
                    "line-clamp-2 text-[12px] leading-snug tracking-tight transition-colors duration-300",
                    displayCaption.kind === "error"
                      ? "text-red-300/95"
                      : displayCaption.kind === "said"
                        ? "text-white/80"
                        : "text-white/65",
                  )}
                >
                  {displayCaption.kind !== "error" ? (
                    <span className="mr-1.5 text-[10px] font-medium uppercase tracking-wider text-white/30">
                      {displayCaption.kind === "said" ? "Boris" : "You"}
                    </span>
                  ) : null}
                  {displayCaption.text}
                </p>
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}

/** Remount + fade when `textKey` changes so phase labels don't pop. */
function FadeText({
  textKey,
  className,
  children,
}: {
  textKey: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <span
      key={textKey}
      data-tauri-drag-region
      className={cn("overlay-fade-in inline-block max-w-full", className)}
    >
      {children}
    </span>
  );
}

function PresenceOrb({
  motion,
  accent,
}: {
  motion: ReturnType<typeof toneFor>["motion"];
  accent: string;
}) {
  return (
    <div
      data-tauri-drag-region
      className="relative flex size-9 shrink-0 items-center justify-center"
      aria-hidden
    >
      {motion !== "none" ? (
        <>
          <span
            key={`ring-${motion}`}
            className={cn(
              "overlay-orb-ring absolute inset-0 rounded-full border-[1.5px] overlay-fade-in",
              motion === "listen" && "overlay-ring-listen",
              motion === "think" && "overlay-ring-think",
              motion === "speak" && "overlay-ring-speak",
              motion === "breathe" && "overlay-ring-breathe",
            )}
            style={{ borderColor: accent }}
          />
          {(motion === "listen" || motion === "speak") && (
            <span
              key={`ring2-${motion}`}
              className="overlay-orb-ring overlay-ring-listen-delay absolute inset-0.5 rounded-full border overlay-fade-in"
              style={{ borderColor: accent }}
            />
          )}
        </>
      ) : null}

      <span
        className={cn(
          "overlay-orb-core relative size-3 rounded-full",
          motion === "breathe" && "overlay-core-breathe",
          motion === "listen" && "overlay-core-listen",
          motion === "think" && "overlay-core-think",
          motion === "speak" && "overlay-core-speak",
        )}
        style={{
          background: accent,
          boxShadow: `0 0 14px ${accent}`,
        }}
      />
    </div>
  );
}

function DevicePips({ status }: { status: StatusPicture }) {
  return (
    <div data-tauri-drag-region className="flex shrink-0 items-center gap-1.5">
      <Pip
        ok={status.mic.ok && status.engine !== "Off"}
        title={`Mic · ${status.mic.label}`}
        icon={<Mic className="size-3" strokeWidth={2} />}
      />
      <Pip
        ok={status.speaker.ok && status.engine !== "Off"}
        title={`Speaker · ${status.speaker.label}`}
        icon={<Volume2 className="size-3" strokeWidth={2} />}
      />
    </div>
  );
}

function Pip({
  ok,
  title,
  icon,
}: {
  ok: boolean;
  title: string;
  icon: ReactNode;
}) {
  return (
    <div
      data-tauri-drag-region
      title={title}
      className={cn(
        "relative flex size-7 items-center justify-center rounded-full border transition-all duration-300",
        ok
          ? "border-white/10 bg-white/5 text-white/70"
          : "border-white/5 bg-white/[0.03] text-white/25",
      )}
    >
      {icon}
      <span
        className={cn(
          "absolute bottom-0.5 right-0.5 size-1.5 rounded-full ring-1 ring-black/40 transition-colors duration-300",
          ok ? "bg-emerald-400" : "bg-white/20",
        )}
        aria-hidden
      />
    </div>
  );
}

type Caption = {
  kind: "heard" | "said" | "error";
  text: string;
};

function pickCaption(status: StatusPicture): Caption | null {
  if (status.detail?.trim()) {
    return { kind: "error", text: status.detail.trim() };
  }
  if (
    (status.phase === "Talking" || status.phase === "AwaitingReply") &&
    status.said?.trim()
  ) {
    return { kind: "said", text: status.said.trim() };
  }
  if (
    (status.phase === "Thinking" ||
      status.phase === "Reading" ||
      status.phase === "Hearing" ||
      status.phase === "Talking" ||
      status.phase === "AwaitingReply") &&
    status.heard?.trim()
  ) {
    if (
      (status.phase !== "Talking" && status.phase !== "AwaitingReply") ||
      !status.said?.trim()
    ) {
      return { kind: "heard", text: status.heard.trim() };
    }
  }
  if (status.said?.trim() && status.phase === "Armed") {
    return { kind: "said", text: status.said.trim() };
  }
  if (status.heard?.trim() && status.engine === "On") {
    return { kind: "heard", text: status.heard.trim() };
  }
  return null;
}
