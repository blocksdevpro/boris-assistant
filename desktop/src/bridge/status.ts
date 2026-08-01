/**
 * Typed bridge for engine status + control.
 * Rust owns the source of truth; this is the window/overlay surface.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invokeErrorMessage } from "./errors";
import {
  normalizeStatus,
  OFF_STATUS,
  type DeviceDto,
  type StatusPicture,
} from "./types";

/** Pull once (overlay mount, window focus, etc.). */
export async function getStatus(): Promise<StatusPicture> {
  try {
    const raw = await invoke<StatusPicture>("get_status");
    return normalizeStatus(raw);
  } catch {
    return { ...OFF_STATUS };
  }
}

/**
 * Subscribe to status pushes from Rust (`emit("status", …)`).
 * Returns an unsubscribe function.
 */
export async function onStatus(
  handler: (picture: StatusPicture) => void,
): Promise<() => void> {
  let unlisten: UnlistenFn | undefined;
  try {
    unlisten = await listen<StatusPicture>("status", (e) => {
      handler(normalizeStatus(e.payload));
    });
  } catch {
    return () => {};
  }
  return () => {
    unlisten?.();
  };
}

export async function startEngine(
  apiKey = "",
  model?: string,
): Promise<void> {
  try {
    await invoke("start_engine", {
      apiKey,
      model: model?.trim() ? model.trim() : null,
    });
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

export async function stopEngine(): Promise<void> {
  try {
    await invoke("stop_engine");
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

export async function listInputDevices(): Promise<DeviceDto[]> {
  try {
    return await invoke<DeviceDto[]>("list_input_devices");
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

export async function listOutputDevices(): Promise<DeviceDto[]> {
  try {
    return await invoke<DeviceDto[]>("list_output_devices");
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

export async function switchInput(deviceId: string): Promise<void> {
  try {
    await invoke("switch_input", { deviceId });
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}

export async function switchOutput(deviceId: string): Promise<void> {
  try {
    await invoke("switch_output", { deviceId });
  } catch (e) {
    throw new Error(invokeErrorMessage(e));
  }
}
