import { invoke } from "@tauri-apps/api/core";
import { logger } from "@/lib/logger";
import { invokeErrorMessage } from "../errors";
import { COMMANDS } from "../ipc";
import type { ArtifactCard, ArtifactListItem } from "../types";

/** Visual cards in the active voice session (empty when no session). */
export async function listSessionArtifacts(): Promise<ArtifactListItem[]> {
  try {
    return await invoke<ArtifactListItem[]>(COMMANDS.listSessionArtifacts);
  } catch (e) {
    logger.warn("list_session_artifacts failed", invokeErrorMessage(e));
    return [];
  }
}

/** One card body. Omit `id` for the current card. */
export async function getSessionArtifact(
  id?: string | null,
): Promise<ArtifactCard | null> {
  try {
    return await invoke<ArtifactCard>(COMMANDS.getSessionArtifact, {
      id: id ?? null,
    });
  } catch (e) {
    logger.warn("get_session_artifact failed", invokeErrorMessage(e));
    return null;
  }
}
