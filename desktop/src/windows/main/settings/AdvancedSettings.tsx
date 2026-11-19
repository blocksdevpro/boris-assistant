import { Activity, Sparkles } from "lucide-react";
import { Input } from "@/components/ui/input";
import {
  SettingsField,
  SettingsGroup,
  SettingsRow,
  Toggle,
} from "@/components/settings";
import type { AppSettings } from "@/bridge";
import { cn } from "@/lib/utils";
import { fieldInputClass } from "../mainWindowShared";
import { ProviderField } from "./ModelFields";

const rowButtonClass =
  "inline-flex min-h-9 items-center gap-1.5 rounded-lg px-3 text-[13px] font-medium text-white/75 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25";

/** Developer page: only reachable with "Show advanced options" on. */
export function AdvancedSettings({
  settings,
  locked,
  logPath,
  onPatch,
  onOpenMonitor,
  onReplayOnboarding,
}: {
  settings: AppSettings;
  locked: boolean;
  logPath: string;
  onPatch: (p: Partial<AppSettings>) => void;
  onOpenMonitor: () => void;
  onReplayOnboarding: () => void;
}) {
  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup title="Tools">
        <SettingsRow
          label="Developer monitor"
          subtitle="Live phases, events, and turn timings"
        >
          <button type="button" onClick={onOpenMonitor} className={rowButtonClass}>
            <Activity className="size-3.5" /> Open
          </button>
        </SettingsRow>
        <SettingsRow
          label="Welcome tour"
          subtitle="Walk through first-run setup again"
          last
        >
          <button type="button" onClick={onReplayOnboarding} className={rowButtonClass}>
            <Sparkles className="size-3.5" /> Replay
          </button>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="Model routing"
        footer={
          locked
            ? "Stop Boris to change routing."
            : "Leave on Auto unless a specific inference provider is required."
        }
      >
        <ProviderField
          label="Primary provider"
          value={settings.openrouter_model_provider}
          disabled={locked}
          onChange={(v) => onPatch({ openrouter_model_provider: v })}
        />
        <ProviderField
          label="Fast provider"
          value={settings.openrouter_fast_provider}
          disabled={locked}
          onChange={(v) => onPatch({ openrouter_fast_provider: v })}
        />
        <SettingsRow
          label="Require selected providers"
          subtitle="Do not use another provider if these are unavailable"
          last
        >
          <Toggle
            checked={settings.openrouter_pin_provider}
            disabled={
              locked ||
              (!settings.openrouter_model_provider.trim() &&
                !settings.openrouter_fast_provider.trim())
            }
            onChange={(v) => onPatch({ openrouter_pin_provider: v })}
            aria-label="Require selected providers"
          />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="Diagnostics"
        footer="The log filter applies after restart."
      >
        <SettingsField
          label="Log filter"
          labelFor="logging-filter"
          last={!logPath}
        >
          <Input
            id="logging-filter"
            placeholder="info or boris=debug"
            value={settings.logging_filter}
            onChange={(e) => onPatch({ logging_filter: e.target.value })}
            className={cn(fieldInputClass, "font-mono text-[13px]")}
          />
        </SettingsField>
        {logPath ? (
          <div className="px-4 py-3">
            <p className="text-[13px] text-white/55">Log file</p>
            <p
              className="mt-1 select-text break-all font-mono text-[11px] leading-snug text-white/50"
              title={logPath}
            >
              {logPath.replace(/\\/g, "/")}
            </p>
          </div>
        ) : null}
      </SettingsGroup>
    </div>
  );
}
