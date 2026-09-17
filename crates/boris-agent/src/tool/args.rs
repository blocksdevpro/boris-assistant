//! JSON argument helpers for tool authors.

use serde_json::{Map, Value};

use super::error::ToolError;

/// Require `args` to be a JSON object; return the map or [`ToolError::invalid_args`].
pub fn require_object(args: &Value) -> Result<&Map<String, Value>, ToolError> {
    args.as_object().ok_or_else(|| {
        ToolError::invalid_args(format!(
            "tool args must be a JSON object, got {}",
            value_type_name(args)
        ))
    })
}

/// Optional string field: `None` if missing or not a JSON string.
pub fn optional_string(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// Required string field; error if missing or not a JSON string.
pub fn require_string(obj: &Map<String, Value>, key: &str) -> Result<String, ToolError> {
    match obj.get(key) {
        None => Err(ToolError::invalid_args(format!(
            "missing required string argument `{key}`"
        ))),
        Some(v) => match v.as_str() {
            Some(s) => Ok(s.to_string()),
            None => Err(ToolError::invalid_args(format!(
                "argument `{key}` must be a string, got {}",
                value_type_name(v)
            ))),
        },
    }
}

/// Optional number coerced to `u64` (JSON number only).
pub fn optional_u64(obj: &Map<String, Value>, key: &str) -> Option<u64> {
    obj.get(key).and_then(coerce_u64)
}

/// Strict integer field: `None` when missing/`null`, `Ok(Some)` for JSON
/// integers and numeric strings, `Err(invalid_args)` for floats and other
/// shapes (never a silent default).
///
/// Use for strict `integer` schema fields (`offset`/`limit`/`timeout`/…).
/// Lenient grep aliases (`-B`/`-A`/`-C`/…) keep [`coerce_u64`].
pub fn optional_u64_strict(obj: &Map<String, Value>, key: &str) -> Result<Option<u64>, ToolError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => strict_u64(v, key).map(Some),
    }
}

/// First present key among `keys` parsed with [`optional_u64_strict`] rules.
///
/// Missing/`null` keys are skipped. The first present non-null value decides:
/// integers and numeric strings succeed, floats and other shapes return
/// `Err(invalid_args)` instead of silently falling back to a default.
pub fn optional_u64_keys_strict(
    obj: &Map<String, Value>,
    keys: &[&str],
) -> Result<Option<u64>, ToolError> {
    for k in keys {
        match obj.get(*k) {
            None | Some(Value::Null) => continue,
            Some(v) => return strict_u64(v, k).map(Some),
        }
    }
    Ok(None)
}

/// Strict JSON → u64: integers and integer strings only.
///
/// Floats (even `12.0`) are rejected with [`ToolError::invalid_args`] so a
/// model typo cannot silently become the default limit/timeout. Negative
/// integers, booleans, arrays, objects, and non-numeric strings are also
/// rejected.
pub fn strict_u64(v: &Value, field: &str) -> Result<u64, ToolError> {
    if let Some(n) = v.as_u64() {
        return Ok(n);
    }
    if let Some(i) = v.as_i64() {
        if i >= 0 {
            if let Ok(n) = u64::try_from(i) {
                return Ok(n);
            }
        }
        return Err(ToolError::invalid_args(format!(
            "argument `{field}` must be a non-negative integer, got {i}"
        )));
    }
    if v.is_number() {
        return Err(ToolError::invalid_args(format!(
            "argument `{field}` must be an integer, got float; pass an integer (e.g. 3) not 3.0"
        )));
    }
    if let Some(s) = v.as_str() {
        let t = s.trim();
        if let Ok(n) = t.parse::<u64>() {
            return Ok(n);
        }
        // Reject float-looking strings explicitly rather than defaulting.
        if t.parse::<f64>().is_ok() {
            return Err(ToolError::invalid_args(format!(
                "argument `{field}` must be an integer, got float string `{t}`"
            )));
        }
        return Err(ToolError::invalid_args(format!(
            "argument `{field}` must be an integer, got string `{t}`"
        )));
    }
    Err(ToolError::invalid_args(format!(
        "argument `{field}` must be an integer, got {}",
        value_type_name(v)
    )))
}

/// Optional bool.
pub fn optional_bool(obj: &Map<String, Value>, key: &str) -> Option<bool> {
    obj.get(key).and_then(coerce_bool)
}

/// First present string among `keys` (models alias Grok-style names).
pub fn optional_string_keys(obj: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| optional_string(obj, k))
}

/// First present number among `keys`, accepting ints, floats, and numeric strings.
pub fn optional_u64_keys(obj: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|k| obj.get(*k).and_then(coerce_u64))
}

/// First present bool among `keys`, accepting true/false, 1/0, and "true"/"false".
pub fn optional_bool_keys(obj: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|k| obj.get(*k).and_then(coerce_bool))
}

/// Lenient JSON → u64 (number, numeric string, truncated float).
///
/// Truncates floats (`12.0` → `12`, `12.9` → `12`). Intended for lenient grep
/// alias fields (`-B`/`-A`/`-C`/`head_limit`/…) whose schemas omit `type` so
/// strings and floats reach this path. Strict `integer` fields must use
/// [`strict_u64`] / [`optional_u64_strict`] instead so floats become
/// `invalid_args` rather than a silent default or truncation.
pub fn coerce_u64(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return Some(n);
    }
    if let Some(i) = v.as_i64() {
        return u64::try_from(i).ok();
    }
    if let Some(f) = v.as_f64() {
        if f.is_finite() && f >= 0.0 {
            return Some(f as u64);
        }
    }
    v.as_str().and_then(|s| s.trim().parse().ok())
}

/// Lenient JSON → bool (bool, 0/1, "true"/"false"/"yes"/"no").
pub fn coerce_bool(v: &Value) -> Option<bool> {
    if let Some(b) = v.as_bool() {
        return Some(b);
    }
    if let Some(n) = v.as_i64() {
        return match n {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
    }
    v.as_str()
        .map(|s| s.trim().to_ascii_lowercase())
        .and_then(|s| match s.as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        })
}

/// JSON type name for error messages.
pub fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolErrorKind;
    use serde_json::json;

    #[test]
    fn require_string_ok() {
        let obj = json!({ "name": "boris", "n": 1 })
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(require_string(&obj, "name").unwrap(), "boris");
    }

    #[test]
    fn require_string_missing() {
        let obj = Map::new();
        let err = require_string(&obj, "name").unwrap_err();
        assert_eq!(err.kind(), ToolErrorKind::InvalidArgs);
        assert!(err.message.contains("missing"));
        assert!(err.message.contains("name"));
    }

    #[test]
    fn require_string_wrong_type() {
        let obj = json!({ "name": 42 }).as_object().unwrap().clone();
        let err = require_string(&obj, "name").unwrap_err();
        assert_eq!(err.kind(), ToolErrorKind::InvalidArgs);
        assert!(err.message.contains("string"));
    }

    #[test]
    fn optional_string_behaviour() {
        let obj = json!({ "a": "yes", "b": 1 }).as_object().unwrap().clone();
        assert_eq!(optional_string(&obj, "a").as_deref(), Some("yes"));
        assert_eq!(optional_string(&obj, "b"), None);
        assert_eq!(optional_string(&obj, "missing"), None);
    }

    #[test]
    fn require_object_ok_and_err() {
        let v = json!({ "x": 1 });
        let map = require_object(&v).unwrap();
        assert_eq!(map.get("x").and_then(|n| n.as_i64()), Some(1));

        let err = require_object(&json!([])).unwrap_err();
        assert_eq!(err.kind(), ToolErrorKind::InvalidArgs);
    }

    #[test]
    fn optional_u64_and_bool() {
        let obj = json!({ "n": 3, "b": true }).as_object().unwrap().clone();
        assert_eq!(optional_u64(&obj, "n"), Some(3));
        assert_eq!(optional_bool(&obj, "b"), Some(true));
        assert_eq!(optional_u64(&obj, "missing"), None);
    }

    #[test]
    fn lenient_coercion_and_aliases() {
        let obj = json!({
            "-i": "true",
            "-C": "3",
            "head_limit": 12.0,
            "output_mode": "files_with_matches"
        })
        .as_object()
        .unwrap()
        .clone();
        assert_eq!(optional_bool_keys(&obj, &["ignore_case", "-i"]), Some(true));
        assert_eq!(optional_u64_keys(&obj, &["context", "-C"]), Some(3));
        assert_eq!(optional_u64_keys(&obj, &["head_limit", "limit"]), Some(12));
        assert_eq!(
            optional_string_keys(&obj, &["output_mode"]).as_deref(),
            Some("files_with_matches")
        );
        assert_eq!(coerce_bool(&json!(1)), Some(true));
        assert_eq!(coerce_bool(&json!("no")), Some(false));
    }

    #[test]
    fn strict_u64_accepts_ints_and_numeric_strings() {
        assert_eq!(strict_u64(&json!(3), "limit").unwrap(), 3);
        assert_eq!(strict_u64(&json!("42"), "limit").unwrap(), 42);
        assert_eq!(strict_u64(&json!("  7  "), "limit").unwrap(), 7);
        let obj = json!({"limit": "3"}).as_object().cloned().unwrap();
        assert_eq!(optional_u64_strict(&obj, "limit").unwrap(), Some(3));
        let obj = json!({"a": 5, "b": 9}).as_object().cloned().unwrap();
        assert_eq!(
            optional_u64_keys_strict(&obj, &["missing", "a", "b"]).unwrap(),
            Some(5)
        );
        let empty = Map::new();
        assert_eq!(optional_u64_strict(&empty, "limit").unwrap(), None);
    }

    #[test]
    fn strict_u64_rejects_floats_with_invalid_args() {
        for v in [json!(12.0), json!(3.14), json!("12.0"), json!(1.5)] {
            let err = strict_u64(&v, "limit").unwrap_err();
            assert_eq!(err.kind(), ToolErrorKind::InvalidArgs);
            assert!(err.message.contains("integer"), "got: {}", err.message);
        }
        let obj = json!({"limit": 12.0}).as_object().cloned().unwrap();
        let err = optional_u64_strict(&obj, "limit").unwrap_err();
        assert_eq!(err.kind(), ToolErrorKind::InvalidArgs);
        let obj = json!({"limit": 12.0}).as_object().cloned().unwrap();
        let err = optional_u64_keys_strict(&obj, &["limit"]).unwrap_err();
        assert_eq!(err.kind(), ToolErrorKind::InvalidArgs);
        // Negative and wrong shapes also reject (never silent default).
        for v in [json!(-1), json!(true), json!("nope"), json!([1])] {
            assert!(strict_u64(&v, "limit").is_err(), "got ok for {v}");
        }
    }
}
