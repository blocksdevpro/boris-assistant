//! Chat role type used by [`super::Message`] and transcript loaders.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        f.write_str(s)
    }
}

impl Role {
    /// Parse a transcript / wire role string (`"user"`, `"assistant"`, …).
    ///
    /// Unknown roles return `None` (callers skip them when rebuilding history).
    pub fn from_role_str(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Role::System),
            "user" => Some(Role::User),
            "assistant" => Some(Role::Assistant),
            // Grok disk uses `tool_result`; agent context uses `tool`.
            "tool" | "tool_result" => Some(Role::Tool),
            // Reasoning rows are audit-only; never re-inject into the LLM context.
            _ => None,
        }
    }
}

impl FromStr for Role {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Role::from_role_str(s).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_from_str_roundtrip() {
        assert!(matches!(Role::from_str("user"), Ok(Role::User)));
        assert!(matches!(Role::from_str("assistant"), Ok(Role::Assistant)));
        assert!(matches!(Role::from_str("system"), Ok(Role::System)));
        assert!(matches!(Role::from_str("tool"), Ok(Role::Tool)));
        assert!(matches!(Role::from_str("tool_result"), Ok(Role::Tool)));
        assert!(Role::from_str("nope").is_err());
        assert!(Role::from_str("reasoning").is_err());
    }

    #[test]
    fn display_matches_wire_strings() {
        assert_eq!(Role::System.to_string(), "system");
        assert_eq!(Role::User.to_string(), "user");
        assert_eq!(Role::Assistant.to_string(), "assistant");
        assert_eq!(Role::Tool.to_string(), "tool");
    }
}
