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
         already in your tool list. Returns names and required parameters."
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

        let already: HashSet<String> = self.activated.lock().map(|g| g.clone()).unwrap_or_default();

        let mut scored: Vec<(u32, Arc<dyn Tool>)> = Vec::new();
        for tool in snapshot {
            if tool.name() == "tool_search" {
                continue;
            }
            let score = score_tool(tool.as_ref(), &query);
            if score == 0 {
                continue;
            }
            // Prefer tools not already activated (slight boost for discovery).
            let boost = if already.contains(tool.name()) { 0 } else { 1 };
            scored.push((score + boost, tool));
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name().cmp(b.1.name())));
        scored.truncate(limit);

        if scored.is_empty() {
            return Ok(format!(
                "No tools matched query {query:?}. Try: files, web, shell, clipboard, memory."
            ));
        }

        let names: Vec<String> = scored.iter().map(|(_, t)| t.name().to_string()).collect();
        activate_tools(&self.activated, names.iter().cloned());

        let mut lines = Vec::new();
        lines.push(format!(
            "Activated {} tool(s) for this session (available next round):",
            scored.len()
        ));
        for (_, tool) in &scored {
            let req = required_param_summary(tool.as_ref());
            lines.push(format!(
                "- {} — {}{}",
                tool.name(),
                short_desc(tool.description()),
                req
            ));
        }
        lines.push(
            "Call these tools by name on the next step; full schemas will appear in your tool list."
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
