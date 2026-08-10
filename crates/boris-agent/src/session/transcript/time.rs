//! Timestamp helpers for transcript wire (`ts` RFC3339).

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{SecondsFormat, TimeZone, Utc};

pub(super) fn ms_to_rfc3339(ms: u64) -> String {
    match Utc.timestamp_millis_opt(ms as i64) {
        chrono::LocalResult::Single(dt) => dt.to_rfc3339_opts(SecondsFormat::Millis, true),
        _ => Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    }
}

pub(super) fn rfc3339_to_ms(s: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.timestamp_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_rfc3339_roundtrip() {
        let ms = 1_700_000_000_000u64;
        let s = ms_to_rfc3339(ms);
        assert!(s.contains('T'));
        assert_eq!(rfc3339_to_ms(&s), ms);
    }

    #[test]
    fn invalid_rfc3339_yields_zero() {
        assert_eq!(rfc3339_to_ms("not-a-date"), 0);
    }
}
