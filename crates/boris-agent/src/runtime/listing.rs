//! Progressive tool listing: core + per-turn top-k + LRU/TTL activation.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::task::TaskTraits;
use crate::tool::{Tool, ToolKind};

/// Session activation set mutated by `tool_search` (bounded LRU + TTL).
pub type ActivationSet = Arc<Mutex<ActivationTable>>;

pub fn new_activation_set() -> ActivationSet {
    Arc::new(Mutex::new(ActivationTable::default()))
}

/// Max activated tool names (LRU evicts oldest).
pub const MAX_ACTIVATED: usize = 32;
/// Activations expire so a session-long set cannot keep growing forever.
pub const ACTIVATION_TTL: Duration = Duration::from_secs(15 * 60);
/// Host-side top-k listed on top of the always-on core (when progressive).
pub const DEFAULT_TOP_K: usize = 10;
/// Maximum serialized OpenAI-style tool definitions sent in one request.
///
/// Message compaction cannot make an oversized schema table smaller, so the
/// listing layer also owns a hard request-local ceiling. 64 KiB leaves ample
/// room for useful schemas without allowing a large plugin registry to consume
/// the context window before the conversation starts.
pub const MAX_TOOL_SCHEMA_CHARS: usize = 64 * 1024;

/// One activated tool with last-used timestamp.
#[derive(Debug, Clone)]
pub struct ActivationEntry {
    pub name: String,
    pub last_used: Instant,
}

/// Bounded LRU + TTL activation table (replaces an unbounded session HashSet).
#[derive(Debug, Clone)]
pub struct ActivationTable {
    entries: Vec<ActivationEntry>,
    ttl: Duration,
    max: usize,
}

impl Default for ActivationTable {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            ttl: ACTIVATION_TTL,
            max: MAX_ACTIVATED,
        }
    }
}

impl ActivationTable {
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn snapshot(&mut self) -> HashSet<String> {
        self.evict_expired();
        self.entries.iter().map(|e| e.name.clone()).collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.name == name && e.last_used.elapsed() < self.ttl)
    }

    fn evict_expired(&mut self) {
        self.entries.retain(|e| e.last_used.elapsed() < self.ttl);
    }

    pub fn activate(&mut self, names: impl IntoIterator<Item = String>) {
        let now = Instant::now();
        for name in names {
            if name.is_empty() {
                continue;
            }
            if let Some(pos) = self.entries.iter().position(|e| e.name == name) {
                self.entries.remove(pos);
            }
            self.entries.push(ActivationEntry {
                name,
                last_used: now,
            });
        }
        self.evict_expired();
        while self.entries.len() > self.max {
            self.entries.remove(0);
        }
    }
}

/// Hard-core tool names always listed when progressive is on (if registered).
pub const DEFAULT_CORE_TOOL_NAMES: &[&str] = &[
    "get_time",
    "get_date",
    "remember_note",
    "recall_notes",
    "todo_read",
    "todo_write",
    "get_system_info",
    "get_user_context",
    "tool_search",
];

/// Feature flags for listing / concurrency / progress (owned by [`crate::Agent`]).
#[derive(Debug, Clone)]
pub struct ToolRuntimeFeatures {
    /// Shrink tools_json to core ∪ activated ∪ should_list opt-in.
    pub progressive_listing: bool,
    /// Ignore progressive filter; list everything.
    pub force_list_all: bool,
    /// Partition multi-tool batches into a parallel **read** wave then a sequential
    /// **write** wave (Grok-style fan-out). When false, falls back to legacy
    /// parallel dispatch (chunked by `max_parallel_tools`) for every auto-allowed call.
    ///
    /// (Historically named `concurrency_v2` during the rollout; the name was
    /// just "second concurrency strategy", not a protocol version.)
    pub wave_scheduling: bool,
    /// Global max concurrent tools in the read-only wave (default 16).
    pub max_parallel_tools: u32,
    /// Attach progress sinks (always cheap when tools don't report).
    pub progress_events: bool,
    /// Optional host override for core tool names.
    pub core_tools: Option<Vec<String>>,
}

impl Default for ToolRuntimeFeatures {
    fn default() -> Self {
        Self {
            // Progressive listing is on: core + top-k + tool_search fallback.
            progressive_listing: true,
            force_list_all: false,
            // Parallel reads + sequential writes for multi-tool assistant messages.
            wave_scheduling: true,
            // Enough headroom for multi-grep / multi-read / multi-search in one step.
            max_parallel_tools: 16,
            progress_events: true,
            core_tools: None,
        }
    }
}

/// Per-LLM-round snapshot for listing decisions.
#[derive(Debug, Clone)]
pub struct ListToolsContext {
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    /// Activated names snapshot for this round (immutable).
    pub activated: Arc<HashSet<String>>,
    pub features: ToolRuntimeFeatures,
    /// Latest user-task traits (drives host-side top-k / domain tools).
    pub task: Option<TaskTraits>,
}

impl Default for ListToolsContext {
    fn default() -> Self {
        Self {
            session_id: None,
            turn_id: None,
            activated: Arc::new(HashSet::new()),
            features: ToolRuntimeFeatures::default(),
            task: None,
        }
    }
}

impl ListToolsContext {
    pub fn from_features(features: ToolRuntimeFeatures) -> Self {
        Self {
            features,
            ..Default::default()
        }
    }
}

pub fn is_core_name(name: &str, features: &ToolRuntimeFeatures) -> bool {
    if let Some(ref override_names) = features.core_tools {
        return override_names.iter().any(|n| n == name);
    }
    DEFAULT_CORE_TOOL_NAMES.contains(&name)
}

/// Filter tools for the model tool list.
pub fn filter_listed_tools<'a>(
    tools: &'a [Arc<dyn Tool>],
    ctx: &ListToolsContext,
) -> Vec<&'a Arc<dyn Tool>> {
    if !ctx.features.progressive_listing || ctx.features.force_list_all {
        return tools.iter().collect();
    }
    // Small registries: skip discovery tax.
    if tools.len() <= 12 {
        return tools.iter().collect();
    }
    select_tools_for_turn(tools, ctx, DEFAULT_TOP_K)
}

/// Host-side top-k: always-on core + obvious domain tools + LRU activations.
pub fn select_tools_for_turn<'a>(
    tools: &'a [Arc<dyn Tool>],
    ctx: &ListToolsContext,
    top_k: usize,
) -> Vec<&'a Arc<dyn Tool>> {
    let mut scored: Vec<(i32, usize)> = tools
        .iter()
        .enumerate()
        .map(|(i, t)| (score_tool(t.as_ref(), ctx), i))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut out = Vec::new();
    let mut extras = 0usize;
    for (score, i) in scored {
        let t = &tools[i];
        let name = t.name();
        let core = is_core_name(name, &ctx.features) || name == "tool_search";
        let activated = ctx.activated.contains(name);
        let domain = score >= 400;
        if core || activated || domain || t.should_list(ctx) {
            out.push(t);
            continue;
        }
        if extras < top_k && score > 0 {
            out.push(t);
            extras += 1;
        }
    }
    // Preserve original registration order for the selected set.
    out.sort_by_key(|t| tools.iter().position(|x| x.name() == t.name()).unwrap_or(0));
    out
}

fn score_tool(tool: &dyn Tool, ctx: &ListToolsContext) -> i32 {
    let name = tool.name();
    if is_core_name(name, &ctx.features) || name == "tool_search" {
        return 1_000;
    }
    if ctx.activated.contains(name) {
        return 500;
    }
    let Some(task) = ctx.task else {
        return 0;
    };
    let kind = tool.meta().kind;
    let mut score = 0;
    if task.time_date && matches!(name, "get_time" | "get_date") {
        score += 400;
    }
    if task.research_depth != crate::task::ResearchDepth::None
        && (name.starts_with("web_") || kind == ToolKind::Web)
    {
        score += 400;
    }
    if task.coding
        && matches!(
            name,
            "file_read" | "file_write" | "file_edit" | "glob" | "grep" | "bash"
        )
    {
        score += 400;
    }
    if task.side_effects && (name == "bash" || kind == ToolKind::Execute || kind == ToolKind::Write)
    {
        score += 300;
    }
    if (name.contains("memory") || kind == ToolKind::Memory)
        && (name.contains("remember") || name.contains("memory") || name.contains("note"))
    {
        score += 250;
    }
    if task.greeting || task.time_date {
        // Do not pull in extra domains on a greeting.
        return score.min(400);
    }
    score
}

/// Retention priority when the serialized schema table exceeds its budget.
///
/// Core tools remain above activated tools, explicit `should_list` tools, and
/// task-domain matches. The caller removes the lowest priority definitions
/// first (and may use definition size as a tie-breaker).
pub(crate) fn schema_retention_priority(tool: &dyn Tool, ctx: &ListToolsContext) -> i32 {
    let name = tool.name();
    if is_core_name(name, &ctx.features) || name == "tool_search" {
        return 10_000;
    }
    if ctx.activated.contains(name) {
        return 5_000;
    }
    if tool.should_list(ctx) {
        return 4_500;
    }
    score_tool(tool, ctx)
}

/// Insert names into the activation set (cap at [`MAX_ACTIVATED`], LRU + TTL).
pub fn activate_tools(activated: &ActivationSet, names: impl IntoIterator<Item = String>) {
    let Ok(mut guard) = activated.lock() else {
        return;
    };
    guard.activate(names);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Tool, ToolError, ToolMeta};
    use async_trait::async_trait;
    use serde_json::{json, Value};

    struct Named {
        name: &'static str,
        list: bool,
    }

    #[async_trait]
    impl Tool for Named {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "t"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object","properties":{}})
        }
        fn meta(&self) -> ToolMeta {
            ToolMeta::safe_default()
        }
        fn should_list(&self, _: &ListToolsContext) -> bool {
            self.list
        }
        async fn execute(
            &self,
            _: &crate::tool_context::ToolCallContext,
            _: Value,
        ) -> Result<String, ToolError> {
            Ok(String::new())
        }
    }

    #[test]
    fn progressive_lists_core_and_opt_in_only() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(Named {
                name: "get_time",
                list: false,
            }),
            Arc::new(Named {
                name: "bash",
                list: false,
            }),
            Arc::new(Named {
                name: "list_skills",
                list: true,
            }),
            Arc::new(Named {
                name: "file_read",
                list: false,
            }),
        ];
        // pad so len > 12 triggers progressive filter
        let mut many = tools;
        for i in 0..15 {
            many.push(Arc::new(Named {
                name: Box::leak(format!("extra_{i}").into_boxed_str()),
                list: false,
            }));
        }

        let features = ToolRuntimeFeatures {
            progressive_listing: true,
            ..Default::default()
        };
        let mut activated = HashSet::new();
        activated.insert("file_read".into());
        let ctx = ListToolsContext {
            activated: Arc::new(activated),
            features,
            ..Default::default()
        };
        let listed: Vec<&str> = filter_listed_tools(&many, &ctx)
            .iter()
            .map(|t| t.name())
            .collect();
        assert!(listed.contains(&"get_time"));
        assert!(listed.contains(&"list_skills"));
        assert!(listed.contains(&"file_read"));
        assert!(!listed.contains(&"bash"));
    }

    #[test]
    fn progressive_off_lists_all() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(Named {
                name: "a",
                list: false,
            }),
            Arc::new(Named {
                name: "b",
                list: false,
            }),
        ];
        let ctx = ListToolsContext::default();
        assert_eq!(filter_listed_tools(&tools, &ctx).len(), 2);
    }

    #[test]
    fn default_features_enable_parallel_read_wave() {
        let f = ToolRuntimeFeatures::default();
        assert!(f.wave_scheduling, "wave scheduling on by default");
        assert!(f.max_parallel_tools >= 16);
        assert!(f.progressive_listing, "progressive listing on by default");
        assert!(f.progress_events);
    }

    #[test]
    fn activation_table_lru_and_ttl() {
        let mut t = ActivationTable {
            entries: Vec::new(),
            ttl: Duration::from_millis(50),
            max: 2,
        };
        t.activate(["a".into(), "b".into(), "c".into()]);
        assert_eq!(t.snapshot().len(), 2);
        assert!(!t.contains("a"));
        assert!(t.contains("c"));
        std::thread::sleep(Duration::from_millis(60));
        assert!(t.snapshot().is_empty());
    }
}
