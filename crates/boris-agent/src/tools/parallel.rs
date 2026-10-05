//! Model-facing batch envelope. The loop expands it before runtime dispatch.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tool::{Tool, ToolError, ToolMeta};
use crate::tool_context::ToolCallContext;

pub(crate) const NAME: &str = "parallel";
pub(crate) const MAX_TOOL_USES: usize = 32;

/// A scheduling interface, never an executor with its own permissions.
pub struct ParallelTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParallelArgs {
    tool_uses: Vec<ToolUse>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolUse {
    pub recipient_name: String,
    pub parameters: serde_json::Map<String, Value>,
}

pub(crate) fn parse_tool_uses(args: Value) -> Result<Vec<ToolUse>, ToolError> {
    let mut args: ParallelArgs = serde_json::from_value(args).map_err(|error| {
        ToolError::invalid_args(format!(
            "parallel expects tool_uses: [{{recipient_name: tool name, parameters: object}}]: {error}"
        ))
    })?;
    if args.tool_uses.is_empty() || args.tool_uses.len() > MAX_TOOL_USES {
        return Err(ToolError::invalid_args(format!(
            "parallel requires 1–{MAX_TOOL_USES} tool_uses; split larger batches"
        )));
    }
    for call in &mut args.tool_uses {
        call.recipient_name = boris_ai::canonical_tool_name(&call.recipient_name).to_owned();
        if !boris_ai::is_valid_tool_name(&call.recipient_name) {
            return Err(ToolError::invalid_args(
                "recipient_name must match ^[a-zA-Z0-9_-]+$; use the exact listed tool name",
            ));
        }
        if call.recipient_name == NAME {
            return Err(ToolError::invalid_args(
                "Do not nest parallel calls. Put independent leaf tools in one tool_uses array",
            ));
        }
    }
    Ok(args.tool_uses)
}

#[async_trait]
impl Tool for ParallelTool {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        "Run independent tools in one round. Prefer this for two or more independent reads/searches. \
         Each recipient_name is a listed tool's exact name; parameters is its ordinary argument object. \
         Do not include calls that need another call's result or nest parallel. \
         The host expands children into native tool_calls: bounded parallel reads, ordered writes, \
         per-tool approvals and errors. Native multi-tool_calls are also supported."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tool_uses": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_TOOL_USES,
                    "items": {
                        "type": "object",
                        "properties": {
                            "recipient_name": {"type": "string", "pattern": "^[a-zA-Z0-9_-]+$", "description": "Exact listed tool name, e.g. grep or file_read"},
                            "parameters": {"type": "object", "description": "That tool's ordinary arguments"}
                        },
                        "required": ["recipient_name", "parameters"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["tool_uses"],
            "additionalProperties": false
        })
    }

    fn meta(&self) -> ToolMeta {
        // The envelope itself cannot execute children; expansion uses each
        // child's metadata. Read-only child registries can expose it too.
        ToolMeta::safe_default().read_only(true).max_concurrency(1)
    }

    async fn execute(&self, _: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        parse_tool_uses(args)?;
        // Direct runtime invocation cannot dispatch nested calls outside the loop.
        Err(ToolError::invalid_args(
            "parallel must be dispatched by the agent loop; use native tool_calls when calling the runtime directly",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_oversized_nested_and_malformed_envelopes() {
        let leaf = json!({"recipient_name":"get_time", "parameters":{}});
        for args in [
            json!({"tool_uses":[]}),
            json!({"tool_uses":vec![leaf.clone(); MAX_TOOL_USES + 1]}),
            json!({"tool_uses":[{"recipient_name":"parallel", "parameters":{}}]}),
            json!({"tool_uses":[{"recipient_name":"functions.parallel", "parameters":{}}]}),
            json!({"tool_uses":[{"recipient_name":"", "parameters":{}}]}),
            json!({"tool_uses":[{"recipient_name":"file_read", "parameters":"{}"}]}),
            json!({"tool_uses":[leaf], "skip_confirmation":true}),
        ] {
            assert!(parse_tool_uses(args).is_err());
        }
    }

    #[test]
    fn malformed_names_reject_the_whole_envelope() {
        for name in [
            "functions.",
            "functions.web.search",
            "other.web_search",
            "web/search",
            "web search",
            "réad",
        ] {
            let error = parse_tool_uses(json!({"tool_uses":[
                {"recipient_name":"web_search", "parameters":{"query":"example"}},
                {"recipient_name":name, "parameters":{}}
            ]}))
            .err()
            .unwrap();
            assert!(error.to_string().contains("recipient_name"));
        }
    }
}
