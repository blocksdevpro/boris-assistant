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
import { OFF_STATUS } from "@/bridge";

/**
 * WINDOW — configure + control.
 * Writes intent (config, start/stop, devices). Reads status.
 * Never touches PCM / models.
 */
export function MainWindow() {
  // Local placeholder until bridge + config land.
  const status = OFF_STATUS;

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-background text-foreground">
      <TitleBar />

      <main className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto p-6">
        {/* Status strip */}
        <div className="flex flex-wrap items-center gap-3 rounded-lg border border-border bg-card px-4 py-3 text-sm">
          <span className="text-muted-foreground">Engine</span>
          <span className="font-medium">{status.engine}</span>
          <span className="text-border">|</span>
          <span className="text-muted-foreground">Phase</span>
          <span className="font-medium">{status.phase}</span>
          {status.detail ? (
            <>
              <span className="text-border">|</span>
              <span className="text-destructive">{status.detail}</span>
            </>
          ) : null}
        </div>

        <div className="grid gap-6 lg:grid-cols-2">
          {/* Config — forms only; save later */}
          <Card>
            <CardHeader>
              <CardTitle>Assistant</CardTitle>
              <CardDescription>
                API key, model, and local model paths. Saved in a later step.
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <div className="flex flex-col gap-2">
                <Label htmlFor="model-id">Model</Label>
                <Input
                  id="model-id"
                  placeholder="e.g. openrouter model id"
                  disabled
                />
              </div>
              <div className="flex flex-col gap-2">
                <Label htmlFor="api-key">API key</Label>
                <Input
                  id="api-key"
                  type="password"
                  placeholder="Stored in OS keyring later"
                  disabled
                />
              </div>
            </CardContent>
          </Card>

          {/* Devices — list/select later via controller */}
          <Card>
            <CardHeader>
              <CardTitle>Devices</CardTitle>
              <CardDescription>
                Mic and speaker pickers. AUDIO service owns the truth later.
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <div className="flex flex-col gap-2">
                <Label htmlFor="mic">Microphone</Label>
                <Input id="mic" value={status.mic.label} disabled readOnly />
              </div>
              <div className="flex flex-col gap-2">
                <Label htmlFor="speaker">Speaker</Label>
                <Input
                  id="speaker"
                  value={status.speaker.label}
                  disabled
                  readOnly
                />
              </div>
            </CardContent>
          </Card>
        </div>

        {/* Engine controls — wire to controller later */}
        <Card>
          <CardHeader>
            <CardTitle>Engine</CardTitle>
            <CardDescription>
              Start / stop the orchestrator. Disabled until Rust host exists.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex gap-3">
            <Button type="button" disabled>
              Start
            </Button>
            <Button type="button" variant="outline" disabled>
              Stop
            </Button>
          </CardContent>
        </Card>
      </main>
    </div>
  );
}
