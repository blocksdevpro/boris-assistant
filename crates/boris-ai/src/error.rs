//! LLM provider errors.

use std::fmt;

/// Classification of an [`LlmError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

impl LlmErrorKind {
    /// Stable lowercase label for logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Http => "http",
            Self::Parse => "parse",
            Self::Provider => "provider",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for LlmErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Failure talking to an LLM provider or parsing its response.
#[derive(Debug, Clone)]
pub struct LlmError {
    /// Human-readable description (safe to show in logs; may include status bodies).
    pub message: String,
    kind: LlmErrorKind,
}

impl LlmError {
    /// Create an error with kind [`LlmErrorKind::Other`].
    pub fn new(message: impl Into<String>) -> Self {
        Self::with_kind(LlmErrorKind::Other, message)
    }

    /// Timeout (connect or overall request deadline).
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::with_kind(LlmErrorKind::Timeout, message)
    }

    /// Transport / HTTP status failure.
    pub fn http(message: impl Into<String>) -> Self {
        Self::with_kind(LlmErrorKind::Http, message)
    }

    /// JSON / shape parse failure.
    pub fn parse(message: impl Into<String>) -> Self {
        Self::with_kind(LlmErrorKind::Parse, message)
    }

    /// Provider application error (rate limit, bad model, …).
    pub fn provider(message: impl Into<String>) -> Self {
        Self::with_kind(LlmErrorKind::Provider, message)
    }

    fn with_kind(kind: LlmErrorKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    /// Error classification.
    pub fn kind(&self) -> LlmErrorKind {
        self.kind
    }

    /// Borrow the message string.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LlmError {}

impl From<reqwest::Error> for LlmError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            return Self::timeout(format!(
                "LLM request timed out (connect or overall timeout): {err}"
            ));
        }
        if err.is_connect() {
            return Self::http(format!(
                "LLM connection failed (connect timeout or network): {err}"
            ));
        }
        Self::http(format!("HTTP request failed: {err}"))
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
        assert_eq!(e.message(), "oops");
        assert_eq!(e.to_string(), "oops");
    }

    #[test]
    fn constructors_set_kinds() {
        assert_eq!(LlmError::timeout("t").kind(), LlmErrorKind::Timeout);
        assert_eq!(LlmError::http("h").kind(), LlmErrorKind::Http);
        assert_eq!(LlmError::parse("p").kind(), LlmErrorKind::Parse);
        assert_eq!(LlmError::provider("p").kind(), LlmErrorKind::Provider);
        assert_eq!(LlmErrorKind::Timeout.as_str(), "timeout");
    }
}
