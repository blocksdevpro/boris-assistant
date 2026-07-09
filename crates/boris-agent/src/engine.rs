use serde_json::{json, Value};

use crate::{
    client::LlmClient,
    context::{Context, Role},
    error::AgentError,
    tool::Tool,
};

pub struct AgentEngine {
    client: Box<dyn LlmClient>,
    tools: Vec<Box<dyn Tool>>,
    context: Context,
}

impl AgentEngine {
    /// Create a new engine.
    ///
    /// `system_prompt` sets the assistant's persona and hard rules.
    /// Register tools with [`register_tool`] before the first [`chat`] call.
    pub fn new(client: Box<dyn LlmClient>, system_prompt: &str) -> Self {
        let mut context = Context::new(20);
        context.push(Role::System, system_prompt);
        Self {
            client,
            tools: vec![],
            context,
        }
    }

    /// Register a tool the LLM is allowed to call.
    pub fn register_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Serialize registered tools into the JSON array expected by the OpenAI API.
    fn tools_json(&self) -> Value {
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

    /// Send a user message and run the tool-call loop until the LLM gives a
    /// plain-text reply (or only tool side-effects like `speak`).
    ///
    /// Speech still flows through registered tools (e.g. `SpeakTool`). The
    /// returned string is the final plain-text content when the model does not
    /// use tools — callers may treat a non-empty value as a fallback utterance.
    pub fn chat(&mut self, message: &str) -> Result<String, AgentError> {
        self.context.push(Role::User, message);

        loop {
            let response = self
                .client
                .complete(self.context.as_json(), self.tools_json())?;

            let tool_calls = &response["tool_calls"];
            if let Some(calls) = tool_calls.as_array() {
                if !calls.is_empty() {
                    self.context.push(Role::Assistant, response.clone());

                    for call in calls {
                        let call_id = call["id"].as_str().unwrap_or("").to_string();
                        let fn_name = call["function"]["name"].as_str().unwrap_or("");
                        let args: Value = serde_json::from_str(
                            call["function"]["arguments"].as_str().unwrap_or("{}"),
                        )
                        .unwrap_or(json!({}));

                        let result = self
                            .tools
                            .iter()
                            .find(|t| t.name() == fn_name)
                            .map(|t| match t.execute(args) {
                                Ok(output) => output,
                                Err(e) => format!("Error: {}", e.message),
                            })
                            .unwrap_or_else(|| format!("Unknown tool: {fn_name}"));

                        self.context.push(
                            Role::Tool,
                            json!({ "tool_call_id": call_id, "content": result }),
                        );
                    }

                    continue;
                }
            }

            let reply = response["content"].as_str().unwrap_or("").to_string();
            self.context.push(Role::Assistant, reply.clone());
            return Ok(reply);
        }
    }
}
