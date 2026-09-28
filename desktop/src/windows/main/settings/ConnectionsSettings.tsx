import type { ReactNode } from "react";
import { ExternalLink, Eye, EyeOff } from "lucide-react";
import { Input } from "@/components/ui/input";
import { SettingsField, SettingsGroup } from "@/components/settings";
import type { AppSettings } from "@/bridge";
import { cn } from "@/lib/utils";
import { fieldInputClass } from "../mainWindowShared";
import { ModelField } from "./ModelFields";

export function ConnectionsSettings({
  settings,
  locked,
  showOpenRouterKey,
  showExaKey,
  onToggleOpenRouter,
  onToggleExa,
  onOpenExternal,
  onPatch,
}: {
  settings: AppSettings;
  locked: boolean;
  showOpenRouterKey: boolean;
  showExaKey: boolean;
  onToggleOpenRouter: () => void;
  onToggleExa: () => void;
  onOpenExternal: (url: string) => void;
  onPatch: (p: Partial<AppSettings>) => void;
}) {
  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup
        title="API Keys"
        footer={
          locked
            ? "Stop Boris to change API keys."
            : "Keys stay in ~/.boris/auth.json on this computer."
        }
      >
        <SettingsField
          label="OpenRouter"
          subtitle="Required for chat"
          labelFor="openrouter-key"
        >
          <SecretField
            id="openrouter-key"
            shown={showOpenRouterKey}
            onToggle={onToggleOpenRouter}
            value={settings.openrouter_api_key}
            disabled={locked}
            placeholder="sk-or-v1-…"
            onChange={(value) => onPatch({ openrouter_api_key: value })}
          />
          <HelpLink
            onClick={() => onOpenExternal("https://openrouter.ai/keys")}
          >
            Get an OpenRouter key
          </HelpLink>
        </SettingsField>
        <SettingsField
          label="Exa"
          subtitle="Optional upgrade. Web search works without this."
          labelFor="exa-key"
          last
        >
          <SecretField
            id="exa-key"
            shown={showExaKey}
            onToggle={onToggleExa}
            value={settings.exa_api_key}
            disabled={locked}
            placeholder="Exa API key"
            onChange={(value) => onPatch({ exa_api_key: value })}
          />
          <HelpLink onClick={() => onOpenExternal("https://dashboard.exa.ai")}>
            Open Exa dashboard
          </HelpLink>
        </SettingsField>
      </SettingsGroup>
      <SettingsGroup
        title="Chat Models"
        footer={
          locked
            ? "Stop Boris to change models."
            : "The fast model handles simpler requests."
        }
      >
        <ModelField
          label="Primary model"
          value={settings.openrouter_model}
          disabled={locked}
          onChange={(v) => onPatch({ openrouter_model: v })}
        />
        <ModelField
          label="Fast model"
          subtitle="Uses the primary model when unset"
          value={settings.openrouter_fast_model}
          disabled={locked}
          onChange={(v) => onPatch({ openrouter_fast_model: v })}
          allowEmpty
          last
        />
      </SettingsGroup>
    </div>
  );
}

function SecretField({
  id,
  shown,
  onToggle,
  value,
  disabled,
  placeholder,
  onChange,
}: {
  id: string;
  shown: boolean;
  onToggle: () => void;
  value: string;
  disabled: boolean;
  placeholder: string;
  onChange: (value: string) => void;
}) {
  return (
    <div className="relative">
      <Input
        id={id}
        type={shown ? "text" : "password"}
        placeholder={placeholder}
        value={value}
        disabled={disabled}
        autoComplete="off"
        spellCheck={false}
        onChange={(e) => onChange(e.target.value)}
        className={cn(fieldInputClass, "pr-11")}
      />
      <button
        type="button"
        disabled={disabled}
        onClick={onToggle}
        aria-label={shown ? "Hide API key" : "Show API key"}
        className="absolute right-1 top-1 inline-flex size-8 items-center justify-center rounded-md text-white/45 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40"
      >
        {shown ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
      </button>
    </div>
  );
}

function HelpLink({
  onClick,
  children,
}: {
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="inline-flex min-h-9 w-fit items-center gap-1.5 rounded-md px-1 text-[12px] text-sky-300/75 hover:text-sky-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
    >
      {children}
      <ExternalLink className="size-3" />
    </button>
  );
}
