import { ChoiceList, SettingsGroup, SettingsRow, Toggle } from "@/components/settings";
import type { AppSettings } from "@/bridge";
import { CAPABILITY_OPTIONS } from "../mainWindowShared";

const TOOL_CHOICES = CAPABILITY_OPTIONS.map((o) => ({
  id: o.id as string,
  label: o.label,
  detail: o.footer,
}));

/** What Boris remembers and what he is allowed to touch. */
export function PrivacySettings({
  settings,
  engineOn,
  onPatch,
}: {
  settings: AppSettings;
  engineOn: boolean;
  onPatch: (p: Partial<AppSettings>) => void;
}) {
  const preset = TOOL_CHOICES.some((o) => o.id === settings.capability_preset)
    ? settings.capability_preset
    : "full";
  const applyNote = engineOn
    ? " Changes apply after the current conversation."
    : "";

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup
        title="Memory"
        footer={`Memories stay on this computer.${applyNote}`}
      >
        <SettingsRow
          label="Remember things about me"
          subtitle="Boris keeps notes about your preferences between conversations"
          last
        >
          <Toggle
            checked={settings.long_term_memory}
            onChange={(v) => onPatch({ long_term_memory: v })}
            aria-label="Remember things about me"
          />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="What Boris can do"
        footer={`Whatever you pick, Boris asks you first before anything risky, like running commands or opening links.${applyNote}`}
      >
        <ChoiceList
          aria-label="What Boris can do"
          options={TOOL_CHOICES}
          value={preset}
          onChange={(id) => onPatch({ capability_preset: id })}
        />
      </SettingsGroup>

      <SettingsGroup title="Asking first">
        <SettingsRow
          label="Save notes and files without asking"
          subtitle="Only inside Boris's own workspace folder. Turn off to approve every save."
          last
        >
          <Toggle
            checked={settings.trusted_auto_moderate}
            onChange={(v) => onPatch({ trusted_auto_moderate: v })}
            aria-label="Save notes and files without asking"
          />
        </SettingsRow>
      </SettingsGroup>
    </div>
  );
}
