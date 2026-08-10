//! Pure helpers for notes tool args and observation formatting.

use serde_json::{Map, Value};

use crate::tool::ToolError;

use super::{DEFAULT_RECALL_LIMIT, MAX_RECALL_LIMIT};

/// Parse `limit` from tool args (default 5, hard cap 20).
pub(super) fn parse_limit(obj: &Map<String, Value>) -> Result<usize, ToolError> {
    match obj.get("limit") {
        None | Some(Value::Null) => Ok(DEFAULT_RECALL_LIMIT),
        Some(v) => {
            let n = v
                .as_u64()
                .or_else(|| {
                    v.as_i64()
                        .and_then(|i| if i >= 0 { Some(i as u64) } else { None })
                })
                .or_else(|| {
                    v.as_f64().and_then(|f| {
                        if f.is_finite() && f >= 0.0 {
                            Some(f as u64)
                        } else {
                            None
                        }
                    })
                })
                .ok_or_else(|| {
                    ToolError::invalid_args("argument `limit` must be a non-negative number")
                })?;
            Ok((n as usize).min(MAX_RECALL_LIMIT))
        }
    }
}

/// Bullet list observation for the model.
pub(super) fn format_notes_list(notes: &[String]) -> String {
    if notes.is_empty() {
        return "No notes found.".to_string();
    }
    notes
        .iter()
        .map(|n| format!("- {n}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_empty_and_bullets() {
        assert_eq!(format_notes_list(&[]), "No notes found.");
        assert_eq!(
            format_notes_list(&["a".into(), "b".into()]),
            "- a\n- b"
        );
    }

    #[test]
    fn parse_limit_default_cap_and_reject() {
        let empty = Map::new();
        assert_eq!(parse_limit(&empty).unwrap(), DEFAULT_RECALL_LIMIT);

        let obj = json!({ "limit": 100 }).as_object().cloned().unwrap();
        assert_eq!(parse_limit(&obj).unwrap(), MAX_RECALL_LIMIT);

        let obj = json!({ "limit": 3 }).as_object().cloned().unwrap();
        assert_eq!(parse_limit(&obj).unwrap(), 3);

        let obj = json!({ "limit": "nope" }).as_object().cloned().unwrap();
        assert!(parse_limit(&obj).is_err());
    }
}
