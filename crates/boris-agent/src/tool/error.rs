//! Tool execution errors.

use std::fmt;

/// Classification of a [`ToolError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolErrorKind {
    /// Arguments failed validation (missing/wrong type/shape).
    InvalidArgs,
    /// Tool ran but failed to produce a useful result.
    Failed,
    /// Tool exceeded its time budget.
    Timeout,
    /// Result (or intermediate data) was truncated past a hard limit.
    Truncated,
    /// Unclassified / catch-all.
    Other,
}

impl ToolErrorKind {
    /// Stable lowercase label for logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgs => "invalid_args",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
            Self::Truncated => "truncated",
            Self::Other => "other",
        }
    }
}

/// Failure while executing a registered tool.
///
/// `message` is public for back-compat with the engine tool loop
/// (`format!("Error: {}", e.message)`). Prefer constructors over struct literals.
#[derive(Debug, Clone)]
pub struct ToolError {
    /// Human-readable detail (often shown back to the model as `Error: …`).
    pub message: String,
    kind: ToolErrorKind,
}

impl ToolError {
    /// Create an error with kind [`ToolErrorKind::Other`].
    pub fn new(message: impl Into<String>) -> Self {
        Self::with_kind(ToolErrorKind::Other, message)
    }

    /// Arguments failed validation.
    pub fn invalid_args(message: impl Into<String>) -> Self {
        Self::with_kind(ToolErrorKind::InvalidArgs, message)
    }

    /// Tool execution failed after args were accepted.
    pub fn failed(message: impl Into<String>) -> Self {
        Self::with_kind(ToolErrorKind::Failed, message)
    }

    /// Tool timed out.
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::with_kind(ToolErrorKind::Timeout, message)
    }

    /// Output or payload was truncated past a hard limit.
    pub fn truncated(message: impl Into<String>) -> Self {
        Self::with_kind(ToolErrorKind::Truncated, message)
    }

    fn with_kind(kind: ToolErrorKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    /// Error classification.
    pub fn kind(&self) -> ToolErrorKind {
        self.kind
    }

    /// Borrow the message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_error_constructors_set_kind() {
        assert_eq!(ToolError::new("x").kind(), ToolErrorKind::Other);
        assert_eq!(
            ToolError::invalid_args("x").kind(),
            ToolErrorKind::InvalidArgs
        );
        assert_eq!(ToolError::failed("x").kind(), ToolErrorKind::Failed);
        assert_eq!(ToolError::timeout("x").kind(), ToolErrorKind::Timeout);
        assert_eq!(ToolError::truncated("x").kind(), ToolErrorKind::Truncated);
        assert_eq!(ToolError::new("msg").message, "msg");
        assert_eq!(ToolErrorKind::Failed.as_str(), "failed");
    }
}
