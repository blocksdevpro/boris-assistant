//! Structured tool observations. Provider-facing text is rendered at the boundary.

use serde_json::{json, Value};

use super::schema::InvalidArgs;

/// Typed error payload inside a structured observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationError {
    pub code: String,
    pub retryable: bool,
    pub message: String,
    pub path: Option<String>,
    pub expected: Option<String>,
    pub raw_preview: Option<String>,
}

impl ObservationError {
    pub fn new(code: impl Into<String>, retryable: bool, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            retryable,
            message: message.into(),
            path: None,
            expected: None,
            raw_preview: None,
        }
    }

    pub fn from_invalid_args(inv: &InvalidArgs) -> Self {
        Self {
            code: inv.code.clone(),
            retryable: true,
            message: inv.message.clone(),
            path: Some(inv.path.clone()),
            expected: Some(inv.expected.clone()),
            raw_preview: Some(inv.raw_preview.clone()),
        }
    }
}

/// Internal observation produced by the runtime. Not sent to the provider as JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolObservation {
    pub ok: bool,
    pub data: Value,
    pub error: Option<ObservationError>,
    pub truncated: bool,
    pub bytes: usize,
    pub duration_ms: u64,
    pub cursor: Option<String>,
}

impl ToolObservation {
    pub fn ok_text(text: impl Into<String>, duration_ms: u64) -> Self {
        let text = text.into();
        let bytes = text.len();
        Self {
            ok: true,
            data: Value::String(text),
            error: None,
            truncated: false,
            bytes,
            duration_ms,
            cursor: None,
        }
    }

    pub fn from_text(
        text: String,
        duration_ms: u64,
        truncated: bool,
        cursor: Option<String>,
    ) -> Self {
        let bytes = text.len();
        Self {
            ok: true,
            data: Value::String(text),
            error: None,
            truncated,
            bytes,
            duration_ms,
            cursor,
        }
    }

    pub fn err(error: ObservationError, duration_ms: u64) -> Self {
        Self {
            ok: false,
            data: Value::Null,
            error: Some(error),
            truncated: false,
            bytes: 0,
            duration_ms,
            cursor: None,
        }
    }

    pub fn invalid_args(inv: InvalidArgs) -> Self {
        Self::err(ObservationError::from_invalid_args(&inv), 0)
    }

    /// Bounded text form for the model context (never the raw internal struct).
    pub fn to_provider_text(&self) -> String {
        if let Some(err) = &self.error {
            let mut s = format!("Error [{}]: {}", err.code, err.message);
            if let Some(path) = &err.path {
                if path != "$" {
                    s.push_str(&format!(" (at {path})"));
                }
            }
            if let Some(expected) = &err.expected {
                s.push_str(&format!("; expected {expected}"));
            }
            if err.retryable {
                s.push_str(". Fix the arguments and retry.");
            }
            if let Some(preview) = &err.raw_preview {
                if !preview.is_empty() {
                    s.push_str(&format!(" Raw: {preview}"));
                }
            }
            return s;
        }
        let mut body = match &self.data {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if let Some(cursor) = &self.cursor {
            let inline = format!("cursor={cursor}");
            let footer = format!("[cursor: {cursor}]");
            if !body.contains(&inline) && !body.contains(&footer) {
                body.push_str(&format!("\n{footer}"));
            }
        }
        if self.truncated && !body.contains("[truncated") && !body.contains("[cursor:") {
            body.push_str("\n…[truncated]");
        }
        body
    }

    /// JSON form used by tests / traces (not the provider payload).
    pub fn to_debug_json(&self) -> Value {
        json!({
            "ok": self.ok,
            "data": self.data,
            "error": self.error.as_ref().map(|e| json!({
                "code": e.code,
                "retryable": e.retryable,
                "message": e.message,
                "path": e.path,
                "expected": e.expected,
            })),
            "truncated": self.truncated,
            "bytes": self.bytes,
            "duration_ms": self.duration_ms,
            "cursor": self.cursor,
        })
    }

    pub fn looks_ok(&self) -> bool {
        self.ok && self.error.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::schema::InvalidArgs;

    #[test]
    fn provider_text_for_ok() {
        let obs = ToolObservation::ok_text("pong", 3);
        assert!(obs.looks_ok());
        assert_eq!(obs.to_provider_text(), "pong");
    }

    #[test]
    fn provider_text_for_invalid_args_is_repairable() {
        let inv = InvalidArgs::new(
            "missing_required",
            "$.command",
            "present",
            "missing required argument `command`",
            "{\"x\":1}",
        );
        let text = ToolObservation::invalid_args(inv).to_provider_text();
        assert!(text.starts_with("Error [missing_required]"));
        assert!(text.contains("command"));
        assert!(text.contains("Fix the arguments"));
        assert!(text.contains("Raw:"));
    }

    #[test]
    fn provider_text_does_not_duplicate_inline_cursor() {
        let obs = ToolObservation::from_text(
            "head\n…[truncated; cursor=byte:4 lines=1-9]…\ntail".into(),
            1,
            true,
            Some("byte:4".into()),
        );
        let text = obs.to_provider_text();
        assert_eq!(text.matches("byte:4").count(), 1);
        assert_eq!(text.matches("[truncated").count(), 1);
    }
}
