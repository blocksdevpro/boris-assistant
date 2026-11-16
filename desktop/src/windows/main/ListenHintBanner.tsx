import { MicOff, UserX, Volume2 } from "lucide-react";
import { openMicPrivacySettings, type StatusPicture } from "@/bridge";
import { logger } from "@/lib/logger";

/**
 * Says why "Hey Boris" is not doing anything right now. Without this, a
 * blocked mic or an ignored wake looks exactly like a broken app.
 */
export function ListenHintBanner({
  status,
  onOpenMicSettings,
  onTeachVoice,
  onOpenSpeechSettings,
}: {
  status: StatusPicture;
  /** Boris settings → Speech, where the microphone is chosen. */
  onOpenMicSettings: () => void;
  onTeachVoice: () => void;
  onOpenSpeechSettings: () => void;
}) {
  const listening =
    status.engine === "On" &&
    (status.phase === "Armed" || status.phase === "Quiet");
  const hint = listening ? status.listen_hint : null;
  if (!hint) return null;

  if (hint === "mic_silent") {
    const device =
      status.mic.label && status.mic.label !== "—"
        ? `“${status.mic.label}”`
        : "your microphone";
    return (
      <div role="alert" className="settings-group rounded-[12px] px-4 py-3.5">
        <p className="flex items-center gap-2 text-[15px] font-medium tracking-[-0.01em] text-white/90">
          <MicOff className="size-4 text-amber-300/90" strokeWidth={2} />
          Boris can&apos;t hear anything
        </p>
        <p className="mt-1 text-[12px] leading-snug text-[var(--boris-label-secondary)]">
          No sound is coming from {device}. Usually one of these:
        </p>
        <ul className="mt-2 list-disc space-y-1 pl-5 text-[12px] leading-snug text-[var(--boris-label-tertiary)]">
          <li>
            Windows is blocking desktop apps from the microphone. Turn on{" "}
            <span className="text-white/70">
              Let desktop apps access your microphone
            </span>
            .
          </li>
          <li>The mic is muted (mute key, headset switch, or volume at 0).</li>
          <li>A different microphone is selected than the one you talk into.</li>
        </ul>
        <div className="mt-3 flex flex-wrap gap-2">
          <button
            type="button"
            onClick={() =>
              void openMicPrivacySettings().catch((e) =>
                logger.error("openMicPrivacySettings failed", String(e)),
              )
            }
            className="h-8 rounded-full bg-white/10 px-3.5 text-[13px] text-white hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
          >
            Open Windows mic settings
          </button>
          <button
            type="button"
            onClick={onOpenMicSettings}
            className="h-8 rounded-full px-3 text-[13px] text-[var(--boris-label-secondary)] hover:text-white/70"
          >
            Choose another microphone
          </button>
        </div>
      </div>
    );
  }

  if (hint === "ignored_voice") {
    return (
      <div role="status" className="settings-group rounded-[12px] px-4 py-3.5">
        <p className="flex items-center gap-2 text-[14px] font-medium text-white/85">
          <UserX className="size-4 text-white/55" strokeWidth={2} />
          Heard &ldquo;Boris&rdquo;, but it didn&apos;t sound like your taught voice
        </p>
        <p className="mt-1 text-[12px] leading-snug text-[var(--boris-label-tertiary)]">
          Teach your voice again from where you usually sit, or turn off
          &ldquo;Ignore speakers and TV&rdquo; so anyone can wake Boris.
        </p>
        <div className="mt-3 flex flex-wrap gap-2">
          <button
            type="button"
            onClick={onTeachVoice}
            className="h-8 rounded-full bg-white/10 px-3.5 text-[13px] text-white hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
          >
            Teach my voice again
          </button>
          <button
            type="button"
            onClick={onOpenSpeechSettings}
            className="h-8 rounded-full px-3 text-[13px] text-[var(--boris-label-secondary)] hover:text-white/70"
          >
            Speech settings
          </button>
        </div>
      </div>
    );
  }

  return (
    <div role="status" className="settings-group rounded-[12px] px-4 py-3">
      <p className="flex items-center gap-2 text-[13px] text-white/70">
        <Volume2 className="size-4 text-white/45" strokeWidth={2} />
        Ignored &ldquo;Boris&rdquo; from a speaker or TV.
        <button
          type="button"
          onClick={onOpenSpeechSettings}
          className="ml-1 text-[12px] text-[var(--boris-label-secondary)] underline-offset-2 hover:text-white/70 hover:underline"
        >
          Change
        </button>
      </p>
    </div>
  );
}
