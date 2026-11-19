//! Tool listing: every builtin in a stable order, plus extensions (MCP /
//! plugins) activated through `tool_search` (bounded LRU + TTL).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::task::TaskTraits;
use crate::tool::Tool;

/// Session activation set mutated by `tool_search` (bounded LRU + TTL).
pub type ActivationSet = Arc<Mutex<ActivationTable>>;

pub fn new_activation_set() -> ActivationSet {
    Arc::new(Mutex::new(ActivationTable::default()))
}

/// Max activated tool names (LRU evicts oldest).
pub const MAX_ACTIVATED: usize = 32;
/// Activations expire so a session-long set cannot keep growing forever.
pub const ACTIVATION_TTL: Duration = Duration::from_secs(15 * 60);
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
    listed: HashSet<String>,
    ttl: Duration,
    max: usize,
}

impl Default for ActivationTable {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            listed: HashSet::new(),
            ttl: ACTIVATION_TTL,
            max: MAX_ACTIVATED,
        }
    }
}

impl ActivationTable {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.listed.clear();
    }

    /// Actual request-local availability, including core tools and schema pruning.
    pub fn record_listed(&mut self, names: impl IntoIterator<Item = String>) -> bool {
        let names: HashSet<String> = names.into_iter().collect();
        let changed = self.listed != names;
        self.listed = names;
        changed
    }

    pub fn listed(&self) -> &HashSet<String> {
        &self.listed
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

/// Names given extra schema-budget protection. Every builtin is listed; these
/// are only the last builtins dropped when the schema table is over budget.
pub const DEFAULT_CORE_TOOL_NAMES: &[&str] = &[
    "remember_note",
    "tool_search",
    "present_artifact",
    "collect_input",
];

/// Discovery meta-tool name. Listed only when extensions are registered.
pub const TOOL_SEARCH_NAME: &str = "tool_search";

/// Feature flags for listing / concurrency / progress (owned by [`crate::Agent`]).
#[derive(Debug, Clone)]
pub struct ToolRuntimeFeatures {
    /// List every builtin plus activated or opted-in extensions. When false,
    /// every registered tool (extensions included) is listed.
    pub progressive_listing: bool,
    /// Ignore progressive filter; list everything.
    pub force_list_all: bool,
    /// Execute contiguous read-only runs in parallel, preserving write/approval
    /// barriers. When false, use legacy bounded parallel dispatch for a
    /// pause-free batch (including auto-approved writes).
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
            // Every builtin + discovered extensions + discovery when needed.
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
    /// Latest user-task traits (informational for `should_list` opt-ins; the
    /// builtin listing never depends on them).
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
///
/// Use [`pruned_tool_count`] or [`filter_listed_tools_with_count`] when the
/// caller must surface how many registered tools stayed unlisted.
pub fn filter_listed_tools<'a>(
    tools: &'a [Arc<dyn Tool>],
    ctx: &ListToolsContext,
) -> Vec<&'a Arc<dyn Tool>> {
    filter_listed_tools_with_count(tools, ctx).0
}

/// Filter tools plus the number left unlisted.
///
/// Returns `(listed, pruned_count)` where `pruned_count = tools.len() -
/// listed.len()`.
pub fn filter_listed_tools_with_count<'a>(
    tools: &'a [Arc<dyn Tool>],
    ctx: &ListToolsContext,
) -> (Vec<&'a Arc<dyn Tool>>, usize) {
    let listed = filter_listed_inner(tools, ctx);
    let pruned = tools.len().saturating_sub(listed.len());
    (listed, pruned)
}

/// Number of registered tools left out of the listing (unactivated extensions,
/// or `tool_search` when no extension is registered).
pub fn pruned_tool_count(tools: &[Arc<dyn Tool>], ctx: &ListToolsContext) -> usize {
    filter_listed_tools_with_count(tools, ctx).1
}

/// Every builtin in registration order, plus activated or opted-in extensions.
///
/// The builtin set never depends on the request text, so the serialized
/// schema table (and the provider prompt-cache prefix) stays identical for the
/// whole session. `tool_search` is listed only when at least one extension is
/// registered, because it finds nothing else.
fn filter_listed_inner<'a>(
    tools: &'a [Arc<dyn Tool>],
    ctx: &ListToolsContext,
) -> Vec<&'a Arc<dyn Tool>> {
    if !ctx.features.progressive_listing || ctx.features.force_list_all {
        return tools.iter().collect();
    }
    let any_extension = tools.iter().any(|tool| tool.is_extension());
    tools
        .iter()
        .filter(|tool| {
            let name = tool.name();
            if name == TOOL_SEARCH_NAME {
                return any_extension;
            }
            !tool.is_extension()
                || ctx.activated.contains(name)
                || tool.should_list(ctx)
                || ctx
                    .features
                    .core_tools
                    .as_ref()
                    .is_some_and(|names| names.iter().any(|n| n == name))
        })
        .collect()
}

/// Retention priority when the serialized schema table exceeds its budget.
///
/// Activated extensions outrank shared builtins; unactivated opt-in
/// extensions go first. The caller removes the lowest priority definitions
/// first (and may use definition size as a tie-breaker).
pub(crate) fn schema_retention_priority(tool: &dyn Tool, ctx: &ListToolsContext) -> i32 {
    let name = tool.name();
    if tool.is_extension() && ctx.activated.contains(name) {
        return 11_000;
    }
    if is_core_name(name, &ctx.features) || name == TOOL_SEARCH_NAME {
        return 10_000;
    }
    if !tool.is_extension() {
        return 5_000;
    }
    if tool.should_list(ctx) {
        return 4_500;
    }
    0
}

/// Activate the extensions among `names`. Builtins are always listed, so they
/// never consume activation slots.
pub fn activate_used_extensions<'a>(
    activated: &ActivationSet,
    tools: &[Arc<dyn Tool>],
    names: impl IntoIterator<Item = &'a String>,
) {
    activate_tools(
        activated,
        names
            .into_iter()
            .filter(|name| {
                tools
                    .iter()
                    .any(|tool| tool.name() == name.as_str() && tool.is_extension())
            })
            .cloned(),
    );
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
        extension: bool,
        list: bool,
    }

    fn builtin(name: &'static str) -> Arc<dyn Tool> {
        Arc::new(Named {
            name,
            extension: false,
            list: false,
        })
    }

    fn extension(name: &'static str) -> Arc<dyn Tool> {
        Arc::new(Named {
            name,
            extension: true,
            list: false,
        })
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
        fn is_extension(&self) -> bool {
            self.extension
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

    fn names<'a>(tools: &'a [Arc<dyn Tool>], ctx: &ListToolsContext) -> Vec<&'a str> {
        filter_listed_tools(tools, ctx)
            .iter()
            .map(|t| t.name())
            .collect()
    }

    #[test]
    fn every_builtin_is_listed_in_registration_order_regardless_of_task() {
        let tools: Vec<Arc<dyn Tool>> = [
            "bash",
            "web_search",
            "file_edit",
            "get_time",
            "clipboard_get",
            "tool_search",
        ]
        .into_iter()
        .map(builtin)
        .collect();
        let greeting = ListToolsContext {
            task: Some(crate::task::classify_task("hi")),
            ..Default::default()
        };
        let coding = ListToolsContext {
            task: Some(crate::task::classify_task("debug this repo")),
            ..Default::default()
        };
        let expected = ["bash", "web_search", "file_edit", "get_time", "clipboard_get"];
        assert_eq!(names(&tools, &greeting), expected);
        assert_eq!(
            names(&tools, &coding),
            expected,
            "listing must not depend on the request text"
        );
    }

    #[test]
    fn extensions_need_activation_and_bring_tool_search() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            builtin("file_read"),
            extension("github__search"),
            builtin("tool_search"),
            Arc::new(Named {
                name: "pinned__plugin",
                extension: true,
                list: true,
            }),
            extension("slack__post"),
        ];
        let mut ctx = ListToolsContext::default();
        assert_eq!(
            names(&tools, &ctx),
            ["file_read", "tool_search", "pinned__plugin"]
        );
        ctx.activated = Arc::new(HashSet::from(["slack__post".to_string()]));
        assert_eq!(
            names(&tools, &ctx),
            ["file_read", "tool_search", "pinned__plugin", "slack__post"]
        );
        assert_eq!(pruned_tool_count(&tools, &ctx), 1);
    }

    #[test]
    fn force_list_all_and_disabled_progressive_list_everything() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            builtin("file_read"),
            extension("github__search"),
            builtin("tool_search"),
        ];
        let mut ctx = ListToolsContext::default();
        ctx.features.force_list_all = true;
        assert_eq!(filter_listed_tools(&tools, &ctx).len(), tools.len());
        ctx.features.force_list_all = false;
        ctx.features.progressive_listing = false;
        assert_eq!(filter_listed_tools(&tools, &ctx).len(), tools.len());
    }

    #[test]
    fn activated_extensions_outrank_builtins_for_schema_budget() {
        let ctx = ListToolsContext {
            activated: Arc::new(HashSet::from(["github__search".to_string()])),
            ..Default::default()
        };
        let active = extension("github__search");
        let idle = extension("slack__post");
        let shared = builtin("bash");
        let core = builtin("remember_note");
        assert!(
            schema_retention_priority(active.as_ref(), &ctx)
                > schema_retention_priority(core.as_ref(), &ctx)
        );
        assert!(
            schema_retention_priority(core.as_ref(), &ctx)
                > schema_retention_priority(shared.as_ref(), &ctx)
        );
        assert!(
            schema_retention_priority(shared.as_ref(), &ctx)
                > schema_retention_priority(idle.as_ref(), &ctx)
        );
    }

    #[test]
    fn only_used_extensions_consume_activation_slots() {
        let tools: Vec<Arc<dyn Tool>> = vec![builtin("bash"), extension("github__search")];
        let activated = new_activation_set();
        activate_used_extensions(
            &activated,
            &tools,
            &["bash".to_string(), "github__search".to_string(), "ghost".to_string()],
        );
        let snapshot = activated.lock().unwrap().snapshot();
        assert_eq!(snapshot, HashSet::from(["github__search".to_string()]));
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
            listed: HashSet::new(),
        };
        t.activate(["a".into(), "b".into(), "c".into()]);
        assert_eq!(t.snapshot().len(), 2);
        assert!(!t.contains("a"));
        assert!(t.contains("c"));
        std::thread::sleep(Duration::from_millis(60));
        assert!(t.snapshot().is_empty());
    }

    #[test]
    fn pruned_count_matches_listed_difference() {
        let mut many: Vec<Arc<dyn Tool>> = vec![builtin("get_time"), builtin("bash")];
        for name in ["a__x", "b__y", "c__z"] {
            many.push(extension(name));
        }
        let ctx = ListToolsContext::default();
        let listed = filter_listed_tools(&many, &ctx);
        let (listed2, pruned) = filter_listed_tools_with_count(&many, &ctx);
        assert_eq!(listed.len(), listed2.len());
        assert_eq!(pruned, pruned_tool_count(&many, &ctx));
        assert_eq!(pruned, 3);
        let small: Vec<Arc<dyn Tool>> = vec![builtin("a")];
        assert_eq!(pruned_tool_count(&small, &ListToolsContext::default()), 0);
    }
}
