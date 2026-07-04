use std::sync::mpsc::{Receiver, Sender};
use std::thread::{self, JoinHandle};

use boris_agent::{Engine, Tool, ToolError};
use boris_core::event::Event;
use serde_json::{Value, json};

// ── Commands ─────────────────────────────────────────────────────────────────

pub enum AgentCommand {
    /// Dispatch a transcribed utterance to the agent for processing.
    Chat(String),
}

// ── Speak Tool ────────────────────────────────────────────────────────────────

/// Gives the LLM the ability to speak aloud by routing text through the
/// event bus as `Event::AgentResponse`.
pub struct SpeakTool {
    event_tx: Sender<Event>,
}

impl SpeakTool {
    pub fn new(event_tx: Sender<Event>) -> Self {
        Self { event_tx }
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
                    "description": "The text to speak aloud to the user. \
                                    Keep it concise and natural for speech."
                }
            },
            "required": ["text"]
        })
    }

    fn execute(&self, args: Value) -> Result<String, ToolError> {
        let text = args["text"].as_str().unwrap_or("").to_string();
        tracing::debug!("speak tool invoked: \"{}\"", text);
        self.event_tx
            .send(Event::AgentResponse(text))
            .map_err(|e| ToolError {
                message: e.to_string(),
            })?;
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
    /// The engine should have all tools registered before being passed here.
    /// Output flows through the event bus via the tools (e.g. `SpeakTool`),
    /// not through the return value of `chat()`.
    pub fn spawn(command_rx: Receiver<AgentCommand>, mut engine: Engine) -> Self {
        let handle = thread::spawn(move || {
            while let Ok(cmd) = command_rx.recv() {
                match cmd {
                    AgentCommand::Chat(text) => {
                        tracing::debug!("Agent received: \"{}\"", text);
                        engine.chat(&text);
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}
