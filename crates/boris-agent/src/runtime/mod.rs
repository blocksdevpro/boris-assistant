//! Async tool execution plane: policy, timeout, truncation, audit, HITL pending.

pub mod audit;
pub mod pending;
pub mod policy;
pub mod timeout;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde_json::Value;

use crate::reminder::with_reminder;
use crate::tool::{truncate_tool_result, Tool, ToolMeta};
use crate::tool_context::ToolCallContext;

pub use audit::{
    args_digest, args_summary, now_ms, AuditEvent, AuditSink, JsonlAuditSink, MemoryAuditSink,
    NullAuditSink,
};
pub use pending::{PendingToolCall, PendingTurn, RawToolCall};
pub use policy::{
    decide, default_user_read_roots, normalize_path, path_is_within, resolve_in_roots,
    NetworkPolicy, PolicyDecision, SandboxConfig, ShellPolicy,
};
pub use timeout::{is_timeout, run_with_timeout};

/// One LLM-requested tool call entering the runtime.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub call_id: String,
    pub name: String,
    pub args: Value,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    /// Optional working directory for this call.
    pub cwd: Option<std::path::PathBuf>,
    /// Optional cancel token (cloned into [`ToolCallContext`]).
    pub cancel: Option<tokio_util::sync::CancellationToken>,
}

impl ToolInvocation {
    pub fn new(call_id: impl Into<String>, name: impl Into<String>, args: Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            args,
            session_id: None,
            turn_id: None,
            cwd: None,
            cancel: None,
        }
    }

    pub fn call_context(&self) -> ToolCallContext {
        ToolCallContext::new(self.call_id.clone())
            .with_session(self.session_id.clone(), self.turn_id.clone())
            .with_cwd(self.cwd.clone())
            .with_cancel(self.cancel.clone())
    }
}

/// Options for a single invoke.
#[derive(Debug, Clone, Copy)]
pub struct InvokeOptions {
    /// Skip HITL (one-shot grant after user approved).
    pub skip_confirmation: bool,
    /// Confirms already used this turn (for cap).
    pub confirms_used: u32,
}

impl Default for InvokeOptions {
    fn default() -> Self {
        Self {
            skip_confirmation: false,
            confirms_used: 0,
        }
    }
}

/// Result of runtime mediation (before the engine continues the ReAct loop).
#[derive(Debug, Clone)]
pub enum InvokeResult {
    /// Observation string for the model (already truncated).
    Observation(String),
    /// Pause for host HITL; tool was not executed.
    NeedsConfirmation {
        pending: PendingToolCall,
        speak_prompt: String,
    },
    /// Hard deny — engine should feed this as an error observation.
    Denied { reason: String },
}

/// Mediates every tool call: policy → (confirm|deny|run) → truncate → audit.
pub struct ToolRuntime {
    policy: SandboxConfig,
    audit: Box<dyn AuditSink>,
    pending_seq: AtomicU64,
}

impl ToolRuntime {
    pub fn new(policy: SandboxConfig, audit: Box<dyn AuditSink>) -> Self {
        Self {
            policy,
            audit,
            pending_seq: AtomicU64::new(1),
        }
    }

    /// Default in-memory policy + null audit (tests / early init).
    pub fn null() -> Self {
        Self::new(SandboxConfig::default(), Box::new(NullAuditSink))
    }

    pub fn policy(&self) -> &SandboxConfig {
        &self.policy
    }

    pub fn set_policy(&mut self, policy: SandboxConfig) {
        self.policy = policy;
    }

    pub fn set_audit(&mut self, audit: Box<dyn AuditSink>) {
        self.audit = audit;
    }

    fn next_pending_id(&self) -> String {
        let n = self.pending_seq.fetch_add(1, Ordering::Relaxed);
        format!("p-{n}-{}", now_ms())
    }

    /// Policy-only decision (no execute). Used to plan parallel batches.
    pub fn decide_only(
        &self,
        tool: &dyn Tool,
        args: &Value,
        opts: InvokeOptions,
    ) -> PolicyDecision {
        let meta = tool.meta();
        if opts.skip_confirmation {
            return PolicyDecision::Allow;
        }
        let args = if args.is_object() {
            args.clone()
        } else {
            Value::Object(Default::default())
        };
        decide(&self.policy, &meta, &args, opts.confirms_used)
    }

    /// Run policy + optional execute for one tool (async).
    pub async fn invoke(
        &self,
        tool: &dyn Tool,
        inv: ToolInvocation,
        opts: InvokeOptions,
    ) -> InvokeResult {
        let meta = tool.meta();
        let args = if inv.args.is_object() {
            inv.args.clone()
        } else {
            Value::Object(Default::default())
        };

        let decision = if opts.skip_confirmation {
            PolicyDecision::Allow
        } else {
            decide(&self.policy, &meta, &args, opts.confirms_used)
        };

        match decision {
            PolicyDecision::Deny { reason } => {
                self.audit_event(&inv, &meta, "deny", None, Some(false), Some("denied"));
                InvokeResult::Denied { reason }
            }
            PolicyDecision::NeedsConfirmation { reason } => {
                let pending = PendingToolCall::new(
                    self.next_pending_id(),
                    inv.name.clone(),
                    args.clone(),
                    args_summary(&inv.name, &args),
                    meta.risk,
                    inv.call_id.clone(),
                );
                self.audit_event(
                    &inv,
                    &meta,
                    "confirm",
                    None,
                    None,
                    Some("needs_confirmation"),
                );
                let speak_prompt = speak_confirm_prompt(&pending, &reason);
                InvokeResult::NeedsConfirmation {
                    pending,
                    speak_prompt,
                }
            }
            PolicyDecision::Allow => self.execute_allowed(tool, inv, meta, args, opts).await,
        }
    }

    async fn execute_allowed(
        &self,
        tool: &dyn Tool,
        inv: ToolInvocation,
        meta: ToolMeta,
        args: Value,
        opts: InvokeOptions,
    ) -> InvokeResult {
        let decision_label = if opts.skip_confirmation {
            "confirmed"
        } else {
            "allow"
        };
        let ctx = inv.call_context();
        let started = Instant::now();
        let result = run_with_timeout(tool, &ctx, args, meta.default_timeout).await;
        let duration_ms = started.elapsed().as_millis() as u64;

        match result {
            Ok(output) => {
                let obs = with_reminder(&inv.name, truncate_tool_result(output));
                self.audit_event(
                    &inv,
                    &meta,
                    decision_label,
                    Some(duration_ms),
                    Some(true),
                    None,
                );
                InvokeResult::Observation(obs)
            }
            Err(e) => {
                let kind = if is_timeout(&e) { "timeout" } else { "error" };
                let decision = if is_timeout(&e) {
                    "timeout"
                } else {
                    decision_label
                };
                self.audit_event(
                    &inv,
                    &meta,
                    decision,
                    Some(duration_ms),
                    Some(false),
                    Some(kind),
                );
                let obs = with_reminder(
                    &inv.name,
                    truncate_tool_result(format!("Error: {}", e.message)),
                );
                InvokeResult::Observation(obs)
            }
        }
    }

    /// Record a user rejection without executing.
    pub fn audit_rejection(
        &self,
        pending: &PendingToolCall,
        session_id: Option<&str>,
        turn_id: Option<&str>,
    ) {
        self.audit.write(&AuditEvent {
            ts_ms: now_ms(),
            session_id: session_id.map(|s| s.to_string()),
            turn_id: turn_id.map(|s| s.to_string()),
            tool: pending.name.clone(),
            risk: pending.risk.as_str().to_string(),
            decision: "rejected".into(),
            args_digest: args_digest(&pending.args),
            ok: Some(false),
            duration_ms: None,
            error_kind: Some("user_rejected".into()),
        });
    }

    fn audit_event(
        &self,
        inv: &ToolInvocation,
        meta: &ToolMeta,
        decision: &str,
        duration_ms: Option<u64>,
        ok: Option<bool>,
        error_kind: Option<&str>,
    ) {
        self.audit.write(&AuditEvent {
            ts_ms: now_ms(),
            session_id: inv.session_id.clone(),
            turn_id: inv.turn_id.clone(),
            tool: inv.name.clone(),
            risk: meta.risk.as_str().to_string(),
            decision: decision.to_string(),
            args_digest: args_digest(&inv.args),
            ok,
            duration_ms,
            error_kind: error_kind.map(|s| s.to_string()),
        });
    }
}

fn speak_confirm_prompt(pending: &PendingToolCall, _reason: &str) -> String {
    let summary = if pending.args_summary.chars().count() > 80 {
        let head: String = pending.args_summary.chars().take(77).collect();
        format!("{head}…")
    } else {
        pending.args_summary.clone()
    };
    format!("I want to run {summary}. Should I go ahead?")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Tool, ToolError, ToolMeta, ToolRisk, MAX_TOOL_RESULT_CHARS};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;
    use std::time::Duration;

    struct LongTool;

    #[async_trait]
    impl Tool for LongTool {
        fn name(&self) -> &str {
            "long"
        }
        fn description(&self) -> &str {
            "long"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object","properties":{},"required":[]})
        }
        async fn execute(
            &self,
            _ctx: &crate::tool_context::ToolCallContext,
            _args: Value,
        ) -> Result<String, ToolError> {
            Ok("x".repeat(MAX_TOOL_RESULT_CHARS + 100))
        }
    }

    struct ConfirmTool {
        ran: Mutex<bool>,
    }

    #[async_trait]
    impl Tool for ConfirmTool {
        fn name(&self) -> &str {
            "danger"
        }
        fn description(&self) -> &str {
            "needs confirm"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object","properties":{},"required":[]})
        }
        fn meta(&self) -> ToolMeta {
            ToolMeta::with_risk(ToolRisk::Dangerous)
        }
        async fn execute(
            &self,
            _ctx: &crate::tool_context::ToolCallContext,
            _args: Value,
        ) -> Result<String, ToolError> {
            *self.ran.lock().unwrap() = true;
            Ok("ran".into())
        }
    }

    fn inv(id: &str, name: &str) -> ToolInvocation {
        ToolInvocation::new(id, name, json!({}))
    }

    #[tokio::test]
    async fn invoke_truncates() {
        let audit = MemoryAuditSink::new();
        let rt = ToolRuntime::new(SandboxConfig::default(), Box::new(audit));
        match rt.invoke(&LongTool, inv("1", "long"), InvokeOptions::default()).await {
            InvokeResult::Observation(s) => {
                // Reminder may append; truncation applies to the tool body first.
                assert!(s.contains("[truncated]"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn dangerous_pauses_without_running() {
        let tool = ConfirmTool {
            ran: Mutex::new(false),
        };
        let rt = ToolRuntime::null();
        match rt
            .invoke(&tool, inv("c1", "danger"), InvokeOptions::default())
            .await
        {
            InvokeResult::NeedsConfirmation { pending, .. } => {
                assert_eq!(pending.name, "danger");
                assert!(!*tool.ran.lock().unwrap());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn grant_skips_confirm_and_runs() {
        let tool = ConfirmTool {
            ran: Mutex::new(false),
        };
        let rt = ToolRuntime::null();
        let opts = InvokeOptions {
            skip_confirmation: true,
            confirms_used: 1,
        };
        match rt.invoke(&tool, inv("c1", "danger"), opts).await {
            InvokeResult::Observation(s) => {
                assert!(s.starts_with("ran"));
                assert!(*tool.ran.lock().unwrap());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn timeout_becomes_error_observation() {
        struct Slow;
        #[async_trait]
        impl Tool for Slow {
            fn name(&self) -> &str {
                "slow"
            }
            fn description(&self) -> &str {
                "s"
            }
            fn parameters(&self) -> Value {
                json!({"type":"object","properties":{},"required":[]})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::safe_default().timeout(Duration::from_millis(40))
            }
            async fn execute(
                &self,
                _ctx: &crate::tool_context::ToolCallContext,
                _: Value,
            ) -> Result<String, ToolError> {
                tokio::time::sleep(Duration::from_secs(2)).await;
                Ok("nope".into())
            }
        }
        let rt = ToolRuntime::null();
        match rt.invoke(&Slow, inv("1", "slow"), InvokeOptions::default()).await {
            InvokeResult::Observation(s) => assert!(s.contains("timed out") || s.contains("Error")),
            other => panic!("unexpected {other:?}"),
        }
    }
}
