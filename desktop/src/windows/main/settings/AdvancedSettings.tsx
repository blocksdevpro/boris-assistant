import { ChevronRight } from "lucide-react";
import { Input } from "@/components/ui/input";
import {
  SettingsField,
  SettingsGroup,
  SettingsRow,
  Toggle,
} from "@/components/settings";
import type { AppSettings } from "@/bridge";
import { cn } from "@/lib/utils";
import {
  fieldInputClass,
  selectCompactClass,
} from "../mainWindowShared";
import { ProviderField } from "./ModelFields";

export function AdvancedSettings({
  settings,
  locked,
  advancedOpen,
  logPath,
  onToggleAdvanced,
  onPatch,
}: {
  settings: AppSettings;
  locked: boolean;
  advancedOpen: boolean;
  logPath: string;
  onToggleAdvanced: () => void;
  onPatch: (p: Partial<AppSettings>) => void;
}) {
  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup title="Safety">
        <SettingsRow
          label="Trusted mode"
          subtitle="Automatically allow notes and writes inside the safe workspace"
          last
        >
          <Toggle
            checked={settings.trusted_auto_moderate}
            disabled={locked}
            onChange={(v) => onPatch({ trusted_auto_moderate: v })}
            aria-label="Trusted mode"
          />
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup
        title="Speech"
        footer="Balanced keeps voice models warm through active follow-ups, then releases them when idle. Low latency keeps them warm for the session."
      >
        <SettingsRow label="Model residency" labelFor="model-residency" last>
          <select
            id="model-residency"
            className={selectCompactClass}
            value={settings.model_residency}
            disabled={locked}
            onChange={(e) =>
              onPatch({
                model_residency: e.target
                  .value as AppSettings["model_residency"],
              })
            }
          >
            <option value="low_memory">Low memory</option>
            <option value="balanced">Balanced</option>
            <option value="low_latency">Low latency</option>
          </select>
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup
        title="Model Routing"
        footer="Optional. Leave this on Auto unless a specific inference provider is required."
      >
        <button
          type="button"
          onClick={onToggleAdvanced}
          aria-expanded={advancedOpen}
          aria-controls="provider-routing-controls"
          className="flex min-h-11 w-full items-center justify-between px-4 py-2.5 text-left text-[15px] text-white/80 hover:bg-white/[0.03] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-white/25"
        >
          <span>Provider routing</span>
          <ChevronRight
            className={cn(
              "size-4 text-white/35 transition-transform",
              advancedOpen && "rotate-90",
            )}
          />
        </button>
        {advancedOpen ? (
          <div id="provider-routing-controls">
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
          </div>
        ) : null}
      </SettingsGroup>
      <SettingsGroup
        title="Diagnostics"
        footer="The log filter applies after restart."
      >
        <SettingsField label="Log filter" labelFor="logging-filter">
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
              className="mt-1 break-all font-mono text-[11px] leading-snug text-white/50"
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
