import {
  SegmentedControl,
  SettingsField,
  SettingsGroup,
  SettingsRow,
  Toggle,
} from "@/components/settings";
import type { AppSettings } from "@/bridge";

const POSITION_OPTIONS = [
  { id: "top_left", label: "Left" },
  { id: "top_center", label: "Center" },
  { id: "top_right", label: "Right" },
] as const satisfies readonly { id: AppSettings["overlay_position"]; label: string }[];

const CAPTION_OPTIONS = [
  { id: "full", label: "Everything" },
  { id: "assistant", label: "Boris only" },
  { id: "hidden", label: "Off" },
] as const satisfies readonly { id: AppSettings["overlay_caption_mode"]; label: string }[];

const CAPTION_HINTS: Record<AppSettings["overlay_caption_mode"], string> = {
  full: "Shows what you said and what Boris says.",
  assistant: "Shows only Boris's replies. Handy while screen sharing.",
  hidden: "Shows status only. Nothing you or Boris say appears on screen.",
};

const SUBMIT_OPTIONS = [
  { id: "enter", label: "Enter" },
  { id: "ctrl_enter", label: "Ctrl + Enter" },
] as const satisfies readonly { id: AppSettings["typed_input_submit"]; label: string }[];

/** The floating bubble that appears when Boris wakes. */
export function OverlaySettings({
  settings,
  onPatch,
}: {
  settings: AppSettings;
  onPatch: (p: Partial<AppSettings>) => void;
}) {
  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup
        title="Pop-up"
        footer="The pop-up appears at the top of the screen you're using."
      >
        <SettingsRow
          label="Show the pop-up when Boris wakes"
          subtitle="So you can see he's listening"
        >
          <Toggle
            checked={settings.show_overlay_on_wake}
            onChange={(v) => onPatch({ show_overlay_on_wake: v })}
            aria-label="Show the pop-up when Boris wakes"
          />
        </SettingsRow>
        <SettingsField label="Position">
          <SegmentedControl
            aria-label="Pop-up position"
            options={POSITION_OPTIONS}
            value={settings.overlay_position}
            onChange={(id) => onPatch({ overlay_position: id })}
          />
        </SettingsField>
        <SettingsField
          label={`Size · ${settings.overlay_scale_percent}%`}
          labelFor="overlay-scale"
          last
        >
          <input
            id="overlay-scale"
            type="range"
            min={75}
            max={125}
            step={5}
            value={settings.overlay_scale_percent}
            onChange={(e) =>
              onPatch({ overlay_scale_percent: Number(e.target.value) })
            }
            className="h-9 w-full accent-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
          />
        </SettingsField>
      </SettingsGroup>

      <SettingsGroup
        title="Captions"
        footer={CAPTION_HINTS[settings.overlay_caption_mode]}
      >
        <SettingsField label="Show on screen" last>
          <SegmentedControl
            aria-label="Captions"
            options={CAPTION_OPTIONS}
            value={settings.overlay_caption_mode}
            onChange={(id) => onPatch({ overlay_caption_mode: id })}
          />
        </SettingsField>
      </SettingsGroup>

      <SettingsGroup
        title="Typing"
        footer="When Boris asks you to type something. Esc cancels."
      >
        <SettingsField label="Send with" last>
          <SegmentedControl
            aria-label="Send typed text with"
            options={SUBMIT_OPTIONS}
            value={settings.typed_input_submit}
            onChange={(id) => onPatch({ typed_input_submit: id })}
          />
        </SettingsField>
      </SettingsGroup>
    </div>
  );
}
