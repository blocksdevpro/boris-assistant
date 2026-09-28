import { invoke } from "@tauri-apps/api/core";
import { logger } from "@/lib/logger";
import { invokeErrorMessage } from "../errors";
import { COMMANDS } from "../ipc";
import type { DeviceDto } from "../types";
export async function listInputDevices(): Promise<DeviceDto[]> {
  try {
    return await invoke<DeviceDto[]>(COMMANDS.listInputDevices);
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("list_input_devices failed", msg);
    throw new Error(msg);
  }
}

export async function listOutputDevices(): Promise<DeviceDto[]> {
  try {
    return await invoke<DeviceDto[]>(COMMANDS.listOutputDevices);
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("list_output_devices failed", msg);
    throw new Error(msg);
  }
}

export async function switchInput(deviceId: string): Promise<void> {
  logger.info("switchInput", { deviceId });
  try {
    await invoke(COMMANDS.switchInput, { deviceId });
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("switchInput failed", msg);
    throw new Error(msg);
  }
}

export async function switchOutput(deviceId: string): Promise<void> {
  logger.info("switchOutput", { deviceId });
  try {
    await invoke(COMMANDS.switchOutput, { deviceId });
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("switchOutput failed", msg);
    throw new Error(msg);
  }
}

/** Whether Parakeet + Supertone are present under `~/.boris/models`. */
