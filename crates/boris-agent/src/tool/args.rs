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
    obj.get(key).and_then(|v| v.as_u64())
}

/// Optional bool.
pub fn optional_bool(obj: &Map<String, Value>, key: &str) -> Option<bool> {
    obj.get(key).and_then(|v| v.as_bool())
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
}
