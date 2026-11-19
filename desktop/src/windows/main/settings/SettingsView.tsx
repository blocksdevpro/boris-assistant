import { useState } from "react";
import {
  AlertCircle,
  Check,
  Code2,
  KeyRound,
  LoaderCircle,
  Mic,
  PanelTop,
  Settings as SettingsIcon,
  ShieldCheck,
} from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  AppSettings,
  DeviceDto,
  DownloadProgress,
} from "@/bridge";
import type { AvailableUpdate, UpdateProgress } from "@/lib/updater";
import { Toggle } from "@/components/settings";
import { cn } from "@/lib/utils";
import {
  type ModelCheckState,
  type SaveState,
  type SettingsCategory,
  type UpdateUiState,
} from "../mainWindowShared";
import { AdvancedSettings } from "./AdvancedSettings";
import { ConnectionsSettings } from "./ConnectionsSettings";
import { GeneralSettings } from "./GeneralSettings";
import { OverlaySettings } from "./OverlaySettings";
import { PrivacySettings } from "./PrivacySettings";
import { SpeechSettings } from "./SpeechSettings";

export function SettingsView({
  settings,
  engineOn,
  busy,
  inputs,
  outputs,
  selectedInput,
  selectedOutput,
  modelsReady,
  modelCheckState,
  installing,
  installProgress,
  logPath,
  error,
  saveState,
  category,
  appVer,
  updateUi,
  availableUpdate,
  updateProgress,
  updateError,
  onCategoryChange,
  onPatch,
  onInputChange,
  onOutputChange,
  onRefreshDevices,
  onRefreshModels,
  onInstall,
  onCheckUpdate,
  onInstallUpdate,
  onTeachVoice,
  onOpenMonitor,
  onReplayOnboarding,
}: {
  settings: AppSettings;
  engineOn: boolean;
  busy: boolean;
  inputs: DeviceDto[];
  outputs: DeviceDto[];
  selectedInput: string;
  selectedOutput: string;
  modelsReady: boolean;
  modelCheckState: ModelCheckState;
  installing: boolean;
  installProgress: DownloadProgress | null;
  logPath: string;
  error: string | null;
  saveState: SaveState;
  category: SettingsCategory;
  appVer: string | null;
  updateUi: UpdateUiState;
  availableUpdate: AvailableUpdate | null;
  updateProgress: UpdateProgress | null;
  updateError: string | null;
  onCategoryChange: (category: SettingsCategory) => void;
  onPatch: (p: Partial<AppSettings>) => void;
  onInputChange: (id: string) => void;
  onOutputChange: (id: string) => void;
  onRefreshDevices: () => void;
  onRefreshModels: () => void;
  onInstall: () => void;
  onTeachVoice: () => void;
  onCheckUpdate: () => void;
  onInstallUpdate: () => void;
  onOpenMonitor: () => void;
  onReplayOnboarding: () => void;
}) {
  const locked = engineOn;
  const reduceMotion = useReducedMotion();
  const [showOpenRouterKey, setShowOpenRouterKey] = useState(false);
  const [showExaKey, setShowExaKey] = useState(false);
  const advanced = settings.show_advanced_settings;
  const allCategories: {
    id: SettingsCategory;
    label: string;
    description: string;
    icon: typeof SettingsIcon;
  }[] = [
    {
      id: "general",
      label: "General",
      description: "Startup, speed, and updates",
      icon: SettingsIcon,
    },
    {
      id: "speech",
      label: "Voice & Audio",
      description: "Your microphone, speakers, and how Boris listens",
      icon: Mic,
    },
    {
      id: "overlay",
      label: "Pop-up",
      description: "The bubble that appears when you talk to Boris",
      icon: PanelTop,
    },
    {
      id: "privacy",
      label: "Privacy",
      description: "What Boris remembers and what he's allowed to do",
      icon: ShieldCheck,
    },
    {
      id: "connections",
      label: "Account",
      description: advanced
        ? "API keys and chat models"
        : "Your OpenRouter key",
      icon: KeyRound,
    },
    {
      id: "advanced",
      label: "Developer",
      description: "Routing, diagnostics, and testing tools",
      icon: Code2,
    },
  ];
  const categories = allCategories.filter(
    (item) => advanced || item.id !== "advanced",
  );
  const activeCategory =
    categories.find((item) => item.id === category) ?? categories[0]!;
  const openExternal = (url: string) => {
    void openUrl(url).catch(() =>
      window.open(url, "_blank", "noopener,noreferrer"),
    );
  };

  return (
    <div className="settings-view mx-auto grid w-full max-w-5xl gap-6 px-5 py-5 pb-12 sm:grid-cols-[11rem_minmax(0,1fr)] sm:px-6 sm:py-6">
      <aside className="settings-sidebar sm:sticky sm:top-5 sm:self-start">
        <div className="mb-4 hidden px-2 sm:block">
          <p className="text-[11px] font-semibold uppercase tracking-[0.12em] text-[var(--boris-label-tertiary)]">
            Preferences
          </p>
        </div>
        <div
          className="settings-sidebar__list flex gap-1 overflow-x-auto rounded-[16px] border border-white/[0.05] bg-white/[0.025] p-1.5 shadow-[0_12px_35px_rgba(0,0,0,0.08)] sm:flex-col sm:overflow-visible"
          role="tablist"
          aria-label="Settings categories"
          aria-orientation="vertical"
          onKeyDown={(event) => {
            if (
              !["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight"].includes(
                event.key,
              )
            )
              return;
            event.preventDefault();
            const current = categories.findIndex((item) => item.id === category);
            const forwards =
              event.key === "ArrowDown" || event.key === "ArrowRight";
            const next =
              (current + (forwards ? 1 : -1) + categories.length) %
              categories.length;
            const nextCategory = categories[next]!;
            onCategoryChange(nextCategory.id);
            document
              .getElementById(`settings-tab-${nextCategory.id}`)
              ?.focus();
          }}
        >
          {categories.map((item) => {
            const Icon = item.icon;
            const active = category === item.id;
            return (
              <button
                key={item.id}
                id={`settings-tab-${item.id}`}
                type="button"
                role="tab"
                aria-controls="settings-panel"
                aria-selected={active}
                tabIndex={active ? 0 : -1}
                onClick={() => onCategoryChange(item.id)}
                className={cn(
                  "settings-sidebar__item relative flex min-h-10 shrink-0 items-center gap-2.5 overflow-hidden rounded-[11px] px-3 text-left text-[13px] font-medium",
                  "transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-white/20",
                  active
                    ? "text-white"
                    : "text-[var(--boris-label-secondary)] hover:bg-white/[0.045] hover:text-white/78",
                )}
              >
                {active ? (
                  <motion.span
                    layoutId="settings-active-category"
                    className="absolute inset-0 rounded-[11px] border border-white/[0.06] bg-white/[0.09] shadow-[0_1px_0_rgba(255,255,255,0.035)_inset]"
                    transition={
                      reduceMotion
                        ? { duration: 0 }
                        : { type: "spring", stiffness: 420, damping: 34 }
                    }
                    aria-hidden
                  />
                ) : null}
                <Icon
                  className={cn(
                    "relative size-3.5 shrink-0",
                    active ? "text-white/82" : "text-white/34",
                  )}
                  strokeWidth={1.8}
                />
                <span className="relative">{item.label}</span>
              </button>
            );
          })}
        </div>

        <div
          role="status"
          aria-live="polite"
          className="settings-save-state mt-3 flex min-h-7 items-center gap-1.5 px-2 text-[11px] text-[var(--boris-label-tertiary)]"
        >
          {saveState === "saving" ? (
            <>
              <LoaderCircle className="size-3 animate-spin" /> Saving changes…
            </>
          ) : null}
          {saveState === "saved" ? (
            <>
              <Check className="size-3 text-emerald-300/80" /> Changes saved
            </>
          ) : null}
          {saveState === "error" ? (
            <>
              <AlertCircle className="size-3 text-red-300/80" /> Save failed
            </>
          ) : null}
        </div>

        <div className="settings-advanced-switch mt-2 hidden rounded-[14px] border border-white/[0.05] bg-white/[0.02] px-3 py-3 sm:block">
          <div className="flex items-center justify-between gap-2">
            <label
              htmlFor="show-advanced"
              className="text-[12px] font-medium leading-tight text-white/70"
            >
              Advanced options
            </label>
            <Toggle
              id="show-advanced"
              checked={advanced}
              onChange={(v) => {
                onPatch({ show_advanced_settings: v });
                if (!v && category === "advanced") onCategoryChange("general");
              }}
              aria-label="Show advanced options"
            />
          </div>
          <p className="mt-1.5 text-[11px] leading-snug text-[var(--boris-label-tertiary)]">
            Extra controls for tinkerers. Most people never need them.
          </p>
        </div>
      </aside>

      <div className="settings-content min-w-0">
        <header className="settings-content__header mb-5 flex min-h-14 items-start justify-between gap-4 px-1">
          <div>
            <h1 className="text-[24px] font-semibold tracking-[-0.035em] text-white/94">
              {activeCategory.label}
            </h1>
            <p className="mt-1 text-[12px] leading-relaxed text-[var(--boris-label-secondary)]">
              {activeCategory.description}
            </p>
          </div>
        </header>

        {error ? (
          <div
            role="alert"
            className="mb-5 flex items-start gap-2.5 rounded-[14px] border border-red-400/10 bg-red-500/[0.075] px-3.5 py-3 text-[12px] leading-relaxed text-red-200/82"
          >
            <AlertCircle className="mt-0.5 size-4 shrink-0 text-red-300/75" />
            <span>{error}</span>
          </div>
        ) : null}

        <AnimatePresence mode="wait" initial={false}>
          <motion.div
            key={category}
            id="settings-panel"
            role="tabpanel"
            aria-labelledby={`settings-tab-${category}`}
            initial={reduceMotion ? false : { opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: reduceMotion ? 1 : 0 }}
            transition={
              reduceMotion
                ? { duration: 0 }
                : { duration: 0.14, ease: [0.22, 1, 0.36, 1] }
            }
          >
            {category === "general" ? (
              <GeneralSettings
                settings={settings}
                advanced={advanced}
                engineOn={engineOn}
                appVer={appVer}
                updateUi={updateUi}
                availableUpdate={availableUpdate}
                updateProgress={updateProgress}
                updateError={updateError}
                onPatch={onPatch}
                onCheckUpdate={onCheckUpdate}
                onInstallUpdate={onInstallUpdate}
              />
            ) : null}
            {category === "speech" ? (
              <SpeechSettings
                settings={settings}
                advanced={advanced}
                busy={busy}
                inputs={inputs}
                outputs={outputs}
                selectedInput={selectedInput}
                selectedOutput={selectedOutput}
                modelsReady={modelsReady}
                modelCheckState={modelCheckState}
                installing={installing}
                progress={installProgress}
                onPatch={onPatch}
                onInputChange={onInputChange}
                onOutputChange={onOutputChange}
                onRefreshDevices={onRefreshDevices}
                onRefreshModels={onRefreshModels}
                onInstall={onInstall}
                onTeachVoice={onTeachVoice}
              />
            ) : null}
            {category === "overlay" ? (
              <OverlaySettings settings={settings} onPatch={onPatch} />
            ) : null}
            {category === "privacy" ? (
              <PrivacySettings
                settings={settings}
                engineOn={engineOn}
                onPatch={onPatch}
              />
            ) : null}
            {category === "connections" ? (
              <ConnectionsSettings
                settings={settings}
                advanced={advanced}
                locked={locked}
                showOpenRouterKey={showOpenRouterKey}
                showExaKey={showExaKey}
                onToggleOpenRouter={() => setShowOpenRouterKey((v) => !v)}
                onToggleExa={() => setShowExaKey((v) => !v)}
                onOpenExternal={openExternal}
                onPatch={onPatch}
              />
            ) : null}
            {category === "advanced" && advanced ? (
              <AdvancedSettings
                settings={settings}
                locked={locked}
                logPath={logPath}
                onPatch={onPatch}
                onOpenMonitor={onOpenMonitor}
                onReplayOnboarding={onReplayOnboarding}
              />
            ) : null}
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}
