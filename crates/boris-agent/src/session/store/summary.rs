//! On-disk `summary.json` / `current.json` pure shapes.

use chrono::{SecondsFormat, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::session::types::{SessionId, SessionMeta, SessionStatus};

/// Pointer file at `{sessions_root}/current.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct CurrentFile {
    pub session_id: SessionId,
}

/// On-disk Grok-like `summary.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SummaryFile {
    pub info: SummaryInfo,
    #[serde(default)]
    pub session_summary: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub num_messages: u64,
    #[serde(default)]
    pub num_chat_messages: u64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boris_home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SummaryInfo {
    pub id: String,
    #[serde(default)]
    pub cwd: String,
}

impl SummaryFile {
    pub(super) fn from_meta(meta: &SessionMeta, num_messages: u64) -> Self {
        let status = match meta.status {
            SessionStatus::Active => "active",
            SessionStatus::Ended => "ended",
        };
        Self {
            info: SummaryInfo {
                id: meta.id.as_str().to_string(),
                cwd: "desktop".into(),
            },
            session_summary: meta.title.clone(),
            created_at: ms_to_rfc3339(meta.created_at_unix_ms),
            updated_at: ms_to_rfc3339(meta.updated_at_unix_ms),
            num_messages,
            num_chat_messages: num_messages,
            status: status.into(),
            title: meta.title.clone(),
            current_model_id: None,
            boris_home: default_boris_home(),
            cwd: Some("desktop".into()),
        }
    }

    pub(super) fn to_meta(&self) -> SessionMeta {
        let status = match self.status.as_str() {
            "ended" => SessionStatus::Ended,
            _ => SessionStatus::Active,
        };
        SessionMeta {
            id: SessionId::from(self.info.id.as_str()),
            status,
            created_at_unix_ms: rfc3339_to_ms(&self.created_at),
            updated_at_unix_ms: rfc3339_to_ms(&self.updated_at),
            title: if self.title.is_empty() {
                self.session_summary.clone()
            } else {
                self.title.clone()
            },
        }
    }
}

/// Best-effort `~/.boris` path for summary metadata.
fn default_boris_home() -> Option<String> {
    std::env::var("BORIS_HOME").ok().or_else(|| {
        std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .ok()
            .map(|h| format!("{h}/.boris").replace('\\', "/"))
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::types::generate_session_id;

    #[test]
    fn summary_from_meta_roundtrips_status_and_id() {
        let id = generate_session_id();
        let mut meta = SessionMeta::new_active(id.clone());
        meta.title = "hello".into();
        let file = SummaryFile::from_meta(&meta, 3);
        assert_eq!(file.status, "active");
        assert_eq!(file.num_messages, 3);
        assert_eq!(file.title, "hello");
        assert_eq!(file.info.id, id.as_str());

        let back = file.to_meta();
        assert_eq!(back.id, id);
        assert_eq!(back.status, SessionStatus::Active);
        assert_eq!(back.title, "hello");
    }

    #[test]
    fn summary_ended_status() {
        let mut meta = SessionMeta::new_active(generate_session_id());
        meta.end();
        let file = SummaryFile::from_meta(&meta, 0);
        assert_eq!(file.status, "ended");
        assert_eq!(file.to_meta().status, SessionStatus::Ended);
    }

    #[test]
    fn empty_title_falls_back_to_session_summary() {
        let file = SummaryFile {
            info: SummaryInfo {
                id: "abc".into(),
                cwd: "desktop".into(),
            },
            session_summary: "from summary".into(),
            created_at: ms_to_rfc3339(1_000),
            updated_at: ms_to_rfc3339(2_000),
            num_messages: 0,
            num_chat_messages: 0,
            status: "active".into(),
            title: String::new(),
            current_model_id: None,
            boris_home: None,
            cwd: None,
        };
        assert_eq!(file.to_meta().title, "from summary");
    }

    #[test]
    fn current_file_serde() {
        let id = SessionId::from("019f4633-5c50-7293-a20d-62505015e1a4");
        let cur = CurrentFile {
            session_id: id.clone(),
        };
        let json = serde_json::to_string(&cur).unwrap();
        let back: CurrentFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_id, id);
    }
}
