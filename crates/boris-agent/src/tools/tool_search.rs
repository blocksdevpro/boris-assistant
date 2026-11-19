//! Discovery meta-tool for extension tools (MCP servers / plugins).
//!
//! Every builtin is already listed on every request, so discovery only exists
//! for extensions. Hits include their parameter schemas: the model can call a
//! found tool in its very next response (the loop executes any registered
//! tool, schema-validated, whether or not it is listed yet). New hits are also
//! activated so they appear in the tool list from the next request.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::runtime::{activate_tools, ActivationSet};
use crate::tool::{
    require_object, require_string, Permission, Tool, ToolError, ToolKind, ToolMeta, ToolRisk,
};
use crate::tool_context::ToolCallContext;

const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 16;
/// Largest inline parameter schema per hit. Bigger schemas are listed from the
/// next request instead of being truncated into an unusable fragment.
const MAX_INLINE_SCHEMA_CHARS: usize = 4_000;

/// Live tool registry mirror owned by [`crate::Agent`].
pub type SharedToolRegistry = Arc<Mutex<Vec<Arc<dyn Tool>>>>;

/// Search extension tools by name/description/kind and return callable schemas.
pub struct ToolSearchTool {
    tools: SharedToolRegistry,
    activated: ActivationSet,
}

impl ToolSearchTool {
    pub fn new(tools: SharedToolRegistry, activated: ActivationSet) -> Self {
        Self { tools, activated }
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        crate::runtime::TOOL_SEARCH_NAME
    }

    fn description(&self) -> &str {
        "Search optional extension tools (MCP servers, plugins) by keyword. Every built-in \
         tool is already in your tool list; do not search for those. Results include each \
         extension's parameter schema, so you can call a found tool directly in your next \
         response. Keyword matches do not prove capability; check descriptions. If nothing \
         fits, report the capability unavailable instead of searching again."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Short search query (e.g. \"github issues\", \"calendar\")"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (default 8, max 16)"
                }
            },
            "required": ["query"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Search)
            .permissions(&[Permission::None])
            .read_only(true)
            .max_concurrency(1)
    }

    async fn execute(&self, _ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        let started = std::time::Instant::now();
        let obj = require_object(&args)?;
        let query = require_string(obj, "query")?;
        let query = query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return Err(ToolError::invalid_args("query is empty"));
        }
        let limit = obj
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LIMIT as u64)
            .clamp(1, MAX_LIMIT as u64) as usize;

        // Snapshot registry without holding lock across await.
        let snapshot: Vec<Arc<dyn Tool>> = self
            .tools
            .lock()
            .map_err(|_| ToolError::failed("tool registry lock poisoned"))?
            .clone();

        let listed: HashSet<String> = self
            .activated
            .lock()
            .map(|g| g.listed().clone())
            .unwrap_or_default();

        let mut scored: Vec<(u32, Arc<dyn Tool>)> = Vec::new();
        for tool in snapshot {
            if tool.name() == crate::runtime::TOOL_SEARCH_NAME {
                continue;
            }
            let score = score_tool(tool.as_ref(), &query);
            if score == 0 {
                continue;
            }
            // Extensions are what discovery is for; rank them above builtins.
            let boost = if tool.is_extension() { 1_000 } else { 0 };
            scored.push((score + boost, tool));
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name().cmp(b.1.name())));
        scored.truncate(limit);

        if scored.is_empty() {
            return Ok(format!(
                "No tools matched query {query:?}. Built-in tools are already listed; \
                 report the capability unavailable unless a specific new lead warrants another query."
            ));
        }

        let new_extensions: Vec<String> = scored
            .iter()
            .filter(|(_, tool)| tool.is_extension() && !listed.contains(tool.name()))
            .map(|(_, tool)| tool.name().to_string())
            .collect();
        activate_tools(&self.activated, new_extensions.iter().cloned());
        let extensions = scored.iter().filter(|(_, t)| t.is_extension()).count();
        tracing::info!(query = %query, matches = scored.len(), extensions,
            newly_activated = new_extensions.len(), duration_ms = started.elapsed().as_millis() as u64,
            "tool discovery");

        let mut lines = vec![format!(
            "{} matching tool(s), {} extension(s):",
            scored.len(),
            extensions
        )];
        for (_, tool) in &scored {
            let name = tool.name();
            if !tool.is_extension() {
                lines.push(format!(
                    "- {name} [built-in, already in your tool list] — {}",
                    short_desc(tool.description())
                ));
                continue;
            }
            lines.push(format!(
                "- {name} [extension, callable now] — {}",
                short_desc(tool.description())
            ));
            let schema = tool.parameters().to_string();
            if schema.chars().count() <= MAX_INLINE_SCHEMA_CHARS {
                lines.push(format!("  parameters: {schema}"));
            } else {
                lines.push(format!(
                    "  parameters: schema too large to inline{}; it is in your tool list from the next request",
                    required_param_summary(tool.as_ref())
                ));
            }
        }
        lines.push(
            "Call a found extension directly with arguments that match its parameters. \
             Check descriptions, not just keyword hits; if none performs the needed action, \
             report it unavailable."
                .into(),
        );
        Ok(lines.join("\n"))
    }
}

fn score_tool(tool: &dyn Tool, query: &str) -> u32 {
    let name = tool.name().to_ascii_lowercase();
    let desc = tool.description().to_ascii_lowercase();
    let kind = tool.meta().kind.as_str();
    let mut score = 0u32;
    if name == query {
        score += 100;
    } else if name.contains(query) {
        score += 50;
    }
    for token in query.split_whitespace() {
        if token.is_empty() {
            continue;
        }
        if name.contains(token) {
            score += 20;
        }
        if desc.contains(token) {
            score += 10;
        }
        if kind.contains(token) {
            score += 15;
        }
    }
    score
}

fn short_desc(s: &str) -> String {
    let one = s.lines().next().unwrap_or(s).trim();
    let count = one.chars().count();
    if count <= 160 {
        one.to_string()
    } else {
        format!("{}…", one.chars().take(159).collect::<String>())
    }
}

fn required_param_summary(tool: &dyn Tool) -> String {
    let params = tool.parameters();
    let required = params
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    if required.is_empty() {
        return String::new();
    }
    let props = params.get("properties").and_then(|p| p.as_object());
    let mut parts = Vec::new();
    for name in required {
        let ty = props
            .and_then(|p| p.get(name))
            .and_then(|s| s.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("any");
        parts.push(format!("{name}: {ty}"));
    }
    format!(" (required: {})", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NamedTool {
        name: &'static str,
        extension: bool,
    }
    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "file tools"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object", "properties":{"path":{"type":"string"}}, "required":["path"]})
        }
        fn is_extension(&self) -> bool {
            self.extension
        }
        async fn execute(&self, _: &ToolCallContext, _: Value) -> Result<String, ToolError> {
            Ok("ok".into())
        }
    }

    #[tokio::test]
    async fn discovery_returns_callable_extension_schemas_and_skips_builtins() {
        let activated = crate::runtime::new_activation_set();
        activated
            .lock()
            .unwrap()
            .record_listed(["file_read".into()]);
        let registry = Arc::new(Mutex::new(vec![
            Arc::new(NamedTool {
                name: "file_read",
                extension: false,
            }) as Arc<dyn Tool>,
            Arc::new(NamedTool {
                name: "plugin__file_view",
                extension: true,
            }) as Arc<dyn Tool>,
        ]));
        let search = ToolSearchTool::new(registry, activated.clone());
        let out = search
            .execute(&ToolCallContext::new("c"), json!({"query":"file"}))
            .await
            .unwrap();
        assert!(out.contains("2 matching tool(s), 1 extension(s)"));
        // Extensions rank first and carry their full parameter schema.
        let ext = out.find("plugin__file_view [extension, callable now]").unwrap();
        let builtin = out
            .find("file_read [built-in, already in your tool list]")
            .unwrap();
        assert!(ext < builtin);
        let schema_line = out
            .lines()
            .find(|line| line.trim_start().starts_with("parameters: {"))
            .expect("inline schema");
        let schema: Value =
            serde_json::from_str(schema_line.trim_start().trim_start_matches("parameters: "))
                .unwrap();
        assert_eq!(schema["required"], json!(["path"]));
        assert_eq!(schema["properties"]["path"]["type"], "string");
        let mut table = activated.lock().unwrap();
        assert!(!table.contains("file_read"), "builtins never take activation slots");
        assert!(table.contains("plugin__file_view"));
        table.record_listed(["file_read".into(), "plugin__file_view".into()]);
    }
}
