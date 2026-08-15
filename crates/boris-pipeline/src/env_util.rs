//! Shared environment-variable helpers (config + engine setup).

/// Non-empty env value after trim, or `None`.
pub(crate) fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(nonempty)
}

/// `Some(true/false)` when env is a known truthy/falsey token; else `None`.
pub(crate) fn env_truthy(key: &str) -> Option<bool> {
    let v = std::env::var(key).ok()?;
    let v = v.trim().to_ascii_lowercase();
    if matches!(v.as_str(), "1" | "true" | "yes" | "on") {
        Some(true)
    } else if matches!(v.as_str(), "0" | "false" | "no" | "off") {
        Some(false)
    } else {
        None
    }
}

/// Env is one of `0` / `false` / `no` / `off` (case-insensitive).
pub(crate) fn env_flag_false(key: &str) -> bool {
    env_truthy(key) == Some(false)
}

pub(crate) fn nonempty(s: String) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonempty_trims_and_rejects_blank() {
        assert_eq!(nonempty("  hi  ".into()).as_deref(), Some("hi"));
        assert_eq!(nonempty("".into()), None);
        assert_eq!(nonempty("   ".into()), None);
    }
}
