use std::fmt;

// ── LlmError ──────────────────────────────────────────────────────────────────

/// Classification of an [`LlmError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorKind {
    /// Request timed out (connect or overall deadline).
    Timeout,
    /// HTTP transport or non-success status from the provider endpoint.
    Http,
    /// Failed to parse or extract fields from the provider response.
    Parse,
    /// Provider-reported application error (rate limit, invalid model, etc.).
    Provider,
    /// Unclassified / catch-all.
    Other,
}

/// Failure talking to an LLM provider or parsing its response.
#[derive(Debug)]
pub struct LlmError {
    pub message: String,
    kind: LlmErrorKind,
}

impl LlmError {
    /// Create an error with kind [`LlmErrorKind::Other`].
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: LlmErrorKind::Other,
        }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: LlmErrorKind::Timeout,
        }
    }

    pub fn http(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: LlmErrorKind::Http,
        }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: LlmErrorKind::Parse,
        }
    }

    pub fn provider(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: LlmErrorKind::Provider,
        }
    }

    pub fn kind(&self) -> LlmErrorKind {
        self.kind
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LlmError {}

// ── AgentError ────────────────────────────────────────────────────────────────

/// Classification of an [`AgentError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentErrorKind {
    /// Propagated from an [`LlmError`] (non-timeout).
    Llm,
    /// Tool-loop exhaustion or unrecoverable tool iteration issue.
    ToolLoop,
    /// Referenced tool name is not registered.
    UnknownTool,
    /// Deadline exceeded (LLM timeout or agent-level timeout).
    Timeout,
    /// Operation was cancelled.
    Cancelled,
    /// Unclassified / catch-all.
    Other,
}

/// Failure inside the agent turn (LLM error or unrecoverable tool-loop issue).
#[derive(Debug)]
pub struct AgentError {
    pub message: String,
    kind: AgentErrorKind,
}

impl AgentError {
    /// Create an error with kind [`AgentErrorKind::Other`].
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: AgentErrorKind::Other,
        }
    }

    pub fn llm(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: AgentErrorKind::Llm,
        }
    }

    pub fn tool_loop(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: AgentErrorKind::ToolLoop,
        }
    }

    pub fn unknown_tool(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: AgentErrorKind::UnknownTool,
        }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: AgentErrorKind::Timeout,
        }
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: AgentErrorKind::Cancelled,
        }
    }

    pub fn kind(&self) -> AgentErrorKind {
        self.kind
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AgentError {}

impl From<LlmError> for AgentError {
    fn from(value: LlmError) -> Self {
        let kind = match value.kind {
            LlmErrorKind::Timeout => AgentErrorKind::Timeout,
            LlmErrorKind::Http
            | LlmErrorKind::Parse
            | LlmErrorKind::Provider
            | LlmErrorKind::Other => AgentErrorKind::Llm,
        };
        Self {
            message: value.message,
            kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_new_is_other() {
        let e = LlmError::new("oops");
        assert_eq!(e.kind(), LlmErrorKind::Other);
        assert_eq!(e.message, "oops");
        assert_eq!(e.to_string(), "oops");
    }

    #[test]
    fn agent_new_is_other() {
        let e = AgentError::new("oops");
        assert_eq!(e.kind(), AgentErrorKind::Other);
        assert_eq!(e.to_string(), "oops");
    }

    #[test]
    fn from_llm_timeout_maps_to_agent_timeout() {
        let agent: AgentError = LlmError::timeout("deadline").into();
        assert_eq!(agent.kind(), AgentErrorKind::Timeout);
        assert_eq!(agent.message, "deadline");
        assert_eq!(agent.to_string(), "deadline");
    }

    #[test]
    fn from_llm_http_maps_to_agent_llm() {
        let agent: AgentError = LlmError::http("bad status").into();
        assert_eq!(agent.kind(), AgentErrorKind::Llm);
        assert_eq!(agent.message, "bad status");
    }

    #[test]
    fn from_llm_parse_maps_to_agent_llm() {
        let agent: AgentError = LlmError::parse("bad json").into();
        assert_eq!(agent.kind(), AgentErrorKind::Llm);
    }

    #[test]
    fn from_llm_provider_maps_to_agent_llm() {
        let agent: AgentError = LlmError::provider("rate limited").into();
        assert_eq!(agent.kind(), AgentErrorKind::Llm);
    }

    #[test]
    fn from_llm_other_maps_to_agent_llm() {
        let agent: AgentError = LlmError::new("misc").into();
        assert_eq!(agent.kind(), AgentErrorKind::Llm);
        assert_eq!(agent.message, "misc");
    }

    #[test]
    fn agent_constructors_set_kinds() {
        assert_eq!(AgentError::tool_loop("x").kind(), AgentErrorKind::ToolLoop);
        assert_eq!(
            AgentError::unknown_tool("x").kind(),
            AgentErrorKind::UnknownTool
        );
        assert_eq!(AgentError::timeout("x").kind(), AgentErrorKind::Timeout);
        assert_eq!(AgentError::cancelled("x").kind(), AgentErrorKind::Cancelled);
        assert_eq!(AgentError::llm("x").kind(), AgentErrorKind::Llm);
    }
}
