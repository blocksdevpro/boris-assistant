import { useCallback, useEffect, useState } from "react";
import { Check, RefreshCw, Trash2 } from "lucide-react";
import { AlertDialog } from "radix-ui";
import { SettingsGroup, SettingsRow, Toggle } from "@/components/settings";
import {
  clearWakeProfile,
  wakeLivenessStatus,
  type AppSettings,
  type DeviceDto,
  type DownloadProgress,
  type LivenessStatus,
} from "@/bridge";
import {
  SYSTEM_DEFAULT_DEVICE,
  type ModelCheckState,
  selectDeviceClass,
  shortDeviceName,
} from "../mainWindowShared";

/**
 * Mic / speaker picker. "System default" follows Windows when the default
 * changes; a picked device that is unplugged stays selected (shown as not
 * connected) and Boris returns to it when it is plugged back in.
 */
function DevicePicker({
  id,
  devices,
  selected,
  busy,
  noun,
  onChange,
}: {
  id: string;
  devices: DeviceDto[];
  selected: string;
  busy: boolean;
  noun: string;
  onChange: (id: string) => void;
}) {
  const current = devices.find((d) => d.id === selected);
  const defaultDevice = devices.find((d) => d.is_default);
  const missing = selected !== SYSTEM_DEFAULT_DEVICE && !current;
  return (
    <select
      id={id}
      className={selectDeviceClass}
      value={selected}
      disabled={busy}
      title={
        current?.name ??
        (missing
          ? `Not connected — using the system default until it is back`
          : defaultDevice?.name)
      }
      onChange={(e) => onChange(e.target.value)}
    >
      <option value={SYSTEM_DEFAULT_DEVICE} className="bg-[#1c1c1e] text-white">
        System default
        {defaultDevice ? ` · ${shortDeviceName(defaultDevice.name)}` : ""}
      </option>
      {missing ? (
        <option value={selected} className="bg-[#1c1c1e] text-white">
          {`Chosen ${noun} · not connected`}
        </option>
      ) : null}
      {devices.map((d) => (
        <option key={d.id} value={d.id} className="bg-[#1c1c1e] text-white">
          {shortDeviceName(d.name)}
        </option>
      ))}
    </select>
  );
}

export function SpeechSettings({
  settings,
  advanced,
  busy,
  inputs,
  outputs,
  selectedInput,
  selectedOutput,
  modelsReady,
  modelCheckState,
  installing,
  progress,
  onPatch,
  onInputChange,
  onOutputChange,
  onRefreshDevices,
  onRefreshModels,
  onInstall,
  onTeachVoice,
}: {
  settings: AppSettings;
  advanced: boolean;
  busy: boolean;
  inputs: DeviceDto[];
  outputs: DeviceDto[];
  selectedInput: string;
  selectedOutput: string;
  modelsReady: boolean;
  modelCheckState: ModelCheckState;
  installing: boolean;
  progress: DownloadProgress | null;
  onPatch: (p: Partial<AppSettings>) => void;
  onInputChange: (id: string) => void;
  onOutputChange: (id: string) => void;
  onRefreshDevices: () => void;
  onRefreshModels: () => void;
  onInstall: () => void;
  onTeachVoice: () => void;
}) {
  const label =
    modelCheckState === "loading"
      ? "Checking…"
      : modelCheckState === "error"
        ? "Check failed"
        : modelsReady
          ? "Ready to listen and speak"
          : installing
            ? "Downloading…"
            : "Not downloaded yet · about 900 MB";
  const [live, setLive] = useState<LivenessStatus>({
    enrolled: false,
    takes: 0,
  });
  const [enrollErr, setEnrollErr] = useState<string | null>(null);
  const [clearDialogOpen, setClearDialogOpen] = useState(false);
  const [clearingVoice, setClearingVoice] = useState(false);
  const refreshLive = useCallback(async () => {
    setLive(await wakeLivenessStatus());
  }, []);
  useEffect(() => {
    void refreshLive();
  }, [refreshLive]);

  const onClearVoice = useCallback(async () => {
    setClearingVoice(true);
    setEnrollErr(null);
    try {
      await clearWakeProfile();
      await refreshLive();
      setClearDialogOpen(false);
    } catch (error) {
      setEnrollErr(error instanceof Error ? error.message : String(error));
    } finally {
      setClearingVoice(false);
    }
  }, [refreshLive]);

  return (
    <div className="flex flex-col gap-6">
      <SettingsGroup
        title="Microphone and speakers"
        footer="System default follows whatever Windows is using."
        action={
          <button
            type="button"
            disabled={busy}
            onClick={onRefreshDevices}
            className="inline-flex min-h-9 items-center gap-1.5 rounded-lg px-2.5 text-[12px] text-white/55 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40"
          >
            <RefreshCw className="size-3.5" /> Refresh
          </button>
        }
      >
        <SettingsRow label="Microphone" labelFor="input-device" stacked>
          <DevicePicker
            id="input-device"
            devices={inputs}
            selected={selectedInput}
            busy={busy}
            noun="microphone"
            onChange={onInputChange}
          />
        </SettingsRow>
        <SettingsRow label="Speakers" labelFor="output-device" stacked last>
          <DevicePicker
            id="output-device"
            devices={outputs}
            selected={selectedOutput}
            busy={busy}
            noun="speaker"
            onChange={onOutputChange}
          />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        title="“Hey Boris”"
        footer={
          live.enrolled
            ? "Voices from a TV, video, or speaker won't wake Boris. You still will."
            : "Say “Hey Boris” four times so Boris can tell you apart from a TV or video."
        }
      >
        <SettingsRow label="Chime when listening" subtitle="A soft tone the moment Boris hears you">
          <Toggle
            checked={settings.wake_chime}
            onChange={(v) => onPatch({ wake_chime: v })}
            aria-label="Chime when listening"
          />
        </SettingsRow>
        <SettingsRow
          label="Ignore TVs and videos"
          subtitle={
            live.enrolled
              ? `Using your voice sample · ${live.takes} takes`
              : "Teach Boris your voice to turn this on"
          }
        >
          <Toggle
            checked={settings.ignore_speaker_playback}
            onChange={(v) => onPatch({ ignore_speaker_playback: v })}
            aria-label="Ignore TVs and videos"
          />
        </SettingsRow>
        <SettingsRow label="Your voice sample" last>
          {live.enrolled ? (
            <AlertDialog.Root
              open={clearDialogOpen}
              onOpenChange={(open) => {
                if (!clearingVoice) setClearDialogOpen(open);
              }}
            >
              <AlertDialog.Trigger asChild>
                <button
                  type="button"
                  className="min-h-9 rounded-md px-2 text-[13px] font-medium text-red-300/75 decoration-red-300/60 underline-offset-4 transition-colors hover:text-red-200 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-300/30"
                >
                  Clear
                </button>
              </AlertDialog.Trigger>
              <AlertDialog.Portal>
                <AlertDialog.Overlay className="fixed inset-0 z-50 bg-black/65 backdrop-blur-[2px] data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:animate-in data-[state=open]:fade-in-0" />
                <AlertDialog.Content className="fixed left-1/2 top-1/2 z-50 w-[calc(100%-2rem)] max-w-sm -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-white/10 bg-[#1c1c1e] p-5 text-white shadow-[0_24px_80px_rgba(0,0,0,0.65)] outline-none data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95">
                  <div className="flex size-9 items-center justify-center rounded-full bg-red-400/10 text-red-300">
                    <Trash2 className="size-4" strokeWidth={1.8} />
                  </div>
                  <AlertDialog.Title className="mt-4 text-[17px] font-semibold tracking-[-0.015em]">
                    Clear voice sample?
                  </AlertDialog.Title>
                  <AlertDialog.Description className="mt-1.5 text-[13px] leading-relaxed text-white/50">
                    Boris will no longer use your voice sample to distinguish
                    you from nearby speakers. You can teach it again at any
                    time.
                  </AlertDialog.Description>
                  <div className="mt-5 flex justify-end gap-2">
                    <AlertDialog.Cancel asChild>
                      <button
                        type="button"
                        disabled={clearingVoice}
                        className="h-9 rounded-lg px-3 text-[13px] font-medium text-white/65 transition-colors hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40"
                      >
                        Keep
                      </button>
                    </AlertDialog.Cancel>
                    <AlertDialog.Action asChild>
                      <button
                        type="button"
                        disabled={clearingVoice}
                        onClick={(event) => {
                          event.preventDefault();
                          void onClearVoice();
                        }}
                        className="h-9 rounded-lg bg-red-500 px-3 text-[13px] font-semibold text-white transition-[background-color,transform,opacity] hover:bg-red-400 active:scale-[0.98] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-red-300/50 focus-visible:ring-offset-2 focus-visible:ring-offset-[#1c1c1e] disabled:cursor-not-allowed disabled:opacity-50"
                      >
                        {clearingVoice ? "Clearing…" : "Clear"}
                      </button>
                    </AlertDialog.Action>
                  </div>
                </AlertDialog.Content>
              </AlertDialog.Portal>
            </AlertDialog.Root>
          ) : (
            <button
              type="button"
              onClick={onTeachVoice}
              className="min-h-9 rounded-lg px-3 text-[13px] font-medium text-white/75 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25"
            >
              Teach Boris your voice
            </button>
          )}
        </SettingsRow>
        {enrollErr ? (
          <p className="px-4 pb-3 text-[12px] text-red-300/80">{enrollErr}</p>
        ) : null}
      </SettingsGroup>

      <SettingsGroup
        title="Interrupting"
        footer="Say “stop” to end his answer, or keep quiet and he carries on."
      >
        <SettingsRow
          label="Say “Hey Boris” to interrupt"
          subtitle="Cut in while Boris is talking or working on something"
          last
        >
          <Toggle
            checked={settings.voice_barge_in}
            onChange={(v) => onPatch({ voice_barge_in: v })}
            aria-label="Say Hey Boris to interrupt"
          />
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup
        id="speech-models"
        title="Voice"
        footer="Listening and speaking happen on this computer. Your voice isn't uploaded."
      >
        <SettingsRow label="Boris's voice" subtitle="More voices are coming">
          <span className="rounded-lg bg-white/[0.06] px-3 py-2 text-[13px] text-white/65">
            M4 · Default
          </span>
        </SettingsRow>
        <SettingsRow
          label="Speech files"
          subtitle={label}
          last={!installing}
        >
          {modelsReady && !advanced ? (
            <span className="inline-flex items-center gap-1.5 px-2 text-[13px] text-emerald-300/80">
              <Check className="size-3.5" strokeWidth={2.5} /> Installed
            </span>
          ) : (
            <button
              type="button"
              disabled={installing || modelCheckState === "loading"}
              onClick={
                modelCheckState === "missing" ? onInstall : onRefreshModels
              }
              className="min-h-9 rounded-lg px-3 text-[13px] font-medium text-white/75 hover:bg-white/[0.06] hover:text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/25 disabled:opacity-40"
            >
              {installing
                ? "Downloading…"
                : modelCheckState === "missing"
                  ? "Download"
                  : modelCheckState === "error"
                    ? "Retry"
                    : "Re-check"}
            </button>
          )}
        </SettingsRow>
        {installing ? <DownloadProgressView progress={progress} /> : null}
        <SettingsRow
          label="Say when a task runs long"
          subtitle="One short spoken update after a few seconds of tool work"
        >
          <Toggle
            checked={settings.progress_line}
            onChange={(v) => onPatch({ progress_line: v })}
            aria-label="Say when a task runs long"
          />
        </SettingsRow>
      </SettingsGroup>
    </div>
  );
}

function DownloadProgressView({
  progress,
}: {
  progress: DownloadProgress | null;
}) {
  const pct = progress?.total_bytes
    ? Math.min(
        100,
        Math.round((progress.bytes_downloaded / progress.total_bytes) * 100),
      )
    : null;
  return (
    <div
      className="border-t border-white/[0.06] px-4 py-3"
      role="status"
      aria-live="polite"
    >
      <div
        className="main-progress-track"
        role="progressbar"
        aria-label="Speech model download"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct ?? undefined}
      >
        <div className="main-progress-fill" style={{ width: `${pct ?? 4}%` }} />
      </div>
      <p className="mt-1.5 truncate text-[12px] text-white/50">
        {progress?.file_name ?? "Preparing…"}
        {pct != null ? ` · ${pct}%` : ""}
      </p>
    </div>
  );
}
