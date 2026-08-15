//! Provider-agnostic LLM client trait.

use async_trait::async_trait;
use serde_json::Value;

use crate::error::LlmError;
use crate::request::CompleteOptions;
use crate::stream::LlmStreamEvent;

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
/// Implementations may stream internally. [`Self::complete`] stays one-shot
/// (assembled message). [`Self::complete_stream`] exposes typed deltas.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send the conversation + tools; return `choices[0].message` JSON.
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError>;

    /// Same as [`Self::complete`] with per-request reasoning / token overrides.
    ///
    /// Default ignores options so existing mocks stay valid. Product clients
    /// (OpenRouter / routing) override this.
    async fn complete_with_options(
        &self,
        messages: Value,
        tools: Value,
        opts: CompleteOptions,
    ) -> Result<Value, LlmError> {
        let _ = opts;
        self.complete(messages, tools).await
    }

    /// Stream typed events, then return the assembled message (same as `complete`).
    ///
    /// Default: one-shot complete, then emit `ModelSend` + `FinalMessage`.
    async fn complete_stream(
        &self,
        messages: Value,
        tools: Value,
        opts: CompleteOptions,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<Value, LlmError> {
        on_event(LlmStreamEvent::ModelSend {
            model: self.model().to_string(),
        });
        let msg = self.complete_with_options(messages, tools, opts).await?;
        on_event(LlmStreamEvent::FinalMessage(msg.clone()));
        Ok(msg)
    }

    /// Model identifier for logging / turn metadata.
    fn model(&self) -> &str {
        "unknown"
    }
}
