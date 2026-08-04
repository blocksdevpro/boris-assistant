use async_trait::async_trait;
use serde_json::Value;

use crate::error::LlmError;

/// Abstracts the HTTP transport so the agent harness stays provider-agnostic.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send the full conversation + tool definitions and return the raw
    /// `choices[0].message` JSON object.
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError>;

    /// Model identifier used for this client (for logging / turn metadata).
    fn model(&self) -> &str {
        "unknown"
    }
}
