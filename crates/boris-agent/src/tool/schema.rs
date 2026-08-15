//! Centralized JSON Schema argument validation (object subset).
//!
//! Validates the shapes Boris tools actually advertise: `type`, `properties`,
//! `required`, and nested object/array/string/number/integer/boolean. Does
//! **not** coerce malformed JSON or non-objects to `{}`.

use serde_json::Value;

use super::args::value_type_name;

/// Why arguments were rejected. Rendered into a model-repairable observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidArgs {
    pub code: String,
    pub path: String,
    pub expected: String,
    pub message: String,
    pub raw_preview: String,
}

impl InvalidArgs {
    pub fn new(
        code: impl Into<String>,
        path: impl Into<String>,
        expected: impl Into<String>,
        message: impl Into<String>,
        raw_preview: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            path: path.into(),
            expected: expected.into(),
            message: message.into(),
            raw_preview: bound_preview(raw_preview.into()),
        }
    }
}

const PREVIEW_CHARS: usize = 240;

fn bound_preview(raw: String) -> String {
    let count = raw.chars().count();
    if count <= PREVIEW_CHARS {
        raw
    } else {
        let head: String = raw.chars().take(PREVIEW_CHARS.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Validate `args` against a JSON Schema object. `raw_preview` is shown to the model.
pub fn validate_args(schema: &Value, args: &Value, raw_preview: &str) -> Result<(), InvalidArgs> {
    if !args.is_object() {
        return Err(InvalidArgs::new(
            "not_object",
            "$",
            "object",
            format!(
                "tool args must be a JSON object, got {}",
                value_type_name(args)
            ),
            raw_preview,
        ));
    }
    validate_value(schema, args, "$", raw_preview)
}

fn validate_value(
    schema: &Value,
    value: &Value,
    path: &str,
    raw_preview: &str,
) -> Result<(), InvalidArgs> {
    let Some(expected_type) = schema.get("type").and_then(|t| t.as_str()) else {
        // No type constraint — accept.
        return Ok(());
    };

    if !type_matches(expected_type, value) {
        return Err(InvalidArgs::new(
            "type_mismatch",
            path,
            expected_type,
            format!(
                "at `{path}`: expected {expected_type}, got {}",
                value_type_name(value)
            ),
            raw_preview,
        ));
    }

    match expected_type {
        "object" => validate_object(schema, value, path, raw_preview),
        "array" => validate_array(schema, value, path, raw_preview),
        _ => Ok(()),
    }
}

fn type_matches(expected: &str, value: &Value) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn validate_object(
    schema: &Value,
    value: &Value,
    path: &str,
    raw_preview: &str,
) -> Result<(), InvalidArgs> {
    let obj = value.as_object().expect("type checked");
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for key in required {
            let Some(name) = key.as_str() else {
                continue;
            };
            if !obj.contains_key(name) {
                let child = format!("{path}.{name}");
                return Err(InvalidArgs::new(
                    "missing_required",
                    child,
                    "present",
                    format!("missing required argument `{name}`"),
                    raw_preview,
                ));
            }
        }
    }
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return Ok(());
    };
    for (key, prop_schema) in props {
        if let Some(child) = obj.get(key) {
            let child_path = format!("{path}.{key}");
            validate_value(prop_schema, child, &child_path, raw_preview)?;
        }
    }
    Ok(())
}

fn validate_array(
    schema: &Value,
    value: &Value,
    path: &str,
    raw_preview: &str,
) -> Result<(), InvalidArgs> {
    let arr = value.as_array().expect("type checked");
    let Some(item_schema) = schema.get("items") else {
        return Ok(());
    };
    for (i, item) in arr.iter().enumerate() {
        let child_path = format!("{path}[{i}]");
        validate_value(item_schema, item, &child_path, raw_preview)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "n": { "type": "integer" }
            },
            "required": ["name"]
        })
    }

    #[test]
    fn accepts_valid_object() {
        assert!(validate_args(&schema(), &json!({"name": "boris", "n": 1}), "{}").is_ok());
    }

    #[test]
    fn rejects_non_object() {
        let err = validate_args(&schema(), &json!([]), "[]").unwrap_err();
        assert_eq!(err.code, "not_object");
        assert_eq!(err.path, "$");
    }

    #[test]
    fn rejects_missing_required() {
        let err = validate_args(&schema(), &json!({"n": 1}), "{\"n\":1}").unwrap_err();
        assert_eq!(err.code, "missing_required");
        assert!(err.path.contains("name"));
    }

    #[test]
    fn rejects_wrong_field_type() {
        let err = validate_args(&schema(), &json!({"name": 3}), "{\"name\":3}").unwrap_err();
        assert_eq!(err.code, "type_mismatch");
        assert!(err.path.contains("name"));
        assert_eq!(err.expected, "string");
    }

    #[test]
    fn bounds_raw_preview() {
        let long = "x".repeat(500);
        let err = validate_args(&schema(), &json!(1), &long).unwrap_err();
        assert!(err.raw_preview.chars().count() <= PREVIEW_CHARS);
    }
}
