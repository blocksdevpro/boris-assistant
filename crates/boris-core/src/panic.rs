//! Panic payload helpers for threads that catch a panic and report it.

use std::any::Any;

/// Text of a caught panic payload (`panic!("…")` / `panic!("{x}")`).
///
/// Returns `None` for payloads that are neither `&str` nor `String`
/// (e.g. `std::panic::panic_any(42)`), so callers pick their own fallback.
pub fn panic_message(payload: &(dyn Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_static_and_formatted_payloads() {
        let payload = std::panic::catch_unwind(|| panic!("static")).unwrap_err();
        assert_eq!(panic_message(payload.as_ref()), Some("static"));
        let payload = std::panic::catch_unwind(|| panic!("ort {}", 2)).unwrap_err();
        assert_eq!(panic_message(payload.as_ref()), Some("ort 2"));
    }

    #[test]
    fn other_payloads_have_no_text() {
        let payload = std::panic::catch_unwind(|| std::panic::panic_any(42_u8)).unwrap_err();
        assert_eq!(panic_message(payload.as_ref()), None);
    }
}
