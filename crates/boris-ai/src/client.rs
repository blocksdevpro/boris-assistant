//! Provider-agnostic LLM client trait.

use async_trait::async_trait;
use serde_json::Value;

use crate::error::LlmError;

/// HTTP transport abstraction so the agent harness stays provider-agnostic.
///
/// # Contract
///
/// - `messages` — JSON array of chat messages (OpenAI shape).
/// - `tools` — JSON array of tool definitions, or `null` / `[]` when none.
/// - Return value — the raw `choices[0].message` object, with:
///   - `content` preferably a **string** (not a parts array / null)
///   - optional `tool_calls` array
///
/// Implementations may stream internally; the trait surface is still one-shot
/// complete for the voice agent loop.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send the conversation + tools; return `choices[0].message` JSON.
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError>;

    /// Model identifier for logging / turn metadata.
    fn model(&self) -> &str {
        "unknown"
    }
}
