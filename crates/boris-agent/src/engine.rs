use std::time::Instant;

use serde_json::{json, Value};
use tracing::{error, info};

use crate::{
    client::LlmClient,
    context::{Context, Message, Role},
    error::AgentError,
    observe::TurnReport,
    outcome::AgentOutcome,
    tool::Tool,
};

/// Hard cap on tool-call rounds per user turn. Prevents unbounded ReAct loops
/// if the model keeps requesting tools (or invents unknown ones).
const MAX_TOOL_ROUNDS: usize = 5;

/// Max characters of user text included in turn-start logs (avoids dumping secrets).
const LOG_PREVIEW_CHARS: usize = 80;

pub struct AgentEngine {
    client: Box<dyn LlmClient>,
    tools: Vec<Box<dyn Tool>>,
    context: Context,
}

/// Truncate `s` for logging; appends `…` when cut.
fn log_preview(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let preview: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

impl AgentEngine {
    /// Create a new engine.
    ///
    /// `system_prompt` sets the assistant's persona and hard rules.
    /// Tools are optional — Boris currently runs with none registered and
    /// treats the model's final plain-text reply as speech ([`AgentOutcome::Speak`]).
    pub fn new(client: Box<dyn LlmClient>, system_prompt: &str) -> Self {
        let mut context = Context::new(20);
        context.push(Role::System, system_prompt);
        Self {
            client,
            tools: vec![],
            context,
        }
    }

    /// Register a tool the LLM may call during the ReAct-style loop.
    ///
    /// Tool results are fed back into context; they must not speak to the user
    /// directly. Final user-facing speech always comes from a turn's
    /// [`AgentOutcome`].
    pub fn register_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Clear conversation to a fresh system-only context (new session).
    ///
    /// Tools and the LLM client are left unchanged.
    pub fn reset_conversation(&mut self, system_prompt: &str) {
        self.context.messages.clear();
        self.context.push(Role::System, system_prompt);
    }

    /// Load prior user/assistant/tool messages after the system prompt.
    ///
    /// Used when resuming a session. `system_prompt` is forced as the first
    /// message; any system rows in `history` are dropped. Prunes once after bulk load.
    pub fn load_session_history(&mut self, system_prompt: &str, history: Vec<Message>) {
        self.context.load_history(system_prompt, history);
    }

    /// Snapshot messages for saving (clone).
    pub fn export_messages(&self) -> Vec<Message> {
        self.context.messages().to_vec()
    }

    /// Serialize registered tools for the OpenAI-compatible API.
    ///
    /// Returns `Value::Null` when no tools are registered so the client can
    /// omit `tools` / `tool_choice` entirely (avoids empty-array edge cases).
    fn tools_json(&self) -> Value {
        if self.tools.is_empty() {
            return Value::Null;
        }
        let list: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name":        t.name(),
                        "description": t.description(),
                        "parameters":  t.parameters(),
                    }
                })
            })
            .collect();
        json!(list)
    }

    /// Back-compat entry used by the pipeline. Delegates to [`Self::run_turn`].
    ///
    /// Prefer [`Self::run_turn`] / [`Self::run_turn_with_report`] for new call sites.
    pub fn chat(&mut self, message: &str) -> Result<AgentOutcome, AgentError> {
        self.run_turn(message)
    }

    /// Primary turn API: one user message → [`AgentOutcome`].
    ///
    /// Emits the same `tracing` events as [`Self::run_turn_with_report`] but
    /// discards the structured [`TurnReport`].
    pub fn run_turn(&mut self, user_text: &str) -> Result<AgentOutcome, AgentError> {
        self.run_turn_with_report(user_text)
            .map(|(outcome, _report)| outcome)
    }

    /// Run one user turn and return both the outcome and a [`TurnReport`].
    ///
    /// - Non-empty content → [`AgentOutcome::Speak`] (caller / Session should TTS it).
    /// - Empty content → [`AgentOutcome::Silent`].
    /// - Tool rounds are capped at [`MAX_TOOL_ROUNDS`]; unknown tools fail closed.
    /// - On any error the conversation context is rolled back to its pre-turn
    ///   snapshot so a failed HTTP/tool round does not leave unpaired messages.
    ///
    /// This crate never talks to the app event bus; the binary worker maps the
    /// outcome into runtime events.
    pub fn run_turn_with_report(
        &mut self,
        user_text: &str,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        let started = Instant::now();
        let preview = log_preview(user_text, LOG_PREVIEW_CHARS);
        info!(
            model = %self.client.model(),
            message_len = user_text.len(),
            preview = %preview,
            "agent turn start"
        );

        // Snapshot before mutating so prune during a failed turn cannot leave
        // a half-applied user/tool chain in multi-turn context.
        let snapshot = self.context.messages.clone();
        self.context.push(Role::User, user_text);

        match self.execute_turn_loop() {
            Ok(TurnLoopResult {
                outcome,
                tool_rounds,
                tools_used,
            }) => {
                let duration = started.elapsed();
                let outcome_label = match &outcome {
                    AgentOutcome::Speak(_) => "speak",
                    AgentOutcome::Silent => "silent",
                };
                let approx_chars_in = self.context.as_json().to_string().len();
                let report = TurnReport {
                    duration,
                    tool_rounds,
                    tools_used: tools_used.clone(),
                    outcome: outcome_label.to_string(),
                    approx_chars_in,
                };
                info!(
                    outcome = outcome_label,
                    duration_ms = duration.as_millis() as u64,
                    tool_rounds,
                    tools = ?tools_used,
                    approx_chars_in,
                    "agent turn end"
                );
                Ok((outcome, report))
            }
            Err(e) => {
                self.context.messages = snapshot;
                error!(
                    error = %e,
                    duration_ms = started.elapsed().as_millis() as u64,
                    "agent turn failed"
                );
                Err(e)
            }
        }
    }

    /// Internal ReAct loop after the user message is already on context.
    /// Caller owns snapshot / rollback and logging.
    fn execute_turn_loop(&mut self) -> Result<TurnLoopResult, AgentError> {
        let mut tools_used: Vec<String> = Vec::new();
        let mut tool_rounds: u32 = 0;

        for round in 0..=MAX_TOOL_ROUNDS {
            let response = self
                .client
                .complete(self.context.as_json(), self.tools_json())?;

            let tool_calls = &response["tool_calls"];
            if let Some(calls) = tool_calls.as_array() {
                if !calls.is_empty() {
                    if round == MAX_TOOL_ROUNDS {
                        return Err(AgentError::tool_loop(format!(
                            "tool loop exceeded {MAX_TOOL_ROUNDS} rounds without a final reply"
                        )));
                    }

                    tool_rounds += 1;
                    self.context.push(Role::Assistant, response.clone());

                    for call in calls {
                        let call_id = call["id"].as_str().unwrap_or("").to_string();
                        let fn_name = call["function"]["name"].as_str().unwrap_or("");
                        let args: Value = serde_json::from_str(
                            call["function"]["arguments"].as_str().unwrap_or("{}"),
                        )
                        .unwrap_or(json!({}));

                        let tool =
                            self.tools
                                .iter()
                                .find(|t| t.name() == fn_name)
                                .ok_or_else(|| {
                                    AgentError::unknown_tool(format!(
                                        "unknown tool requested by model: {fn_name}"
                                    ))
                                })?;

                        tools_used.push(fn_name.to_string());

                        let result = match tool.execute(args) {
                            Ok(output) => output,
                            Err(e) => format!("Error: {}", e.message),
                        };

                        self.context.push(
                            Role::Tool,
                            json!({ "tool_call_id": call_id, "content": result }),
                        );
                    }

                    continue;
                }
            }

            let reply = response["content"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            self.context.push(Role::Assistant, reply.clone());
            let outcome = if reply.is_empty() {
                AgentOutcome::Silent
            } else {
                AgentOutcome::Speak(reply)
            };
            return Ok(TurnLoopResult {
                outcome,
                tool_rounds,
                tools_used,
            });
        }

        // Unreachable: loop is `0..=MAX_TOOL_ROUNDS` and tool path returns on last round.
        Err(AgentError::tool_loop("tool loop exhausted"))
    }
}

struct TurnLoopResult {
    outcome: AgentOutcome,
    tool_rounds: u32,
    tools_used: Vec<String>,
}
