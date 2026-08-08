//! Small pure helpers used by the engine thread.

/// Minimum alphanumeric chars required before a transcript is sent to the agent.
const MIN_TRANSCRIPT_ALNUM: usize = 2;

/// True when STT text is worth sending to the agent.
///
/// Rejects empty/whitespace and transcripts with fewer than
/// [`MIN_TRANSCRIPT_ALNUM`] alphanumeric characters (noise, partial wake,
/// accidental clicks).
pub(super) fn transcript_usable(text: &str) -> bool {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .count()
        >= MIN_TRANSCRIPT_ALNUM
}

/// Env is one of `1` / `true` / `yes` / `on` (case-insensitive).
pub(super) fn env_flag_true(key: &str) -> bool {
    std::env::var(key)
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

/// Env is one of `0` / `false` / `no` / `off` (case-insensitive).
pub(super) fn env_flag_false(key: &str) -> bool {
    std::env::var(key)
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "0" | "false" | "no" | "off")
        })
        .unwrap_or(false)
}

/// Lightweight session id without pulling in the `uuid` crate.
pub(super) fn uuid_lite() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Mix with address entropy so two engines started same tick differ.
    let mix = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    format!("{mix:016x}")
}

pub(super) fn panic_payload_str(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".into()
    }
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
    fn uuid_lite_is_hex() {
        let id = uuid_lite();
        assert!(!id.is_empty());
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "id={id}");
    }
}
