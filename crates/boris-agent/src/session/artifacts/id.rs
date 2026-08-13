//! Short hex artifact ids (filename + catalog key).

use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::session::types::now_unix_ms;

/// Hex characters in a generated artifact id (`a1f3c9`).
pub const ARTIFACT_ID_LEN: usize = 6;

/// True when `s` is a lowercase hex id of [`ARTIFACT_ID_LEN`].
pub fn is_artifact_id(s: &str) -> bool {
    s.len() == ARTIFACT_ID_LEN && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Normalize a model-supplied id: lowercase hex of the right length.
pub fn normalize_artifact_id(s: &str) -> Option<String> {
    let t = s.trim().to_ascii_lowercase();
    if t.len() == ARTIFACT_ID_LEN && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Some(t)
    } else {
        None
    }
}

/// Generate a 6-char lowercase hex id (wall clock + pid + counter).
pub fn generate_artifact_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = now_unix_ms();
    let pid = process::id() as u64;
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed = t
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(pid.wrapping_shl(16))
        .wrapping_add(n.wrapping_mul(0x1656_67B1));
    format!("{:06x}", mixed & 0x00FF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generated_ids_are_six_lowercase_hex() {
        let id = generate_artifact_id();
        assert_eq!(id.len(), ARTIFACT_ID_LEN);
        assert!(normalize_artifact_id(&id).as_deref() == Some(id.as_str()));
    }

    #[test]
    fn generated_ids_are_unique() {
        let mut seen = HashSet::new();
        for _ in 0..32 {
            assert!(seen.insert(generate_artifact_id()));
        }
    }

    #[test]
    fn normalize_accepts_mixed_case() {
        assert_eq!(normalize_artifact_id("A1F3C9").as_deref(), Some("a1f3c9"));
        assert_eq!(normalize_artifact_id("nope"), None);
        assert_eq!(normalize_artifact_id("a1f3c"), None);
    }

    #[test]
    fn is_artifact_id_requires_lowercase() {
        assert!(is_artifact_id("a1f3c9"));
        assert!(!is_artifact_id("A1F3C9"));
        assert!(!is_artifact_id("zzzzzz"));
    }
}
