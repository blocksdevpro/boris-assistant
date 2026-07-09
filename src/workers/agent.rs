use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use boris_agent::{AgentEngine, Tool, ToolError};
use boris_core::{event::Event, ServiceKind, TurnId};
use serde_json::{json, Value};

// ── Commands ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum AgentCommand {
    /// Dispatch a transcribed utterance to the agent for processing.
    Chat { turn: TurnId, text: String },
}

// ── Speak Tool ────────────────────────────────────────────────────────────────

/// Routes spoken text through the event bus as [`Event::AgentResponse`].
///
/// The active [`TurnId`] is set by [`AgentWorker`] before each `chat` call so
/// late tool results stay correlated to the correct session turn.
pub struct SpeakTool {
    event_tx: Sender<Event>,
    turn: Arc<Mutex<TurnId>>,
    spoke: Arc<AtomicBool>,
}

impl SpeakTool {
    pub fn new(
        event_tx: Sender<Event>,
        turn: Arc<Mutex<TurnId>>,
        spoke: Arc<AtomicBool>,
    ) -> Self {
        Self {
            event_tx,
            turn,
            spoke,
        }
    }
}

impl Tool for SpeakTool {
    fn name(&self) -> &str {
        "speak"
    }

    fn description(&self) -> &str {
        "Speak a response aloud to the user. \
         Always use this tool to deliver your answer — do not reply with plain text."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to speak aloud. Keep it concise and natural for speech."
                }
            },
            "required": ["text"]
        })
    }

    fn execute(&self, args: Value) -> Result<String, ToolError> {
        let text = args["text"].as_str().unwrap_or("").to_string();
        let turn = *self.turn.lock().map_err(|e| ToolError {
            message: format!("turn lock poisoned: {e}"),
        })?;
        tracing::debug!(%turn, text = %text, "speak tool invoked");
        self.event_tx
            .send(Event::AgentResponse { turn, text })
            .map_err(|e| ToolError {
                message: e.to_string(),
            })?;
        self.spoke.store(true, Ordering::SeqCst);
        Ok("spoken".to_string())
    }
}

// ── Agent Worker ──────────────────────────────────────────────────────────────

pub struct AgentWorker {
    _handle: JoinHandle<()>,
}

impl AgentWorker {
    /// Spawn the agent on its own thread.
    ///
    /// `turn_slot` / `spoke_flag` are shared with [`SpeakTool`]. If the model
    /// never calls `speak` but returns plain text, that text is forwarded once
    /// as [`Event::AgentResponse`].
    pub fn spawn(
        command_rx: Receiver<AgentCommand>,
        mut engine: AgentEngine,
        turn_slot: Arc<Mutex<TurnId>>,
        spoke_flag: Arc<AtomicBool>,
        event_tx: Sender<Event>,
    ) -> Self {
        let handle = thread::spawn(move || {
            while let Ok(cmd) = command_rx.recv() {
                match cmd {
                    AgentCommand::Chat { turn, text } => {
                        tracing::debug!(%turn, text = %text, "agent received message");
                        if let Ok(mut slot) = turn_slot.lock() {
                            *slot = turn;
                        }
                        spoke_flag.store(false, Ordering::SeqCst);

                        match engine.chat(&text) {
                            Ok(reply) => {
                                if !spoke_flag.load(Ordering::SeqCst) && !reply.trim().is_empty() {
                                    event_tx
                                        .send(Event::AgentResponse {
                                            turn,
                                            text: reply,
                                        })
                                        .ok();
                                } else if !spoke_flag.load(Ordering::SeqCst) {
                                    event_tx
                                        .send(Event::WorkerError {
                                            turn: Some(turn),
                                            worker: "AgentWorker",
                                            kind: ServiceKind::Agent,
                                            message: "agent produced no speech".into(),
                                        })
                                        .ok();
                                }
                            }
                            Err(e) => {
                                tracing::error!(error = %e, %turn, "agent chat failed");
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
