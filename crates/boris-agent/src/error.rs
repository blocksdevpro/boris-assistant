use std::fmt;

/// Failure talking to an LLM provider or parsing its response.
#[derive(Debug)]
pub struct LlmError {
    pub message: String,
}

impl LlmError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LlmError {}

/// Failure inside the agent turn (LLM error or unrecoverable tool-loop issue).
#[derive(Debug)]
pub struct AgentError {
    pub message: String,
}

impl AgentError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
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
        Self {
            message: value.message,
        }
    }
}
