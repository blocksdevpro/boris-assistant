//! Progressive discovery meta-tool: search registered tools and activate them.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::runtime::{activate_tools, ActivationSet, ListToolsContext};
use crate::tool::{
    require_object, require_string, Permission, Tool, ToolError, ToolKind, ToolMeta, ToolRisk,
};
use crate::tool_context::ToolCallContext;

const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 16;

/// Live tool registry mirror owned by [`crate::Agent`].
pub type SharedToolRegistry = Arc<Mutex<Vec<Arc<dyn Tool>>>>;

/// Search tools by name/description/kind and activate hits for this session.
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
        "tool_search"
    }

    fn description(&self) -> &str {
        "Search available tools by keyword (e.g. files, web, shell, clipboard) and \
         activate matches for this session. Call this before using tools that are not \
         already in your tool list. Keyword matches do not prove capability; check descriptions. \
         Do not rediscover listed tools or repeat searches without a specific new lead. \
         Returns names and required parameters, distinguishing current availability from new activation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Short search query (e.g. \"files\", \"web\", \"bash\")"
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

    fn should_list(&self, _ctx: &ListToolsContext) -> bool {
        // Soft-core: always show when progressive is on (also in hard core).
        true
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

        let already: HashSet<String> = self
            .activated
            .lock()
            .map(|g| g.listed().clone())
            .unwrap_or_default();

        let mut scored: Vec<(u32, Arc<dyn Tool>)> = Vec::new();
        for tool in snapshot {
            if tool.name() == "tool_search" {
                continue;
            }
            let score = score_tool(tool.as_ref(), &query);
            if score == 0 {
                continue;
            }
            // Prefer tools absent from the actual request (slight discovery boost).
            let boost = if already.contains(tool.name()) { 0 } else { 1 };
            scored.push((score + boost, tool));
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name().cmp(b.1.name())));
        scored.truncate(limit);

        if scored.is_empty() {
            return Ok(format!(
                "No tools matched query {query:?}. Search again only with a specific new lead; otherwise report the capability unavailable."
            ));
        }

        let names: Vec<String> = scored.iter().map(|(_, t)| t.name().to_string()).collect();
        let new_names: Vec<String> = names
            .iter()
            .filter(|name| !already.contains(*name))
            .cloned()
            .collect();
        activate_tools(&self.activated, new_names.iter().cloned());
        tracing::info!(query = %query, matches = names.len(), newly_activated = new_names.len(),
            already_available = names.len() - new_names.len(), duration_ms = started.elapsed().as_millis() as u64,
            "tool discovery");

        let mut lines = Vec::new();
        lines.push(format!(
            "{} matching tool(s): {} already available in your current tool list; {} newly activated for the next round:",
            scored.len(), scored.len() - new_names.len(), new_names.len()
        ));
        for (_, tool) in &scored {
            let req = required_param_summary(tool.as_ref());
            lines.push(format!(
                "- {} [{}] — {}{}",
                tool.name(),
                if already.contains(tool.name()) {
                    "already available"
                } else {
                    "new activation"
                },
                short_desc(tool.description()),
                req
            ));
        }
        lines.push(
            "Use already-available tools directly. New schemas appear next round within the schema budget. Check descriptions, not just keyword hits; if none performs the needed action, report it unavailable unless a specific new lead warrants another query."
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
    // Aliases
    let aliases: &[(&str, &[&str])] = &[
        (
            "files",
            &["file", "list_dir", "glob", "grep", "read", "write", "edit"],
        ),
        (
            "file",
            &["file_read", "file_write", "file_edit", "list_dir"],
        ),
        ("web", &["web_search", "web_fetch", "http", "url"]),
        ("shell", &["bash", "command", "cmd"]),
        ("bash", &["bash", "shell"]),
        ("clipboard", &["clipboard"]),
        (
            "memory",
            &["memory_search", "memory_get", "remember", "recall"],
        ),
        ("skill", &["list_skills", "load_skill"]),
        (
            "artifact",
            &["present_artifact", "list_artifacts", "get_artifact"],
        ),
        ("card", &["present_artifact", "list_artifacts"]),
        ("show", &["present_artifact"]),
    ];
    for (key, needles) in aliases {
        if query.contains(key) {
            for n in *needles {
                if name.contains(n) || desc.contains(n) {
                    score += 25;
                }
            }
        }
    }
    score
}

fn short_desc(s: &str) -> String {
    let one = s.lines().next().unwrap_or(s).trim();
    let count = one.chars().count();
    if count <= 100 {
        one.to_string()
    } else {
        format!("{}…", one.chars().take(99).collect::<String>())
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
    format!(" [required: {}]", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NamedTool(&'static str);
    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "file tools"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object", "properties":{}})
        }
        async fn execute(&self, _: &ToolCallContext, _: Value) -> Result<String, ToolError> {
            Ok("ok".into())
        }
    }

    #[tokio::test]
    async fn discovery_distinguishes_current_availability_and_does_not_reactivate_core() {
        let activated = crate::runtime::new_activation_set();
        activated
            .lock()
            .unwrap()
            .record_listed(["file_read".into()]);
        let registry = Arc::new(Mutex::new(vec![
            Arc::new(NamedTool("file_read")) as Arc<dyn Tool>,
            Arc::new(NamedTool("plugin_file_view")) as Arc<dyn Tool>,
        ]));
        let search = ToolSearchTool::new(registry, activated.clone());
        let out = search
            .execute(&ToolCallContext::new("c"), json!({"query":"file"}))
            .await
            .unwrap();
        assert!(out.contains("1 already available"));
        assert!(out.contains("1 newly activated"));
        assert!(out.contains("file_read [already available]"));
        let mut table = activated.lock().unwrap();
        assert!(!table.contains("file_read"));
        assert!(table.contains("plugin_file_view"));
        table.record_listed(["file_read".into(), "plugin_file_view".into()]);
        drop(table);
        let out = search
            .execute(&ToolCallContext::new("again"), json!({"query":"file"}))
            .await
            .unwrap();
        assert!(out.contains("0 newly activated"));
    }
}
