import { useEffect, useState } from "react";
import { Input } from "@/components/ui/input";
import { SettingsRow } from "@/components/settings";
import { MODEL_PRESETS, PROVIDER_PRESETS } from "@/bridge";
import { cn } from "@/lib/utils";
import { fieldInputClass, selectCompactClass } from "../mainWindowShared";

export function ModelField({
  label,
  subtitle,
  value,
  onChange,
  disabled,
  allowEmpty,
  last,
}: {
  label: string;
  subtitle?: string;
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
  allowEmpty?: boolean;
  last?: boolean;
}) {
  const match = MODEL_PRESETS.some((p) => p.id === value);
  const isCustomValue = Boolean(value) && !match;
  // When allowEmpty, empty means "Same as model" — track intentional Custom…
  // so the text field can open before the user types an id.
  const [forceCustom, setForceCustom] = useState(false);
  const [customDraft, setCustomDraft] = useState(value);
  const fieldId = `model-${label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;

  useEffect(() => {
    if (match) setForceCustom(false);
    else if (isCustomValue) setForceCustom(true);
    setCustomDraft(value);
  }, [match, isCustomValue, value]);

  const inCustomMode = forceCustom || isCustomValue || (!allowEmpty && !match);
  const selectValue = inCustomMode
    ? "__custom__"
    : !value && allowEmpty
      ? ""
      : match
        ? value
        : "__custom__";
  const showCustom = selectValue === "__custom__";

  return (
    <>
      <SettingsRow
        label={label}
        labelFor={fieldId}
        subtitle={subtitle}
        last={last && !showCustom}
      >
        <select
          id={fieldId}
          className={selectCompactClass}
          disabled={disabled}
          value={selectValue}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "__custom__") {
              setForceCustom(true);
              setCustomDraft(isCustomValue ? value : "");
              return;
            }
            setForceCustom(false);
            onChange(v);
          }}
        >
          {allowEmpty ? (
            <option value="" className="bg-[#1c1c1e] text-white">
              Same as model
            </option>
          ) : null}
          {MODEL_PRESETS.map((p) => (
            <option key={p.id} value={p.id} className="bg-[#1c1c1e] text-white">
              {p.label}
            </option>
          ))}
          <option value="__custom__" className="bg-[#1c1c1e] text-white">
            Custom…
          </option>
        </select>
      </SettingsRow>
      {showCustom ? (
        <div
          className={cn(
            "border-b border-white/[0.06] px-4 pb-3",
            last && "border-b-0",
          )}
        >
          <Input
            id={`${fieldId}-custom`}
            aria-label={`Custom ${label.toLowerCase()} identifier`}
            value={customDraft}
            disabled={disabled}
            placeholder="provider/model-id"
            onChange={(e) => {
              setForceCustom(true);
              setCustomDraft(e.target.value);
            }}
            onBlur={() => {
              const next = customDraft.trim();
              if (/^\S+\/\S+$/.test(next)) onChange(next);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const next = customDraft.trim();
                if (/^\S+\/\S+$/.test(next)) onChange(next);
              }
            }}
            className={cn(fieldInputClass, "h-9 w-full font-mono text-[13px]")}
          />
        </div>
      ) : null}
    </>
  );
}

export function ProviderField({
  label,
  value,
  onChange,
  disabled,
  last,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
  last?: boolean;
}) {
  const first =
    value
      .split(/[,\s]+/)
      .map((s) => s.trim())
      .filter(Boolean)[0] ?? "";
  const match = PROVIDER_PRESETS.some((p) => p.id === first && p.id !== "");
  const isCustomValue = Boolean(value.trim()) && !match;
  // Empty is "Auto" in the preset list — same Custom… sticky mode as ModelField.
  const [forceCustom, setForceCustom] = useState(false);
  const [customDraft, setCustomDraft] = useState(value);
  const fieldId = `provider-${label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;

  useEffect(() => {
    if (match) setForceCustom(false);
    else if (isCustomValue) setForceCustom(true);
    setCustomDraft(value);
  }, [match, isCustomValue, value]);

  const inCustomMode = forceCustom || isCustomValue;
  const selectValue = inCustomMode ? "__custom__" : match ? first : "";
  const showCustom = selectValue === "__custom__";

  return (
    <>
      <SettingsRow label={label} labelFor={fieldId} last={last && !showCustom}>
        <select
          id={fieldId}
          className={selectCompactClass}
          disabled={disabled}
          value={selectValue}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "__custom__") {
              setForceCustom(true);
              setCustomDraft(isCustomValue ? value : "");
              return;
            }
            setForceCustom(false);
            onChange(v);
          }}
        >
          {PROVIDER_PRESETS.map((p) => (
            <option
              key={p.id || "auto"}
              value={p.id}
              className="bg-[#1c1c1e] text-white"
            >
              {p.label}
            </option>
          ))}
          <option value="__custom__" className="bg-[#1c1c1e] text-white">
            Custom…
          </option>
        </select>
      </SettingsRow>
      {showCustom ? (
        <div className="border-b border-white/[0.06] px-4 pb-3">
          <Input
            id={`${fieldId}-custom`}
            aria-label={`Custom ${label.toLowerCase()} value`}
            value={customDraft}
            disabled={disabled}
            placeholder="coreweave,baseten"
            onChange={(e) => {
              setForceCustom(true);
              setCustomDraft(e.target.value);
            }}
            onBlur={() => {
              const next = customDraft.trim();
              if (next) onChange(next);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && customDraft.trim())
                onChange(customDraft.trim());
            }}
            className={cn(fieldInputClass, "h-9 w-full font-mono text-[13px]")}
          />
        </div>
      ) : null}
    </>
  );
}
