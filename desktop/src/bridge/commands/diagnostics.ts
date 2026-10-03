import { invoke } from "@tauri-apps/api/core";
import { COMMANDS } from "../ipc";

export type DebugEvent = {
  seq: number;
  at_ms: number;
  turn_id: string | null;
  kind: string;
  data: Record<string, unknown>;
};

export type DebugSnapshot = {
  enabled: boolean;
  oldest_seq: number;
  latest_seq: number;
  dropped_events: number;
  events: DebugEvent[];
};

export function getDebugCapture(after: number): Promise<DebugSnapshot> {
  return invoke<DebugSnapshot>(COMMANDS.getDebugCapture, { after });
}

export function setDebugCapture(enabled: boolean): Promise<void> {
  return invoke<void>(COMMANDS.setDebugCapture, { enabled });
}

export function clearDebugCapture(): Promise<void> {
  return invoke<void>(COMMANDS.clearDebugCapture);
}
