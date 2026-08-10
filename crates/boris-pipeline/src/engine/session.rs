//! Voice session persistence under `~/.boris/sessions/desktop`.
//!
//! Soft-fails on store I/O — the voice loop continues without persistence.

use boris_agent::context::Context;
use boris_agent::session::store::SessionStore;
use boris_agent::session::types::SessionId;
use boris_agent::Agent;
use boris_audio::service::AudioService;
use boris_inference::{SpeechToText, TextToSpeech};

use crate::status::{EngineState, Phase};

use super::models::release_voice_models;
use super::picture::Picture;

/// Export agent messages as `(role, content)` pairs for session persistence.
pub(super) fn agent_message_pairs(agent: &Agent) -> Vec<(String, serde_json::Value)> {
    agent
        .export_messages()
        .into_iter()
        .map(|m| (m.role.to_string(), m.content))
        .collect()
}

/// Start or resume a voice session and seed the agent context.
pub(super) fn begin_session(
    store: &SessionStore,
    active_session: &mut Option<SessionId>,
    transcript_len: &mut usize,
    agent: &mut Agent,
    system_prompt: &str,
) {
    *active_session = None;
    *transcript_len = 0;
    let previous = match store.current_id() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "session current_id failed");
            None
        }
    };

    match store.resume_or_create() {
        Ok(meta) => {
            let resumed = previous.as_ref() == Some(&meta.id);
            if resumed {
                tracing::info!(session_id = %meta.id, "session resumed");
                match store.load_transcript(&meta.id) {
                    Ok(records) => {
                        let wire: Vec<(String, serde_json::Value)> =
                            records.into_iter().map(|r| (r.role, r.content)).collect();
                        let history = Context::messages_from_transcript(&wire);
                        agent.load_session_history(system_prompt, history);
                        // Align disk with live agent (current system prompt wins over stale system row).
                        let pairs = agent_message_pairs(agent);
                        if let Err(e) = store.write_messages(&meta.id, &pairs) {
                            tracing::warn!(
                                error = %e,
                                session_id = %meta.id,
                                "session rewrite after resume failed"
                            );
                        }
                        *transcript_len = pairs.len();
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
                tracing::info!(session_id = %meta.id, "session created");
                agent.reset(system_prompt);
                // Grok writes the system row at session start.
                seed_transcript(store, &meta.id, transcript_len, agent);
            }
            *active_session = Some(meta.id);
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "session resume_or_create failed; continuing without persistence"
            );
            agent.reset(system_prompt);
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
    let pairs = agent_message_pairs(agent);
    match store.write_messages(id, &pairs) {
        Ok(()) => *transcript_len = pairs.len(),
        Err(e) => {
            tracing::warn!(error = %e, session_id = %id, "session seed transcript failed");
            *transcript_len = 0;
        }
    }
}

/// Soft-fail end of the current session (Stop / go_off / shutdown).
pub(super) fn end_session(
    store: &SessionStore,
    active_session: &mut Option<SessionId>,
    transcript_len: &mut usize,
) {
    match store.end_current() {
        Ok(Some(meta)) => {
            tracing::info!(session_id = %meta.id, "session ended");
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

pub(super) fn go_off(
    picture: &mut Picture,
    audio: &AudioService,
    store: &SessionStore,
    active_session: &mut Option<SessionId>,
    transcript_len: &mut usize,
    stt: &mut dyn SpeechToText,
    tts: &mut dyn TextToSpeech,
) {
    end_session(store, active_session, transcript_len);
    audio.stop();
    release_voice_models(stt, tts, "engine stop");
    picture.engine = EngineState::Off;
    picture.turn = None;
    picture.set_phase(Phase::Off);
}
