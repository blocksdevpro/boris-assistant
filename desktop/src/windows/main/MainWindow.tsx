import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import {
  KeyRound,
  Mic,
  Power,
  PowerOff,
  RefreshCw,
  Volume2,
} from "lucide-react";
import { TitleBar } from "@/components/TitleBar";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  listInputDevices,
  listOutputDevices,
  startEngine,
  stopEngine,
  switchInput,
  switchOutput,
  useStatus,
  type DeviceDto,
  type StatusPicture,
} from "@/bridge";
import { toneFor } from "@/lib/phaseVisual";
import { cn } from "@/lib/utils";

const selectClassName = cn(
  "flex h-9 w-full appearance-none rounded-lg border border-white/[0.08]",
  "bg-white/[0.03] px-3 py-1.5 text-[13px] text-white/90 outline-none",
  "transition-colors hover:bg-white/[0.05]",
  "focus-visible:border-white/20 focus-visible:ring-2 focus-visible:ring-white/10",
  "disabled:cursor-not-allowed disabled:opacity-40",
);

/**
 * WINDOW — configure + control the voice engine.
 *
 * Design goals
 * 1. Presence first — same phase language as the floating island
 * 2. One obvious action — Start / Stop as the hero
 * 3. Conversation is first-class, not an afterthought
 * 4. Setup (API, devices) is secondary and calm
 * 5. Never touch PCM / models — host only
 */
export function MainWindow() {
  const status = useStatus();
  const tone = useMemo(
    () => toneFor(status.phase, status.engine),
    [status.phase, status.engine],
  );

  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("");
  const [inputs, setInputs] = useState<DeviceDto[]>([]);
  const [outputs, setOutputs] = useState<DeviceDto[]>([]);
  const [selectedInput, setSelectedInput] = useState("");
  const [selectedOutput, setSelectedOutput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const engineOn = status.engine === "On" || status.engine === "Starting";
  const engineFault = status.engine === "Fault";

  const refreshDevices = useCallback(async () => {
    try {
      const [ins, outs] = await Promise.all([
        listInputDevices(),
        listOutputDevices(),
      ]);
      setInputs(ins);
      setOutputs(outs);
      setSelectedInput((prev) => {
        if (prev && ins.some((d) => d.id === prev)) return prev;
        return ins.find((d) => d.is_default)?.id ?? ins[0]?.id ?? "";
      });
      setSelectedOutput((prev) => {
        if (prev && outs.some((d) => d.id === prev)) return prev;
        return outs.find((d) => d.is_default)?.id ?? outs[0]?.id ?? "";
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void refreshDevices();
  }, [refreshDevices]);

  const onStart = async () => {
    setBusy(true);
    setError(null);
    try {
      await startEngine(apiKey, model || undefined);
      if (selectedInput) await switchInput(selectedInput);
      if (selectedOutput) await switchOutput(selectedOutput);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const onStop = async () => {
    setBusy(true);
    setError(null);
    try {
      await stopEngine();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const onInputChange = async (id: string) => {
    setSelectedInput(id);
    if (status.engine === "Off") return;
    setBusy(true);
    setError(null);
    try {
      await switchInput(id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const onOutputChange = async (id: string) => {
    setSelectedOutput(id);
    if (status.engine === "Off") return;
    setBusy(true);
    setError(null);
    try {
      await switchOutput(id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="main-console flex h-screen flex-col overflow-hidden text-white">
      <TitleBar
        trailing={
          <span className="inline-flex items-center gap-1.5 rounded-full bg-white/[0.04] px-2.5 py-1 text-[11px] text-white/50 ring-1 ring-white/[0.06]">
            <span
              className="size-1.5 rounded-full"
              style={{ background: tone.accent, boxShadow: `0 0 8px ${tone.glow}` }}
            />
            {tone.label}
          </span>
        }
      />

      <main className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-6 py-5">
        {/* ── Hero presence ───────────────────────────────────────────── */}
        <section
          className="main-hero relative overflow-hidden rounded-2xl px-5 py-5"
          style={
            {
              "--hero-accent": tone.accent,
              "--hero-glow": tone.glow,
            } as CSSProperties
          }
        >
          <div
            aria-hidden
            className="pointer-events-none absolute -right-16 -top-20 size-56 rounded-full opacity-50 blur-3xl"
            style={{ background: tone.glow }}
          />

          <div className="relative flex flex-col gap-5 sm:flex-row sm:items-center sm:justify-between">
            <div className="flex min-w-0 items-center gap-4">
              <HeroOrb accent={tone.accent} motion={tone.motion} />
              <div className="min-w-0">
                <div className="flex flex-wrap items-baseline gap-2">
                  <h1 className="text-xl font-semibold tracking-tight text-white">
                    {tone.label}
                  </h1>
                  {status.turn ? (
                    <span className="font-mono text-[11px] tabular-nums text-white/30">
                      turn #{status.turn}
                    </span>
                  ) : null}
                </div>
                <p className="mt-0.5 text-[13px] text-white/45">{tone.hint}</p>
                {status.detail ? (
                  <p className="mt-1.5 text-[12px] text-red-300/90">{status.detail}</p>
                ) : null}
              </div>
            </div>

            <div className="flex shrink-0 flex-wrap items-center gap-2">
              <Button
                type="button"
                size="lg"
                disabled={busy || engineOn}
                onClick={() => void onStart()}
                className={cn(
                  "h-10 gap-2 rounded-xl px-5 font-semibold",
                  "bg-white text-[#0c0d10] hover:bg-white/90",
                  "disabled:bg-white/20 disabled:text-white/40",
                )}
              >
                <Power className="size-4" strokeWidth={2.25} />
                Start
              </Button>
              <Button
                type="button"
                size="lg"
                variant="outline"
                disabled={busy || status.engine === "Off"}
                onClick={() => void onStop()}
                className={cn(
                  "h-10 gap-2 rounded-xl border-white/10 bg-white/[0.04] px-5 text-white/80",
                  "hover:bg-white/[0.08] hover:text-white",
                  "disabled:opacity-30",
                )}
              >
                <PowerOff className="size-4" strokeWidth={2} />
                Stop
              </Button>
            </div>
          </div>

          {error ? (
            <p className="relative mt-4 rounded-lg bg-red-500/10 px-3 py-2 text-[12px] text-red-300 ring-1 ring-red-500/20">
              {error}
            </p>
          ) : null}
          {engineFault ? (
            <p className="relative mt-3 text-[12px] text-amber-200/80">
              Engine reported a fault — try Stop, then Start again.
            </p>
          ) : null}
        </section>

        {/* ── Conversation ────────────────────────────────────────────── */}
        <ConversationPanel status={status} />

        {/* ── Setup grid ──────────────────────────────────────────────── */}
        <div className="grid gap-4 lg:grid-cols-2">
          <Panel
            icon={<KeyRound className="size-3.5" strokeWidth={2} />}
            title="Connection"
            description="OpenRouter credentials for chat. Empty key uses the process env."
          >
            <div className="flex flex-col gap-3.5">
              <Field label="Model" htmlFor="model-id">
                <Input
                  id="model-id"
                  placeholder="e.g. google/gemini-2.5-flash-lite"
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  disabled={engineOn}
                  className="h-9 border-white/[0.08] bg-white/[0.03] text-[13px] text-white placeholder:text-white/25 focus-visible:border-white/20 focus-visible:ring-white/10"
                />
              </Field>
              <Field label="API key" htmlFor="api-key">
                <Input
                  id="api-key"
                  type="password"
                  placeholder="OPENROUTER_API_KEY or paste here"
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  disabled={engineOn}
                  autoComplete="off"
                  className="h-9 border-white/[0.08] bg-white/[0.03] text-[13px] text-white placeholder:text-white/25 focus-visible:border-white/20 focus-visible:ring-white/10"
                />
              </Field>
            </div>
          </Panel>

          <Panel
            icon={<Mic className="size-3.5" strokeWidth={2} />}
            title="Devices"
            description="Microphone and speakers. Live labels come from the engine."
            action={
              <button
                type="button"
                disabled={busy}
                onClick={() => void refreshDevices()}
                className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-[11px] text-white/45 transition-colors hover:bg-white/[0.06] hover:text-white/80 disabled:opacity-40"
              >
                <RefreshCw className="size-3" />
                Refresh
              </button>
            }
          >
            <div className="flex flex-col gap-3.5">
              <Field label="Microphone" htmlFor="mic">
                <div className="relative">
                  <select
                    id="mic"
                    className={selectClassName}
                    value={selectedInput}
                    disabled={busy || inputs.length === 0}
                    onChange={(e) => void onInputChange(e.target.value)}
                  >
                    {inputs.length === 0 ? (
                      <option value="">No devices found</option>
                    ) : (
                      inputs.map((d) => (
                        <option key={d.id} value={d.id} className="bg-[#1a1b20] text-white">
                          {d.name}
                          {d.is_default ? " (default)" : ""}
                        </option>
                      ))
                    )}
                  </select>
                  <DeviceHint
                    ok={status.mic.ok && engineOn}
                    label={status.mic.label}
                    icon={<Mic className="size-3" />}
                  />
                </div>
              </Field>
              <Field label="Speaker" htmlFor="speaker">
                <div className="relative">
                  <select
                    id="speaker"
                    className={selectClassName}
                    value={selectedOutput}
                    disabled={busy || outputs.length === 0}
                    onChange={(e) => void onOutputChange(e.target.value)}
                  >
                    {outputs.length === 0 ? (
                      <option value="">No devices found</option>
                    ) : (
                      outputs.map((d) => (
                        <option key={d.id} value={d.id} className="bg-[#1a1b20] text-white">
                          {d.name}
                          {d.is_default ? " (default)" : ""}
                        </option>
                      ))
                    )}
                  </select>
                  <DeviceHint
                    ok={status.speaker.ok && engineOn}
                    label={status.speaker.label}
                    icon={<Volume2 className="size-3" />}
                  />
                </div>
              </Field>
            </div>
          </Panel>
        </div>
      </main>
    </div>
  );
}

function HeroOrb({
  accent,
  motion,
}: {
  accent: string;
  motion: ReturnType<typeof toneFor>["motion"];
}) {
  return (
    <div className="relative flex size-14 shrink-0 items-center justify-center" aria-hidden>
      {motion !== "none" ? (
        <span
          className={cn(
            "absolute inset-0 rounded-full border border-current opacity-40",
            motion === "breathe" && "main-orb-breathe",
            motion === "listen" && "main-orb-listen",
            motion === "think" && "main-orb-think",
            motion === "speak" && "main-orb-speak",
          )}
          style={{ borderColor: accent, color: accent }}
        />
      ) : null}
      <span
        className="relative size-4 rounded-full"
        style={{
          background: accent,
          boxShadow: `0 0 22px ${accent}`,
        }}
      />
    </div>
  );
}

function ConversationPanel({ status }: { status: StatusPicture }) {
  const hasLines = Boolean(status.heard?.trim() || status.said?.trim());

  return (
    <section className="main-panel flex min-h-[120px] flex-col rounded-2xl">
      <header className="flex items-center justify-between border-b border-white/[0.05] px-4 py-3">
        <div>
          <h2 className="text-[13px] font-medium tracking-tight text-white/90">
            Conversation
          </h2>
          <p className="text-[11px] text-white/35">
            Last turn — mirrors the floating island
          </p>
        </div>
      </header>
      <div className="flex flex-1 flex-col gap-3 px-4 py-4">
        {!hasLines ? (
          <p className="text-[13px] leading-relaxed text-white/30">
            When Boris is listening, your words and his reply show up here.
          </p>
        ) : (
          <>
            {status.heard?.trim() ? (
              <Bubble who="You" text={status.heard.trim()} />
            ) : null}
            {status.said?.trim() ? (
              <Bubble who="Boris" text={status.said.trim()} accent />
            ) : null}
          </>
        )}
      </div>
    </section>
  );
}

function Bubble({
  who,
  text,
  accent,
}: {
  who: string;
  text: string;
  accent?: boolean;
}) {
  return (
    <div className="flex flex-col gap-1">
      <span
        className={cn(
          "text-[10px] font-semibold uppercase tracking-wider",
          accent ? "text-white/50" : "text-white/30",
        )}
      >
        {who}
      </span>
      <p
        className={cn(
          "rounded-xl px-3.5 py-2.5 text-[13px] leading-relaxed",
          accent
            ? "bg-white/[0.07] text-white/90 ring-1 ring-white/[0.06]"
            : "bg-white/[0.03] text-white/70 ring-1 ring-white/[0.04]",
        )}
      >
        {text}
      </p>
    </div>
  );
}

function Panel({
  icon,
  title,
  description,
  action,
  children,
}: {
  icon: ReactNode;
  title: string;
  description: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="main-panel flex flex-col rounded-2xl">
      <header className="flex items-start justify-between gap-3 border-b border-white/[0.05] px-4 py-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="flex size-6 items-center justify-center rounded-md bg-white/[0.05] text-white/55 ring-1 ring-white/[0.06]">
              {icon}
            </span>
            <h2 className="text-[13px] font-medium tracking-tight text-white/90">
              {title}
            </h2>
          </div>
          <p className="mt-1 text-[11px] leading-snug text-white/35">{description}</p>
        </div>
        {action}
      </header>
      <div className="px-4 py-4">{children}</div>
    </section>
  );
}

function Field({
  label,
  htmlFor,
  children,
}: {
  label: string;
  htmlFor: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={htmlFor} className="text-[11px] font-medium text-white/45">
        {label}
      </Label>
      {children}
    </div>
  );
}

function DeviceHint({
  ok,
  label,
  icon,
}: {
  ok: boolean;
  label: string;
  icon: ReactNode;
}) {
  return (
    <p className="mt-1.5 flex items-center gap-1.5 text-[11px] text-white/30">
      <span
        className={cn(
          "inline-flex size-4 items-center justify-center rounded-full",
          ok ? "text-emerald-400/90" : "text-white/25",
        )}
      >
        {icon}
      </span>
      <span className="truncate">Live · {label}</span>
      <span
        className={cn(
          "size-1.5 shrink-0 rounded-full",
          ok ? "bg-emerald-400" : "bg-white/15",
        )}
      />
    </p>
  );
}
