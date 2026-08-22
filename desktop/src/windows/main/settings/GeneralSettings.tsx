import { LoaderCircle, RefreshCw } from "lucide-react";
import { SettingsGroup, SettingsRow, Toggle } from "@/components/settings";
import type { AppSettings } from "@/bridge";
import type { AvailableUpdate, UpdateProgress } from "@/lib/updater";
import {
  CAPABILITY_OPTIONS,
  type CapabilityOption,
  selectCompactClass,
  type UpdateUiState,
} from "../mainWindowShared";

export function GeneralSettings({
  settings,
  locked,
  capability,
  appVer,
  updateUi,
  availableUpdate,
  updateProgress,
  updateError,
  onPatch,
  onCheckUpdate,
  onInstallUpdate,
}: {
  settings: AppSettings;
  locked: boolean;
  capability: CapabilityOption;
  appVer: string | null;
  updateUi: UpdateUiState;
  availableUpdate: AvailableUpdate | null;
  updateProgress: UpdateProgress | null;
  updateError: string | null;
  onPatch: (p: Partial<AppSettings>) => void;
  onCheckUpdate: () => void;
  onInstallUpdate: () => void;
}) {
  const checking = updateUi === "checking";
  const downloading = updateUi === "downloading";
  const hasUpdate = updateUi === "available" || downloading;
  const versionLabel = appVer ? `v${appVer}` : "—";
  const statusSubtitle = checking
    ? "Checking…"
    : downloading
      ? "Downloading update…"
      : hasUpdate && availableUpdate
        ? `v${availableUpdate.version} ready`
        : updateUi === "up_to_date"
          ? settings.update_channel === "beta"
            ? "You're on the latest beta"
            : "You're on the latest version"
          : updateUi === "error"
            ? "Check failed"
            : "Check for a newer installer";
  const pct =
    updateProgress?.contentLength != null && updateProgress.contentLength > 0
      ? Math.min(
          100,
          Math.round(
            (updateProgress.downloaded / updateProgress.contentLength) * 100,
          ),
        )
      : null;

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup
        title="General"
        footer={
          locked
            ? "Some changes apply after Boris is stopped."
            : capability.footer
        }
      >
        <SettingsRow
          label="Start with Windows"
          subtitle="At sign-in, stay in the tray and turn Boris on"
        >
          <Toggle
            checked={settings.start_with_windows}
            onChange={(v) => onPatch({ start_with_windows: v })}
            aria-label="Start with Windows"
          />
        </SettingsRow>
        <SettingsRow label="Start Boris when the app opens">
          <Toggle
            checked={settings.start_engine_on_launch}
            onChange={(v) => onPatch({ start_engine_on_launch: v })}
            aria-label="Start Boris when the app opens"
          />
        </SettingsRow>
        <SettingsRow
          label="Long-term memory"
          subtitle="Remember notes across sessions"
        >
          <Toggle
            checked={settings.long_term_memory}
            disabled={locked}
            onChange={(v) => onPatch({ long_term_memory: v })}
            aria-label="Long-term memory"
          />
        </SettingsRow>
        <SettingsRow label="Tool access" labelFor="capability-preset" last>
          <select
            id="capability-preset"
            className={selectCompactClass}
            value={
              CAPABILITY_OPTIONS.some(
                (p) => p.id === settings.capability_preset,
              )
                ? settings.capability_preset
                : "full"
            }
            disabled={locked}
            onChange={(e) => onPatch({ capability_preset: e.target.value })}
          >
            {CAPABILITY_OPTIONS.map((p) => (
              <option
                key={p.id}
                value={p.id}
                className="bg-[#1c1c1e] text-white"
              >
                {p.label}
              </option>
            ))}
          </select>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="Updates"
        footer="Updates are signed and downloaded from GitHub Releases. Beta uses a separate pre-release feed. The app restarts after install."
      >
        <SettingsRow
          label="Channel"
          subtitle="Beta gets new builds first. Switch back to Stable anytime."
          labelFor="update-channel"
        >
          <select
            id="update-channel"
            className={selectCompactClass}
            value={settings.update_channel === "beta" ? "beta" : "stable"}
            disabled={downloading}
            onChange={(e) =>
              onPatch({
                update_channel: e.target.value === "beta" ? "beta" : "stable",
              })
            }
          >
            <option value="stable" className="bg-[#1c1c1e] text-white">
              Stable
            </option>
            <option value="beta" className="bg-[#1c1c1e] text-white">
              Beta
            </option>
          </select>
        </SettingsRow>
        <SettingsRow
          label="Version"
          subtitle={statusSubtitle}
          last={!hasUpdate && updateUi !== "error"}
        >
          <div className="flex items-center gap-2">
            <span className="rounded-lg bg-white/[0.06] px-3 py-2 text-[13px] text-white/65">
              {versionLabel}
            </span>
            {hasUpdate ? (
              <button
                type="button"
                disabled={downloading}
                onClick={onInstallUpdate}
                className="min-h-9 rounded-lg bg-white px-3 text-[13px] font-medium text-black hover:bg-white/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40"
              >
                {downloading ? "Installing…" : "Install"}
              </button>
            ) : (
              <button
                type="button"
                disabled={checking}
                onClick={onCheckUpdate}
                className="inline-flex min-h-9 items-center gap-1.5 rounded-lg px-3 text-[13px] font-medium text-white/75 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40"
              >
                {checking ? (
                  <LoaderCircle className="size-3.5 animate-spin" />
                ) : (
                  <RefreshCw className="size-3.5" />
                )}
                {checking ? "Checking…" : "Check"}
              </button>
            )}
          </div>
        </SettingsRow>
        {downloading ? (
          <div className="border-t border-white/[0.06] px-4 py-3" role="status">
            <div
              className="main-progress-track"
              role="progressbar"
              aria-label="App update download"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={pct ?? undefined}
            >
              <div
                className="main-progress-fill"
                style={{ width: `${pct ?? 4}%` }}
              />
            </div>
            <p className="mt-1.5 text-[12px] text-white/50">
              {pct != null ? `${pct}%` : "Downloading…"}
            </p>
          </div>
        ) : null}
        {updateUi === "error" && updateError ? (
          <div className="border-t border-white/[0.06] px-4 py-3">
            <p
              className="text-[12px] leading-snug text-red-300/90"
              role="alert"
            >
              {updateError}
            </p>
          </div>
        ) : null}
        {hasUpdate && availableUpdate?.body ? (
          <div className="border-t border-white/[0.06] px-4 py-3">
            <p className="whitespace-pre-wrap text-[12px] leading-snug text-white/45">
              {availableUpdate.body}
            </p>
          </div>
        ) : null}
      </SettingsGroup>
    </div>
  );
}
