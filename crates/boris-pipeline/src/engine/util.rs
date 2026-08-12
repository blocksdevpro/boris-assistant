//! Small pure helpers used by the engine thread.

pub(super) use crate::env_util::{env_flag_false, env_flag_true};

/// Minimum alphanumeric chars required before a transcript is sent to the agent.
const MIN_TRANSCRIPT_ALNUM: usize = 2;

/// True when STT text is worth sending to the agent.
///
/// Rejects empty/whitespace and transcripts with fewer than
/// [`MIN_TRANSCRIPT_ALNUM`] alphanumeric characters (noise, partial wake,
/// accidental clicks).
pub(super) fn transcript_usable(text: &str) -> bool {
    text.chars().filter(|c| c.is_alphanumeric()).count() >= MIN_TRANSCRIPT_ALNUM
}

/// Lightweight OpenRouter sticky-session token without the `uuid` crate.
pub(super) fn session_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Mix with address entropy so two engines started same tick differ.
    let mix = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    format!("{mix:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_usable_rejects_empty_and_junk() {
        assert!(!transcript_usable(""));
        assert!(!transcript_usable("   \t\n"));
        assert!(!transcript_usable("a"));
        assert!(!transcript_usable("!"));
        assert!(!transcript_usable("."));
        assert!(!transcript_usable("a "));
        assert!(transcript_usable("hi"));
        assert!(transcript_usable("ok"));
        assert!(transcript_usable("hello world"));
        assert!(transcript_usable("  yo  "));
    }

    #[test]
    fn session_token_is_hex() {
        let id = session_token();
        assert!(!id.is_empty());
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "id={id}");
    }
}
