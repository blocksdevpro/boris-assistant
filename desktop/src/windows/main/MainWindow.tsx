import { useCallback, useEffect, useState } from "react";
import { TitleBar } from "@/components/TitleBar";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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
} from "@/bridge";

const selectClassName =
  "flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50";

/**
 * WINDOW — configure + control.
 * Writes intent (config, start/stop, devices). Reads status.
 * Never touches PCM / models.
 */
export function MainWindow() {
  const status = useStatus();
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("");
  const [inputs, setInputs] = useState<DeviceDto[]>([]);
  const [outputs, setOutputs] = useState<DeviceDto[]>([]);
  const [selectedInput, setSelectedInput] = useState("");
  const [selectedOutput, setSelectedOutput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
      // Apply pickers after engine is up (switch is a no-op host command if idle).
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

  const engineOn = status.engine === "On" || status.engine === "Starting";

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-background text-foreground">
      <TitleBar />

      <main className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto p-6">
        <div className="flex flex-wrap items-center gap-3 rounded-lg border border-border bg-card px-4 py-3 text-sm">
          <span className="text-muted-foreground">Engine</span>
          <span className="font-medium">{status.engine}</span>
          <span className="text-border">|</span>
          <span className="text-muted-foreground">Phase</span>
          <span className="font-medium">{status.phase}</span>
          {status.turn ? (
            <>
              <span className="text-border">|</span>
              <span className="text-muted-foreground">Turn</span>
              <span className="font-medium">{status.turn}</span>
            </>
          ) : null}
          {status.detail ? (
            <>
              <span className="text-border">|</span>
              <span className="text-destructive">{status.detail}</span>
            </>
          ) : null}
        </div>

        {(status.heard || status.said) && (
          <div className="grid gap-2 rounded-lg border border-border bg-card px-4 py-3 text-sm">
            {status.heard ? (
              <p>
                <span className="text-muted-foreground">Heard </span>
                {status.heard}
              </p>
            ) : null}
            {status.said ? (
              <p>
                <span className="text-muted-foreground">Said </span>
                {status.said}
              </p>
            ) : null}
          </div>
        )}

        <div className="grid gap-6 lg:grid-cols-2">
          <Card>
            <CardHeader>
              <CardTitle>Assistant</CardTitle>
              <CardDescription>
                OpenRouter key and model. Leave key empty to use{" "}
                <code className="text-xs">OPENROUTER_API_KEY</code> from the
                process environment.
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <div className="flex flex-col gap-2">
                <Label htmlFor="model-id">Model</Label>
                <Input
                  id="model-id"
                  placeholder="optional openrouter model id"
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  disabled={engineOn}
                />
              </div>
              <div className="flex flex-col gap-2">
                <Label htmlFor="api-key">API key</Label>
                <Input
                  id="api-key"
                  type="password"
                  placeholder="or set OPENROUTER_API_KEY"
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  disabled={engineOn}
                  autoComplete="off"
                />
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Devices</CardTitle>
              <CardDescription>
                Pick mic and speaker. Switch applies once the engine is running
                (or on the next start).
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <div className="flex flex-col gap-2">
                <Label htmlFor="mic">Microphone</Label>
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
                      <option key={d.id} value={d.id}>
                        {d.name}
                        {d.is_default ? " (default)" : ""}
                      </option>
                    ))
                  )}
                </select>
                <p className="text-xs text-muted-foreground">
                  Live: {status.mic.label}
                </p>
              </div>
              <div className="flex flex-col gap-2">
                <Label htmlFor="speaker">Speaker</Label>
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
                      <option key={d.id} value={d.id}>
                        {d.name}
                        {d.is_default ? " (default)" : ""}
                      </option>
                    ))
                  )}
                </select>
                <p className="text-xs text-muted-foreground">
                  Live: {status.speaker.label}
                </p>
              </div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={busy}
                onClick={() => void refreshDevices()}
              >
                Refresh devices
              </Button>
            </CardContent>
          </Card>
        </div>

        <Card>
          <CardHeader>
            <CardTitle>Engine</CardTitle>
            <CardDescription>
              Start runs the voice loop (wake → hear → read → think → talk).
              Stop returns to Off between stages.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <div className="flex gap-3">
              <Button
                type="button"
                disabled={busy || engineOn}
                onClick={() => void onStart()}
              >
                Start
              </Button>
              <Button
                type="button"
                variant="outline"
                disabled={busy || status.engine === "Off"}
                onClick={() => void onStop()}
              >
                Stop
              </Button>
            </div>
            {error ? (
              <p className="text-sm text-destructive">{error}</p>
            ) : null}
          </CardContent>
        </Card>
      </main>
    </div>
  );
}
