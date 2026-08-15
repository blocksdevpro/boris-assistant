//! Voice session persistence under `~/.boris/sessions/desktop`.
//!
//! Soft-fails on store I/O — the voice loop continues without persistence.

use boris_agent::context::Context;
use boris_agent::session::store::{SessionStore, SyncCursor};
use boris_agent::session::types::SessionId;
use boris_agent::{Agent, MaintenanceJob};
use boris_audio::service::AudioService;
use boris_inference::{SpeechToText, TextToSpeech};

use crate::status::{EngineState, Phase};

use super::models::release_voice_models;
use super::picture::Picture;

const DURABLE_FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Export agent messages as `(role, content)` pairs for session persistence.
pub(super) fn agent_message_pairs(agent: &Agent) -> Vec<(String, serde_json::Value)> {
    agent
        .export_messages()
        .into_iter()
        .map(|m| (m.role.to_string(), m.content))
        .collect()
}

/// Snapshot the live context and hand JSONL work to the durable maintenance
/// lane. The optimistic count is only a UI/host cursor; the worker reconciles
/// each ordered snapshot against the store's persisted cursor.
pub(super) fn enqueue_transcript_sync(
    store: &SessionStore,
    id: &SessionId,
    transcript_len: &mut usize,
    agent: &Agent,
) {
    let pairs = agent_message_pairs(agent);
    let snapshot_len = pairs.len();
    let Some(maintenance) = agent.maintenance() else {
        // Product setup always installs the worker. Keep the engine responsive
        // if an embedding host omits it instead of silently restoring blocking
        // transcript I/O on the speech path.
        tracing::warn!(session_id = %id, "session sync skipped: maintenance worker unavailable");
        return;
    };
    let job = MaintenanceJob::SyncSession {
        store: store.clone(),
        id: id.clone(),
        messages: pairs,
        cursor: SyncCursor {
            count: *transcript_len,
            fingerprint: 0,
        },
        reply: None,
    };
    match maintenance.submit(job) {
        Ok(()) => *transcript_len = snapshot_len,
        Err(e) => tracing::warn!(
            error = %e,
            session_id = %id,
            "session sync enqueue failed; snapshot skipped to keep engine nonblocking"
        ),
    }
}

/// Start or resume a voice session and seed the agent context.
///
/// Order: `resume_or_create` → `ensure_session_artifacts` → load/reset context
/// → `bind_session` → `active_session = Some`.
pub(super) fn begin_session(
    store: &SessionStore,
    active_session: &mut Option<SessionId>,
    transcript_len: &mut usize,
    agent: &mut Agent,
    system_prompt: &str,
) {
    *active_session = None;
    *transcript_len = 0;
    let started = std::time::Instant::now();
    if let Some(maintenance) = agent.maintenance() {
        if !maintenance.flush_durable(DURABLE_FLUSH_TIMEOUT) {
            tracing::warn!(
                "previous session persistence still draining; starting without session persistence"
            );
            agent.reset(system_prompt);
            agent.unbind_session();
            return;
        }
    }
    let previous = match store.current_id() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "session current_id failed");
            None
        }
    };

    match store.resume_or_create() {
        Ok(meta) => {
            // Ensure session-local todos / dir exist before bind (no workspace migration).
            if let Err(e) = store.ensure_session_artifacts(&meta.id) {
                tracing::warn!(
                    error = %e,
                    session_id = %meta.id,
                    "ensure_session_artifacts failed"
                );
            }

            let resumed = previous.as_ref() == Some(&meta.id);
            if resumed {
                match store.load_transcript(&meta.id) {
                    Ok(records) => {
                        let wire: Vec<(String, serde_json::Value)> =
                            records.into_iter().map(|r| (r.role, r.content)).collect();
                        let history = Context::messages_from_transcript(&wire);
                        agent.load_session_history(system_prompt, history);
                        // Align disk with live agent (current system prompt wins over stale system row).
                        enqueue_transcript_sync(store, &meta.id, transcript_len, agent);
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            session_id = %meta.id,
                            "failed to load session transcript; resetting conversation"
                        );
                        agent.reset(system_prompt);
                        seed_transcript(store, &meta.id, transcript_len, agent);
                    }
                }
            } else {
                agent.reset(system_prompt);
                // Grok writes the system row at session start.
                seed_transcript(store, &meta.id, transcript_len, agent);
            }

            // Bind session-local paths (todos, audit, subagent root, sid, activations).
            agent.bind_session(&store.session_dir(&meta.id), meta.id.as_str());
            tracing::info!(
                session_id = %meta.id,
                ms = started.elapsed().as_millis() as u64,
                resumed,
                "session ready"
            );
            *active_session = Some(meta.id);
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "session resume_or_create failed; continuing without persistence"
            );
            agent.reset(system_prompt);
            agent.unbind_session();
        }
    }
}

/// Write the current agent context (typically just system) as the chat_history baseline.
fn seed_transcript(
    store: &SessionStore,
    id: &SessionId,
    transcript_len: &mut usize,
    agent: &Agent,
) {
    enqueue_transcript_sync(store, id, transcript_len, agent);
}

/// Soft-fail end of the current session (Stop / go_off / shutdown).
pub(super) fn end_session(
    store: &SessionStore,
    active_session: &mut Option<SessionId>,
    transcript_len: &mut usize,
    agent: &mut Agent,
) {
    let started = std::time::Instant::now();
    agent.abort();
    let ending_id = active_session.clone();
    if let Some(ref sid) = ending_id {
        enqueue_transcript_sync(store, sid, transcript_len, agent);
    }
    let durable_ready = agent
        .maintenance()
        .is_none_or(|maintenance| maintenance.flush_durable(DURABLE_FLUSH_TIMEOUT));
    agent.unbind_session();
    let ended = if durable_ready {
        match ending_id.as_ref() {
            Some(id) => store.end(id).map(Some),
            None => Ok(None),
        }
    } else {
        tracing::warn!("session end deferred because durable transcript sync timed out");
        Ok(None)
    };
    match ended {
        Ok(Some(meta)) => {
            tracing::info!(
                session_id = %meta.id,
                ms = started.elapsed().as_millis() as u64,
                "session ended"
            );
        }
        Ok(None) => {
            if let Some(ref sid) = active_session {
                tracing::debug!(session_id = %sid, "session end_current: no current pointer");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "session end_current failed");
        }
    }
    *active_session = None;
    *transcript_len = 0;
}

#[allow(clippy::too_many_arguments)]
pub(super) fn go_off(
    picture: &mut Picture,
    audio: &AudioService,
    store: &SessionStore,
    active_session: &mut Option<SessionId>,
    transcript_len: &mut usize,
    stt: &mut dyn SpeechToText,
    tts: &mut dyn TextToSpeech,
    agent: &mut Agent,
) {
    end_session(store, active_session, transcript_len, agent);
    audio.stop();
    release_voice_models(stt, tts, "engine stop");
    picture.engine = EngineState::Off;
    picture.turn = None;
    picture.artifact = None;
    picture.set_phase(Phase::Off);
}
