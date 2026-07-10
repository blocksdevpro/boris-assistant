use std::sync::mpsc::{Receiver, Sender};
use std::thread::{self, JoinHandle};

use boris_agent::{AgentEngine, AgentOutcome};
use boris_core::{event::Event, ServiceKind, TurnId};

// ── Commands ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum AgentCommand {
    /// Run one agent turn for this session [`TurnId`].
    Chat { turn: TurnId, text: String },
}

// ── Agent Worker ──────────────────────────────────────────────────────────────

/// Owns [`AgentEngine`] on a background thread.
///
/// After each [`AgentEngine::chat`] call, emits exactly one of:
/// - [`Event::AgentResponse`] for [`AgentOutcome::Speak`]
/// - [`Event::WorkerError`] for silent / empty / failed turns
///
/// so Session always leaves `Thinking` (speech path or recovery to Idle).
pub struct AgentWorker {
    _handle: JoinHandle<()>,
}

impl AgentWorker {
    /// Spawn the agent on its own thread.
    ///
    /// `event_tx` is the only channel out of this worker; the engine itself
    /// returns [`AgentOutcome`] and never sends events.
    pub fn spawn(
        command_rx: Receiver<AgentCommand>,
        mut engine: AgentEngine,
        event_tx: Sender<Event>,
    ) -> Self {
        let handle = thread::spawn(move || {
            while let Ok(cmd) = command_rx.recv() {
                match cmd {
                    AgentCommand::Chat { turn, text } => {
                        tracing::debug!(%turn, text = %text, "agent received message");

                        match engine.chat(&text) {
                            Ok(AgentOutcome::Speak(speech)) if !speech.trim().is_empty() => {
                                event_tx
                                    .send(Event::AgentResponse { turn, text: speech })
                                    .ok();
                            }
                            Ok(AgentOutcome::Speak(_)) | Ok(AgentOutcome::Silent) => {
                                event_tx
                                    .send(Event::WorkerError {
                                        turn: Some(turn),
                                        worker: "AgentWorker",
                                        kind: ServiceKind::Agent,
                                        message: "agent produced no speech".into(),
                                    })
                                    .ok();
                            }
                            Err(e) => {
                                event_tx
                                    .send(Event::WorkerError {
                                        turn: Some(turn),
                                        worker: "AgentWorker",
                                        kind: ServiceKind::Agent,
                                        message: e.to_string(),
                                    })
                                    .ok();
                            }
                        }
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}
