import {
  SettingsField,
  SettingsGroup,
  SettingsRow,
  Toggle,
} from "@/components/settings";
import type { AppSettings } from "@/bridge";
import { selectCompactClass } from "../mainWindowShared";

export function OverlaySettings({
  settings,
  onPatch,
}: {
  settings: AppSettings;
  onPatch: (p: Partial<AppSettings>) => void;
}) {
  return (
    <SettingsGroup
      title="Overlay"
      footer="Caption privacy controls spoken text during screen sharing. Hidden keeps the overlay to status only."
    >
      <SettingsRow label="Show overlay when Boris wakes">
        <Toggle
          checked={settings.show_overlay_on_wake}
          onChange={(v) => onPatch({ show_overlay_on_wake: v })}
          aria-label="Show overlay when Boris wakes"
        />
      </SettingsRow>
      <SettingsRow
        label="Captions"
        subtitle="Choose which spoken text appears"
        labelFor="overlay-captions"
      >
        <select
          id="overlay-captions"
          className={selectCompactClass}
          value={settings.overlay_caption_mode}
          onChange={(e) =>
            onPatch({
              overlay_caption_mode: e.target
                .value as AppSettings["overlay_caption_mode"],
            })
          }
        >
          <option value="full">You and Boris</option>
          <option value="assistant">Boris only</option>
          <option value="hidden">Hidden</option>
        </select>
      </SettingsRow>
      <SettingsRow
        label="Send typed input"
        subtitle="Overlay and Home. Esc cancels. Paste with Ctrl+V."
        labelFor="typed-input-submit"
      >
        <select
          id="typed-input-submit"
          className={selectCompactClass}
          value={settings.typed_input_submit}
          onChange={(e) =>
            onPatch({
              typed_input_submit: e.target
                .value as AppSettings["typed_input_submit"],
            })
          }
        >
          <option value="enter">Enter</option>
          <option value="ctrl_enter">Ctrl+Enter</option>
        </select>
      </SettingsRow>
      <SettingsRow label="Position" labelFor="overlay-position">
        <select
          id="overlay-position"
          className={selectCompactClass}
          value={settings.overlay_position}
          onChange={(e) =>
            onPatch({
              overlay_position: e.target
                .value as AppSettings["overlay_position"],
            })
          }
        >
          <option value="top_center">Top center</option>
          <option value="top_left">Top left</option>
          <option value="top_right">Top right</option>
        </select>
      </SettingsRow>
      <SettingsField
        label={`Scale · ${settings.overlay_scale_percent}%`}
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
  );
}
