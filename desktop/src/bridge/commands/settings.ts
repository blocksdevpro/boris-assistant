import { invoke } from "@tauri-apps/api/core";
import { logger } from "@/lib/logger";
import { invokeErrorMessage } from "../errors";
import { COMMANDS } from "../ipc";
import {
  EMPTY_SETTINGS,
  normalizeSettings,
  settingsToWire,
  type AppSettings,
} from "../types";
export async function getSettings(): Promise<AppSettings> {
  try {
    const raw = await invoke<Partial<AppSettings>>(COMMANDS.getSettings);
    return normalizeSettings(raw);
  } catch (e) {
    logger.warn("get_settings failed", invokeErrorMessage(e));
    return { ...EMPTY_SETTINGS };
  }
}

/** Persist API keys + models/providers (never log secret values). */
export async function saveSettings(settings: AppSettings): Promise<void> {
  const wire = settingsToWire(settings);
  try {
    await invoke(COMMANDS.saveAppSettings, { settings: wire });
    logger.info("saveSettings ok", {
      hasOpenRouterKey: Boolean(wire.openrouter_api_key?.trim()),
      hasExaKey: Boolean(wire.exa_api_key?.trim()),
      model: wire.openrouter_model || null,
      fastModel: wire.openrouter_fast_model || null,
      capability: wire.capability_preset || null,
      memory: wire.long_term_memory,
      voice: wire.tts_voice_id || null,
    });
  } catch (e) {
    const msg = invokeErrorMessage(e);
    logger.error("saveSettings failed", msg);
    throw new Error(msg);
  }
}
