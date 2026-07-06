use serde_json::Value;

pub struct ToolError {
    pub message: String,
}

/// Implement this trait to expose a capability to the LLM.
pub trait Tool: Send + Sync {
    /// Snake_case name the LLM uses to invoke this tool.
    fn name(&self) -> &str;

    /// Plain-English description so the LLM knows when to use it.
    fn description(&self) -> &str;

    /// JSON Schema object describing the tool's accepted arguments.
    /// Use `json!({ "type": "object", "properties": {}, "required": [] })`
    /// for tools that take no arguments.
    fn parameters(&self) -> Value;

    /// Execute the tool with the JSON args the LLM supplied.
    /// Return a plain string that is sent back to the LLM as the result.
    fn execute(&self, args: Value) -> Result<String, ToolError>;
}
