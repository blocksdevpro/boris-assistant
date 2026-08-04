use std::fmt;

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
    fn constructors_set_kinds() {
        assert_eq!(LlmError::timeout("t").kind(), LlmErrorKind::Timeout);
        assert_eq!(LlmError::http("h").kind(), LlmErrorKind::Http);
        assert_eq!(LlmError::parse("p").kind(), LlmErrorKind::Parse);
        assert_eq!(LlmError::provider("p").kind(), LlmErrorKind::Provider);
    }
}
