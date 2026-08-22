import { useState } from "react";
import {
  AlertCircle,
  Check,
  Link2,
  LoaderCircle,
  Mic,
  Monitor,
  Settings as SettingsIcon,
  SlidersHorizontal,
} from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  AppSettings,
  DeviceDto,
  DownloadProgress,
} from "@/bridge";
import type { AvailableUpdate, UpdateProgress } from "@/lib/updater";
import { cn } from "@/lib/utils";
import {
  type CapabilityOption,
  type ModelCheckState,
  type SaveState,
  type SettingsCategory,
  type UpdateUiState,
} from "../mainWindowShared";
import { AdvancedSettings } from "./AdvancedSettings";
import { ConnectionsSettings } from "./ConnectionsSettings";
import { GeneralSettings } from "./GeneralSettings";
import { OverlaySettings } from "./OverlaySettings";
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
  advancedOpen,
  logPath,
  capability,
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
  onToggleAdvanced,
  onCheckUpdate,
  onInstallUpdate,
  onTeachVoice,
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
  advancedOpen: boolean;
  logPath: string;
  capability: CapabilityOption;
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
  onToggleAdvanced: () => void;
  onCheckUpdate: () => void;
  onInstallUpdate: () => void;
}) {
  const locked = engineOn;
  const reduceMotion = useReducedMotion();
  const [showOpenRouterKey, setShowOpenRouterKey] = useState(false);
  const [showExaKey, setShowExaKey] = useState(false);
  const categories: {
    id: SettingsCategory;
    label: string;
    description: string;
    icon: typeof SettingsIcon;
  }[] = [
    {
      id: "general",
      label: "General",
      description: "Startup, memory, access, and updates",
      icon: SettingsIcon,
    },
    {
      id: "overlay",
      label: "Overlay",
      description: "Appearance and caption privacy",
      icon: Monitor,
    },
    {
      id: "speech",
      label: "Speech",
      description: "Voice, models, and sound devices",
      icon: Mic,
    },
    {
      id: "connections",
      label: "Connections",
      description: "API keys and chat models",
      icon: Link2,
    },
    {
      id: "advanced",
      label: "Advanced",
      description: "Safety, routing, and diagnostics",
      icon: SlidersHorizontal,
    },
  ];
  const activeCategory =
    categories.find((item) => item.id === category) ?? categories[0]!;
  const openExternal = (url: string) => {
    void openUrl(url).catch(() =>
      window.open(url, "_blank", "noopener,noreferrer"),
    );
  };

  return (
    <div className="settings-view mx-auto grid w-full max-w-5xl gap-6 px-5 py-5 pb-12 sm:grid-cols-[11.5rem_minmax(0,1fr)] sm:px-6 sm:py-6">
      <aside className="settings-sidebar sm:sticky sm:top-5 sm:self-start">
        <div className="mb-4 hidden px-2 sm:block">
          <p className="text-[11px] font-semibold uppercase tracking-[0.12em] text-white/28">
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
                    : "text-white/48 hover:bg-white/[0.045] hover:text-white/78",
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
          className="settings-save-state mt-3 flex min-h-7 items-center gap-1.5 px-2 text-[11px] text-white/38"
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
      </aside>

      <div className="settings-content min-w-0">
        <header className="settings-content__header mb-5 flex min-h-14 items-start justify-between gap-4 px-1">
          <div>
            <h1 className="text-[24px] font-semibold tracking-[-0.035em] text-white/94">
              {activeCategory.label}
            </h1>
            <p className="mt-1 text-[12px] leading-relaxed text-white/36">
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
            initial={reduceMotion ? false : { opacity: 0, x: 8 }}
            animate={{ opacity: 1, x: 0 }}
            exit={reduceMotion ? { opacity: 1 } : { opacity: 0, x: -5 }}
            transition={
              reduceMotion
                ? { duration: 0 }
                : { duration: 0.22, ease: [0.22, 1, 0.36, 1] }
            }
          >
            {category === "general" ? (
              <GeneralSettings
                settings={settings}
                locked={locked}
                capability={capability}
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
            {category === "overlay" ? (
              <OverlaySettings settings={settings} onPatch={onPatch} />
            ) : null}
            {category === "speech" ? (
              <SpeechSettings
                settings={settings}
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
            {category === "connections" ? (
              <ConnectionsSettings
                settings={settings}
                locked={locked}
                showOpenRouterKey={showOpenRouterKey}
                showExaKey={showExaKey}
                onToggleOpenRouter={() => setShowOpenRouterKey((v) => !v)}
                onToggleExa={() => setShowExaKey((v) => !v)}
                onOpenExternal={openExternal}
                onPatch={onPatch}
              />
            ) : null}
            {category === "advanced" ? (
              <AdvancedSettings
                settings={settings}
                locked={locked}
                advancedOpen={advancedOpen}
                logPath={logPath}
                onToggleAdvanced={onToggleAdvanced}
                onPatch={onPatch}
              />
            ) : null}
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}
