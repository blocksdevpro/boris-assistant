use serde_json::{json, Value};

use crate::{
    client::LlmClient,
    context::{Context, Role},
    error::AgentError,
    outcome::AgentOutcome,
    tool::Tool,
};

/// Hard cap on tool-call rounds per user turn. Prevents unbounded ReAct loops
/// if the model keeps requesting tools (or invents unknown ones).
const MAX_TOOL_ROUNDS: usize = 5;

pub struct AgentEngine {
    client: Box<dyn LlmClient>,
    tools: Vec<Box<dyn Tool>>,
    context: Context,
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
    /// directly. Final user-facing speech always comes from [`chat`]'s
    /// [`AgentOutcome`].
    pub fn register_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
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

    /// Run one user turn: call the LLM, execute any requested tools, repeat
    /// until the model returns a final plain-text message (no `tool_calls`).
    ///
    /// - Non-empty content → [`AgentOutcome::Speak`] (caller / Session should TTS it).
    /// - Empty content → [`AgentOutcome::Silent`].
    /// - Tool rounds are capped at [`MAX_TOOL_ROUNDS`]; unknown tools fail closed.
    ///
    /// This crate never talks to the app event bus; the binary worker maps the
    /// outcome into runtime events.
    pub fn chat(&mut self, message: &str) -> Result<AgentOutcome, AgentError> {
        self.context.push(Role::User, message);

        for round in 0..=MAX_TOOL_ROUNDS {
            let response = self
                .client
                .complete(self.context.as_json(), self.tools_json())?;

            let tool_calls = &response["tool_calls"];
            if let Some(calls) = tool_calls.as_array() {
                if !calls.is_empty() {
                    if round == MAX_TOOL_ROUNDS {
                        return Err(AgentError::new(format!(
                            "tool loop exceeded {MAX_TOOL_ROUNDS} rounds without a final reply"
                        )));
                    }

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
                                    AgentError::new(format!(
                                        "unknown tool requested by model: {fn_name}"
                                    ))
                                })?;

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
            if reply.is_empty() {
                return Ok(AgentOutcome::Silent);
            }
            return Ok(AgentOutcome::Speak(reply));
        }

        // Unreachable: loop is `0..=MAX_TOOL_ROUNDS` and tool path returns on last round.
        Err(AgentError::new("tool loop exhausted"))
    }
}
