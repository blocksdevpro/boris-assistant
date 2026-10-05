//! Function names shared by provider history and agent tool dispatch.

use serde_json::Value;

/// Whether a name matches the provider pattern `^[a-zA-Z0-9_-]+$`.
pub fn is_valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Remove the model's `functions.` namespace only when its suffix is a valid
/// plain tool name. Other malformed names stay unchanged for validation;
/// replacing arbitrary punctuation could route a call to a different tool.
pub fn canonical_tool_name(name: &str) -> &str {
    name.strip_prefix("functions.")
        .filter(|suffix| is_valid_tool_name(suffix))
        .unwrap_or(name)
}

/// Normalize actual tool-call names without touching content, arguments, IDs,
/// or optional participant names. Used before dispatch and when replaying
/// older history, including calls to tools no longer in the active listing.
pub fn normalize_tool_call_names(message: &mut Value) {
    let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) else {
        return;
    };
    for call in calls {
        let Some(value) = call.pointer_mut("/function/name") else {
            continue;
        };
        if let Some(name) = value.as_str() {
            let canonical = canonical_tool_name(name);
            if canonical != name {
                *value = Value::String(canonical.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonicalization_is_specific_and_idempotent() {
        for (name, expected) in [
            ("functions.web_search", "web_search"),
            ("functions.server__read-v2", "server__read-v2"),
            ("functions.absent", "absent"),
            ("web_search", "web_search"),
            ("other.web_search", "other.web_search"),
            ("functions.", "functions."),
            ("functions.web.search", "functions.web.search"),
            (
                "functions.functions.web_search",
                "functions.functions.web_search",
            ),
            ("functions.web_search ", "functions.web_search "),
        ] {
            assert_eq!(canonical_tool_name(name), expected);
            assert_eq!(canonical_tool_name(expected), expected);
        }
    }

    #[test]
    fn names_allow_only_ascii_letters_digits_underscores_and_hyphens() {
        for name in ["web_search", "Read-2", "mcp__get", "0", "_"] {
            assert!(is_valid_tool_name(name), "{name:?}");
        }
        for name in [
            "",
            " ",
            "web.search",
            "web/search",
            "web:search",
            "read\n",
            "réad",
        ] {
            assert!(!is_valid_tool_name(name), "{name:?}");
        }
    }

    #[test]
    fn normalization_changes_only_call_names() {
        let mut message = json!({
            "role":"assistant", "name":"participant", "content":[{"type":"text", "text":"functions.web_search"}],
            "tool_calls":[
                {"id":"c1", "type":"function", "function":{"name":"functions.web_search", "arguments":"{\"name\":\"functions.web_fetch\"}"}},
                {"id":"c2", "function":{"name":"other.web_search", "arguments":"{}"}},
                {"id":"placeholder"}
            ]
        });
        let mut expected = message.clone();
        expected["tool_calls"][0]["function"]["name"] = json!("web_search");
        normalize_tool_call_names(&mut message);
        assert_eq!(message, expected);
        normalize_tool_call_names(&mut message);
        assert_eq!(message, expected);
    }
}
