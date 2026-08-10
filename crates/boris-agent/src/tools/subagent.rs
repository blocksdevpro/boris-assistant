//! Lean subagent: run a short read-mostly tool loop and return a summary.
//!
//! Child tools are filtered with [`ToolMeta::is_read_only`](crate::tool::ToolMeta::is_read_only)
//! and `risk <= Moderate`. Production tools must set explicit `read_only(true)`
//! on their meta (kind-only heuristics only treat Read/Search as RO). After the
//! profile-tool meta fix, `get_user_context` / `recall_notes` / file reads are
//! eligible; writers (`save_user_fact`, `remember_note`, bash, …) are not.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use boris_ai::LlmClient;
use serde_json::{json, Value};

use crate::context::{Context, Role};
use crate::loop_::{agent_loop, LoopState};
use crate::runtime::{SandboxConfig, ToolRuntime, NullAuditSink};
use crate::tool::{
    require_object, require_string, truncate_tool_result, Tool, ToolError, ToolKind, ToolMeta,
    ToolRisk,
};
use crate::tool_context::ToolCallContext;
use crate::types::{AgentLoopConfig, DEFAULT_MAX_TOOL_ROUNDS};

/// Shared client for subagent runs (same API key/route as parent).
pub type SharedLlm = Arc<dyn LlmClient>;

/// Read-only (or safe) tools the parent registers for subagents.
pub type SharedTools = Arc<Mutex<Vec<Arc<dyn Tool>>>>;

pub struct SpawnSubagentTool {
    client: SharedLlm,
    /// Tools available to children (filtered to read-ish kinds at execute time).
    tools: SharedTools,
    sandbox: SandboxConfig,
}

impl SpawnSubagentTool {
    pub fn new(client: SharedLlm, tools: SharedTools, sandbox: SandboxConfig) -> Self {
        Self {
            client,
            tools,
            sandbox,
        }
    }
}

#[async_trait]
impl Tool for SpawnSubagentTool {
    fn name(&self) -> &str {
        "spawn_subagent"
    }

    fn description(&self) -> &str {
        "Run a short focused sub-task with read-only tools and return a compact summary. \
         Use for deep exploration (search many files, gather context) while you stay on the main plan. \
         Args: goal (required), max_rounds (optional, default 4, max 8)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "What the subagent should investigate or gather"
                },
                "max_rounds": {
                    "type": "integer",
                    "description": "Tool rounds budget (default 4)"
                }
            },
            "required": ["goal"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Other)
            .timeout(std::time::Duration::from_secs(90))
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(&self, _ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let goal = require_string(obj, "goal")?;
        let max_rounds = obj
            .get("max_rounds")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(4)
            .clamp(1, 8);

        let child_tools: Vec<std::sync::Arc<dyn Tool>> = {
            let tools_arc = self
                .tools
                .lock()
                .map_err(|_| ToolError::failed("subagent tools lock"))?;
            tools_arc
                .iter()
                .filter(|t| {
                    if t.name() == "spawn_subagent" {
                        return false;
                    }
                    let m = t.meta();
                    // Prefer explicit meta.read_only (concurrency annotations).
                    m.is_read_only() && m.risk <= ToolRisk::Moderate
                })
                .cloned()
                .collect()
        };

        if child_tools.is_empty() {
            return Ok(truncate_tool_result(
                "Subagent has no read-only tools available.".into(),
            ));
        }

        let mut context = Context::new(8);
        context.push(
            Role::System,
            "You are a focused research subagent. Use tools to gather facts. \
             When done, reply with a compact bullet summary only (no fluff). \
             Do not call spawn_subagent.",
        );
        context.push(Role::User, goal.as_str());

        let runtime = ToolRuntime::new(self.sandbox.clone(), Box::new(NullAuditSink));
        let config = AgentLoopConfig {
            max_tool_rounds: max_rounds.min(DEFAULT_MAX_TOOL_ROUNDS),
            session_id: None,
            turn_id: None,
            features: crate::runtime::ToolRuntimeFeatures::default(),
            // Child registries are small; always list all child tools.
            force_list_all: true,
        };

        let state = LoopState {
            context: &mut context,
            tools: &child_tools,
            runtime: &runtime,
            client: self.client.as_ref(),
            activated: None,
        };

        let result = agent_loop(state, &goal, &config, vec![], 0, 0, None, None, None, 0)
            .await
            .map_err(|e| ToolError::failed(format!("subagent failed: {e}")))?;

        let summary = match result.outcome {
            crate::outcome::AgentOutcome::Speak { text, .. } => text,
            crate::outcome::AgentOutcome::Silent => {
                "(subagent finished with no text)".into()
            }
            crate::outcome::AgentOutcome::NeedsConfirmation { text, .. } => {
                format!("(subagent paused for confirm: {text})")
            }
        };
        let tools = if result.tools_used.is_empty() {
            "none".into()
        } else {
            result.tools_used.join(", ")
        };
        Ok(truncate_tool_result_to_summary(format!(
            "<subagent_result tools=\"{tools}\" rounds={}>\n{summary}\n</subagent_result>",
            result.tool_rounds
        )))
    }
}

fn truncate_tool_result_to_summary(s: String) -> String {
    truncate_tool_result(s)
}


