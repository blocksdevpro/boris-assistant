//! LLM provider errors.

use std::fmt;

/// Max characters of an HTTP/provider body included in [`LlmError`] messages.
pub const ERROR_BODY_MAX_CHARS: usize = 1024;

/// Classification of an [`LlmError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LlmErrorKind {
    /// Request timed out (connect or overall deadline).
    Timeout,
    /// HTTP transport failure (connect/network) or unclassified status.
    Http,
    /// Failed to parse or extract fields from the provider response.
    Parse,
    /// Provider-reported application error (rate limit, auth, 5xx, invalid model, …).
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
    /// Human-readable description (safe to show in logs; body text is truncated).
    pub message: String,
    kind: LlmErrorKind,
    /// HTTP status code when the failure came from a response, if known.
    status: Option<u16>,
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

    /// Transport / unclassified HTTP failure.
    pub fn http(message: impl Into<String>) -> Self {
        Self::with_kind(LlmErrorKind::Http, message)
    }

    /// JSON / shape parse failure.
    pub fn parse(message: impl Into<String>) -> Self {
        Self::with_kind(LlmErrorKind::Parse, message)
    }

    /// Provider application error (rate limit, bad model, 4xx/5xx body, …).
    pub fn provider(message: impl Into<String>) -> Self {
        Self::with_kind(LlmErrorKind::Provider, message)
    }

    fn with_kind(kind: LlmErrorKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind,
            status: None,
        }
    }

    /// Attach an HTTP status code (builder-style).
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    /// Map a non-success HTTP response from an LLM provider endpoint.
    ///
    /// - `408` / `504` → [`LlmErrorKind::Timeout`]
    /// - other `4xx` / `5xx` → [`LlmErrorKind::Provider`]
    /// - anything else → [`LlmErrorKind::Http`]
    pub fn from_http_status(status: reqwest::StatusCode, body: &str) -> Self {
        let code = status.as_u16();
        let kind = classify_http_status(code);
        let body = truncate_error_body(body);
        let message = if body.is_empty() {
            format!("LLM HTTP {code}")
        } else {
            format!("LLM HTTP {code}: {body}")
        };
        Self {
            message,
            kind,
            status: Some(code),
        }
    }

    /// Provider JSON `error` object on an otherwise-successful HTTP response.
    pub fn from_provider_error_value(error: &serde_json::Value) -> Self {
        let msg = error
            .get("message")
            .and_then(|m| m.as_str())
            .or_else(|| error.as_str())
            .unwrap_or("provider error");
        let mut err = Self::provider(format!("LLM provider error: {}", truncate_error_body(msg)));
        if let Some(code) = error
            .get("code")
            .and_then(|c| c.as_u64())
            .or_else(|| error.get("code").and_then(|c| c.as_str()?.parse().ok()))
        {
            if code <= u16::MAX as u64 {
                err = err.with_status(code as u16);
            }
        }
        err
    }

    /// Error classification.
    pub fn kind(&self) -> LlmErrorKind {
        self.kind
    }

    /// HTTP status when known.
    pub fn status(&self) -> Option<u16> {
        self.status
    }

    /// Borrow the message string.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Truncate a response/error body for inclusion in error messages.
pub fn truncate_error_body(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    let mut chars = body.chars();
    let truncated: String = chars.by_ref().take(ERROR_BODY_MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{truncated}…(truncated)")
    } else {
        truncated
    }
}

/// Classify an HTTP status from a provider endpoint.
pub fn classify_http_status(code: u16) -> LlmErrorKind {
    match code {
        408 | 504 => LlmErrorKind::Timeout,
        // Auth, rate limit, client/server errors reported by the API.
        400..=599 => LlmErrorKind::Provider,
        _ => LlmErrorKind::Http,
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
    use reqwest::StatusCode;

    #[test]
    fn llm_new_is_other() {
        let e = LlmError::new("oops");
        assert_eq!(e.kind(), LlmErrorKind::Other);
        assert_eq!(e.message, "oops");
        assert_eq!(e.message(), "oops");
        assert_eq!(e.to_string(), "oops");
        assert_eq!(e.status(), None);
    }

    #[test]
    fn constructors_set_kinds() {
        assert_eq!(LlmError::timeout("t").kind(), LlmErrorKind::Timeout);
        assert_eq!(LlmError::http("h").kind(), LlmErrorKind::Http);
        assert_eq!(LlmError::parse("p").kind(), LlmErrorKind::Parse);
        assert_eq!(LlmError::provider("p").kind(), LlmErrorKind::Provider);
        assert_eq!(LlmErrorKind::Timeout.as_str(), "timeout");
    }

    #[test]
    fn status_mapping_provider_and_timeout() {
        assert_eq!(classify_http_status(401), LlmErrorKind::Provider);
        assert_eq!(classify_http_status(403), LlmErrorKind::Provider);
        assert_eq!(classify_http_status(429), LlmErrorKind::Provider);
        assert_eq!(classify_http_status(404), LlmErrorKind::Provider);
        assert_eq!(classify_http_status(500), LlmErrorKind::Provider);
        assert_eq!(classify_http_status(503), LlmErrorKind::Provider);
        assert_eq!(classify_http_status(408), LlmErrorKind::Timeout);
        assert_eq!(classify_http_status(504), LlmErrorKind::Timeout);
        assert_eq!(classify_http_status(200), LlmErrorKind::Http);
    }

    #[test]
    fn from_http_status_attaches_code_and_truncates() {
        let long = "x".repeat(ERROR_BODY_MAX_CHARS + 50);
        let e = LlmError::from_http_status(StatusCode::TOO_MANY_REQUESTS, &long);
        assert_eq!(e.kind(), LlmErrorKind::Provider);
        assert_eq!(e.status(), Some(429));
        assert!(e.message.contains("429"));
        assert!(e.message.contains("…(truncated)"));
        assert!(e.message.len() < long.len());
    }

    #[test]
    fn from_provider_error_value() {
        let v = serde_json::json!({
            "message": "No endpoints found that support tool use",
            "code": 404
        });
        let e = LlmError::from_provider_error_value(&v);
        assert_eq!(e.kind(), LlmErrorKind::Provider);
        assert_eq!(e.status(), Some(404));
        assert!(e.message.contains("support tool use"));
    }

    #[test]
    fn truncate_error_body_short_unchanged() {
        assert_eq!(truncate_error_body("  hi  "), "hi");
        assert_eq!(truncate_error_body(""), "");
    }

    #[test]
    fn with_status_builder() {
        let e = LlmError::provider("rate limited").with_status(429);
        assert_eq!(e.status(), Some(429));
        assert_eq!(e.kind(), LlmErrorKind::Provider);
    }
}
