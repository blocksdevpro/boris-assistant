//! Progressive tool listing: core + per-turn top-k + LRU/TTL activation.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::ToolSelection;
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
/// Host-side top-k listed on top of the always-on core (when progressive).
pub const DEFAULT_TOP_K: usize = 4;
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

/// Small shared set. Task bundles provide work tools before the first request.
pub const DEFAULT_CORE_TOOL_NAMES: &[&str] = &[
    "remember_note",
    "tool_search",
    "present_artifact",
    "collect_input",
];

/// Feature flags for listing / concurrency / progress (owned by [`crate::Agent`]).
#[derive(Debug, Clone)]
pub struct ToolRuntimeFeatures {
    /// List shared tools, task bundles, matching plugins, activations, and opt-ins.
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
            // Shared tools + task bundles + plugin top-k + discovery fallback.
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
    pub selection: Option<ToolSelection>,
}

impl Default for ListToolsContext {
    fn default() -> Self {
        Self {
            session_id: None,
            turn_id: None,
            activated: Arc::new(HashSet::new()),
            features: ToolRuntimeFeatures::default(),
            task: None,
            selection: None,
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
/// Pruning was previously silent; use [`pruned_tool_count`] or
/// [`filter_listed_tools_with_count`] when the caller must surface how many
/// definitions were dropped (Group A logs it alongside the schema budget).
pub fn filter_listed_tools<'a>(
    tools: &'a [Arc<dyn Tool>],
    ctx: &ListToolsContext,
) -> Vec<&'a Arc<dyn Tool>> {
    filter_listed_tools_with_count(tools, ctx).0
}

/// Filter tools plus the number pruned (no scoring change).
///
/// Returns `(listed, pruned_count)` where `pruned_count = tools.len() -
/// listed.len()`. Existing callers keep using [`filter_listed_tools`];
/// new callers (Group A) use this to log the silent prune.
pub fn filter_listed_tools_with_count<'a>(
    tools: &'a [Arc<dyn Tool>],
    ctx: &ListToolsContext,
) -> (Vec<&'a Arc<dyn Tool>>, usize) {
    let listed = filter_listed_inner(tools, ctx);
    let pruned = tools.len().saturating_sub(listed.len());
    (listed, pruned)
}

/// Number of tool definitions dropped by progressive listing.
///
/// `0` means everything is listed (progressive off, force-list-all, small
/// registry, or everything selected). Does not change scoring.
pub fn pruned_tool_count(tools: &[Arc<dyn Tool>], ctx: &ListToolsContext) -> usize {
    filter_listed_tools_with_count(tools, ctx).1
}

fn filter_listed_inner<'a>(
    tools: &'a [Arc<dyn Tool>],
    ctx: &ListToolsContext,
) -> Vec<&'a Arc<dyn Tool>> {
    if !ctx.features.progressive_listing || ctx.features.force_list_all {
        return tools.iter().collect();
    }
    // Unclassified embedding callers with a small registry need no discovery.
    if tools.len() <= 12 && ctx.task.is_none() && ctx.selection.is_none() {
        return tools.iter().collect();
    }
    select_tools_for_turn(tools, ctx, DEFAULT_TOP_K)
}

/// Eager builtin bundles plus a bounded number of matching plugin tools.
pub fn select_tools_for_turn<'a>(
    tools: &'a [Arc<dyn Tool>],
    ctx: &ListToolsContext,
    top_k: usize,
) -> Vec<&'a Arc<dyn Tool>> {
    let selection = ctx
        .selection
        .clone()
        .or_else(|| ctx.task.map(ToolSelection::for_task));
    let mut scored: Vec<(i32, usize)> = tools
        .iter()
        .enumerate()
        .map(|(i, t)| {
            (
                selection
                    .as_ref()
                    .map_or(0, |selection| selection.score(t.name(), t.meta().kind)),
                i,
            )
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut selected = HashSet::new();
    let mut extras = 0usize;
    for (score, i) in scored {
        let t = &tools[i];
        let name = t.name();
        let core = is_core_name(name, &ctx.features) || name == "tool_search";
        let activated = ctx.activated.contains(name);
        let domain = score >= 400;
        if core || activated || domain || t.should_list(ctx) {
            selected.insert(i);
            continue;
        }
        if extras < top_k && score > 0 {
            selected.insert(i);
            extras += 1;
        }
    }
    // Preserve registration order even when relevance changes between requests.
    tools
        .iter()
        .enumerate()
        .filter_map(|(i, tool)| selected.contains(&i).then_some(tool))
        .collect()
}

/// Retention priority when the serialized schema table exceeds its budget.
///
/// Activated tools remain above shared tools, explicit opt-ins, and task-domain
/// matches. The caller removes the lowest priority definitions
/// first (and may use definition size as a tie-breaker).
pub(crate) fn schema_retention_priority(tool: &dyn Tool, ctx: &ListToolsContext) -> i32 {
    let name = tool.name();
    if ctx.activated.contains(name) {
        return 11_000;
    }
    if is_core_name(name, &ctx.features) || name == "tool_search" {
        return 10_000;
    }
    if tool.should_list(ctx) {
        return 4_500;
    }
    ctx.selection.as_ref().map_or_else(
        || {
            ctx.task.map_or(0, |task| {
                ToolSelection::for_task(task).score(name, tool.meta().kind)
            })
        },
        |selection| selection.score(name, tool.meta().kind),
    )
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
            if self.name.starts_with("web_plugin_") {
                ToolMeta::safe_default().kind(crate::tool::ToolKind::Web)
            } else {
                ToolMeta::safe_default()
            }
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
                name: "remember_note",
                list: false,
            }),
            Arc::new(Named {
                name: "web_fetch",
                list: false,
            }),
            Arc::new(Named {
                name: "bash",
                list: false,
            }),
            Arc::new(Named {
                name: "pinned_plugin",
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
        activated.insert("extra_0".into());
        let ctx = ListToolsContext {
            activated: Arc::new(activated),
            features,
            selection: Some(ToolSelection::for_request(
                "debug this repo then look up my GitHub profile",
            )),
            ..Default::default()
        };
        let listed: Vec<&str> = filter_listed_tools(&many, &ctx)
            .iter()
            .map(|t| t.name())
            .collect();
        assert!(listed.contains(&"remember_note"));
        assert!(listed.contains(&"pinned_plugin"));
        assert!(listed.contains(&"file_read"));
        assert!(listed.contains(&"bash"));
        assert!(listed.contains(&"web_fetch"));
        assert!(listed.contains(&"extra_0"));
        assert!(!listed.contains(&"extra_1"));
    }

    #[test]
    fn small_unclassified_registry_lists_all() {
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
    fn task_bundle_and_plugin_top_k_preserve_registration_order() {
        let tools: Vec<Arc<dyn Tool>> = [
            "web_plugin_0",
            "web_fetch",
            "web_plugin_1",
            "web_plugin_2",
            "web_plugin_3",
            "web_plugin_4",
            "web_plugin_5",
            "web_search",
            "bash",
        ]
        .into_iter()
        .map(|name| Arc::new(Named { name, list: false }) as Arc<dyn Tool>)
        .collect();
        let mut ctx = ListToolsContext {
            selection: Some(ToolSelection::for_request("look up the latest Rust news")),
            activated: Arc::new(HashSet::from(["web_plugin_5".into()])),
            ..Default::default()
        };
        let names = filter_listed_tools(&tools, &ctx)
            .iter()
            .map(|tool| tool.name())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "web_plugin_0",
                "web_fetch",
                "web_plugin_1",
                "web_plugin_2",
                "web_plugin_3",
                "web_plugin_5",
                "web_search"
            ]
        );
        ctx.features.force_list_all = true;
        assert_eq!(filter_listed_tools(&tools, &ctx).len(), tools.len());
        ctx.features.force_list_all = false;
        ctx.features.progressive_listing = false;
        assert_eq!(filter_listed_tools(&tools, &ctx).len(), tools.len());
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
    fn greeting_omits_unrelated_work_tools() {
        let names = [
            "file_read",
            "file_write",
            "file_edit",
            "list_dir",
            "glob",
            "grep",
            "bash",
            "web_search",
            "web_fetch",
            "memory_search",
            "memory_get",
            "present_artifact",
            "list_artifacts",
            "get_artifact",
        ];
        let tools: Vec<Arc<dyn Tool>> = names
            .iter()
            .map(|name| Arc::new(Named { name, list: false }) as Arc<dyn Tool>)
            .collect();
        let ctx = ListToolsContext {
            task: Some(crate::task::classify_task("hi")),
            ..Default::default()
        };
        let listed = filter_listed_tools(&tools, &ctx)
            .iter()
            .map(|tool| tool.name())
            .collect::<Vec<_>>();
        assert_eq!(listed, ["present_artifact"]);
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
    fn pruned_count_matches_listed_difference_without_breaking_callers() {
        // Build a registry large enough to trigger progressive filtering.
        let mut many: Vec<Arc<dyn Tool>> = vec![
            Arc::new(Named {
                name: "get_time",
                list: false,
            }),
            Arc::new(Named {
                name: "bash",
                list: false,
            }),
        ];
        for i in 0..15 {
            many.push(Arc::new(Named {
                name: Box::leak(format!("extra_{i}").into_boxed_str()),
                list: false,
            }));
        }
        let ctx = ListToolsContext {
            features: ToolRuntimeFeatures {
                progressive_listing: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let listed = filter_listed_tools(&many, &ctx);
        let (listed2, pruned) = filter_listed_tools_with_count(&many, &ctx);
        assert_eq!(listed.len(), listed2.len());
        assert_eq!(pruned, pruned_tool_count(&many, &ctx));
        assert_eq!(pruned, many.len().saturating_sub(listed.len()));
        // Small registry: no prune.
        let small: Vec<Arc<dyn Tool>> = vec![Arc::new(Named {
            name: "a",
            list: false,
        })];
        assert_eq!(pruned_tool_count(&small, &ListToolsContext::default()), 0);
    }
}
