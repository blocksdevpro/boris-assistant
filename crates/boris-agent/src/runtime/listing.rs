//! Progressive tool listing (Grok-lite): core set + opt-in + activation.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::tool::Tool;

/// Session-long activation set mutated by `tool_search`.
pub type ActivationSet = Arc<Mutex<HashSet<String>>>;

pub fn new_activation_set() -> ActivationSet {
    Arc::new(Mutex::new(HashSet::new()))
}

/// Max activated tool names (LRU-ish: drop arbitrary when over cap).
pub const MAX_ACTIVATED: usize = 32;

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
            // Progressive listing stays opt-in (hosts may set BORIS_PROGRESSIVE_TOOLS).
            progressive_listing: false,
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
}

impl Default for ListToolsContext {
    fn default() -> Self {
        Self {
            session_id: None,
            turn_id: None,
            activated: Arc::new(HashSet::new()),
            features: ToolRuntimeFeatures::default(),
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
    tools
        .iter()
        .filter(|t| {
            let name = t.name();
            is_core_name(name, &ctx.features) || ctx.activated.contains(name) || t.should_list(ctx)
        })
        .collect()
}

/// Insert names into the activation set (cap at [`MAX_ACTIVATED`]).
pub fn activate_tools(activated: &ActivationSet, names: impl IntoIterator<Item = String>) {
    let Ok(mut guard) = activated.lock() else {
        return;
    };
    for name in names {
        if name.is_empty() {
            continue;
        }
        guard.insert(name);
        while guard.len() > MAX_ACTIVATED {
            // Drop an arbitrary entry (HashSet has no LRU; fine for voice scale).
            if let Some(first) = guard.iter().next().cloned() {
                guard.remove(&first);
            } else {
                break;
            }
        }
    }
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
        assert!(!f.progressive_listing);
        assert!(f.progress_events);
    }
}
