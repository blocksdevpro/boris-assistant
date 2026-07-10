use serde_json::Value;

pub struct ToolError {
    pub message: String,
}

/// Capability the LLM may invoke during the engine tool loop.
///
/// Implementations return data **to the model** (observations). They must not
/// drive TTS, playback, or the app event bus — final speech is always
/// [`crate::AgentOutcome`] from [`crate::AgentEngine::chat`].
pub trait Tool: Send + Sync {
    /// Snake_case name the LLM uses to invoke this tool.
    fn name(&self) -> &str;

    /// Plain-English description so the LLM knows when to use it.
    fn description(&self) -> &str;

    /// JSON Schema object describing the tool's accepted arguments.
    /// Use `json!({ "type": "object", "properties": {}, "required": [] })`
    /// for tools that take no arguments.
    fn parameters(&self) -> Value;

    /// Run the tool with the JSON args the LLM supplied.
    /// The returned string is sent back to the LLM as the tool result.
    fn execute(&self, args: Value) -> Result<String, ToolError>;
}
