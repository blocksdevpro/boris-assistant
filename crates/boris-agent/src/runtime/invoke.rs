//! [`ToolRuntime`]: policy → (confirm | deny | run) → truncate → audit.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use serde_json::Value;

use crate::tool::{
    truncate_tool_result_detailed, validate_args, Permission, Tool, ToolMeta, ToolObservation,
};

use super::audit::{args_digest, args_summary, now_ms, AuditEvent, AuditSink, NullAuditSink};
use crate::tools::collect_input::parse_collect_args;

use super::pending::{PendingInput, PendingToolCall};
use super::policy::{decide, PolicyDecision, SandboxConfig};
use super::timeout::{is_timeout, run_with_timeout};

// Re-export so `crate::runtime::invoke::{ToolInvocation, …}` stays valid.
pub use super::invocation::{InvokeOptions, InvokeResult, ToolInvocation};

/// Mediates every tool call: policy → (confirm | deny | run) → truncate → audit.
pub struct ToolRuntime {
    policy: SandboxConfig,
    audit: Box<dyn AuditSink>,
    pending_seq: AtomicU64,
    /// After the user approves any shell tool this turn, further shell HITL is
    /// skipped (hard gates still apply). Reset at each new user turn / abort.
    shell_granted_this_turn: AtomicBool,
    /// Monotonic host turn sequence for grant-expiry tracking (see
    /// [`ToolRuntime::begin_turn`]). `0` = no turn started yet.
    turn_seq: AtomicU64,
    /// `turn_seq` value when the shell grant was issued; `0` = no grant.
    /// Lets the runtime warn when a grant would otherwise creep past one turn
    /// because the host forgot [`ToolRuntime::clear_turn_grants`].
    shell_grant_turn: AtomicU64,
    gate: super::ConcurrencyGate,
}

impl ToolRuntime {
    /// Create a runtime with the given sandbox policy and audit sink.
    pub fn new(policy: SandboxConfig, audit: Box<dyn AuditSink>) -> Self {
        Self {
            policy,
            audit,
            pending_seq: AtomicU64::new(1),
            shell_granted_this_turn: AtomicBool::new(false),
            turn_seq: AtomicU64::new(0),
            shell_grant_turn: AtomicU64::new(0),
            gate: super::ConcurrencyGate::new(16),
        }
    }

    /// Default in-memory policy + null audit (tests / early init).
    pub fn null() -> Self {
        Self::new(SandboxConfig::default(), Box::new(NullAuditSink))
    }

    /// Current sandbox policy.
    pub fn policy(&self) -> &SandboxConfig {
        &self.policy
    }

    /// Replace sandbox policy.
    pub fn set_policy(&mut self, policy: SandboxConfig) {
        self.policy = policy;
    }

    /// Replace audit sink.
    pub fn set_audit(&mut self, audit: Box<dyn AuditSink>) {
        self.audit = audit;
    }

    /// Clear turn-scoped shell grant.
    ///
    /// The host **must** call this (or [`ToolRuntime::begin_turn`]) at the
    /// start of each user turn and on abort, otherwise a prior yes would keep
    /// skipping shell HITL forever (grant creep). Prefer `begin_turn` when the
    /// host has a monotonic turn counter so stale grants are auto-cleared with
    /// a `tracing::warn!`; this method is the manual fallback.
    pub fn clear_turn_grants(&self) {
        self.shell_granted_this_turn.store(false, Ordering::Relaxed);
        self.shell_grant_turn.store(0, Ordering::Relaxed);
    }

    /// Start a new host turn, auto-clearing a stale shell grant.
    ///
    /// When a grant from an older turn is still set, it is cleared and a
    /// `tracing::warn!` is emitted (the host forgot to clear between turns).
    /// Grants issued for `turn` itself are kept. `turn` must be monotonic;
    /// `0` is reserved for "no turn".
    pub fn begin_turn(&self, turn: u64) {
        self.turn_seq.store(turn, Ordering::Relaxed);
        let grant_turn = self.shell_grant_turn.load(Ordering::Relaxed);
        if self.shell_granted_this_turn.load(Ordering::Relaxed)
            && grant_turn != 0
            && grant_turn != turn
        {
            tracing::warn!(
                grant_turn,
                current_turn = turn,
                "stale shell turn-grant auto-cleared (host should call clear_turn_grants/begin_turn per turn)"
            );
            self.shell_granted_this_turn.store(false, Ordering::Relaxed);
            self.shell_grant_turn.store(0, Ordering::Relaxed);
        }
    }

    /// Clear a grant older than `current_turn` (manual expiry check).
    ///
    /// Returns `true` when an expired grant was cleared (and warns). Grants
    /// for `current_turn` itself are kept. Hosts without a `begin_turn` hook
    /// should call this at turn boundaries. `#[must_use]` so callers notice
    /// whether a creep was actually cleaned up.
    #[must_use = "check whether an expired grant was cleared"]
    pub fn clear_expired_grants(&self, current_turn: u64) -> bool {
        self.turn_seq.store(current_turn, Ordering::Relaxed);
        let grant_turn = self.shell_grant_turn.load(Ordering::Relaxed);
        if self.shell_granted_this_turn.load(Ordering::Relaxed)
            && grant_turn != 0
            && grant_turn != current_turn
        {
            tracing::warn!(
                grant_turn,
                current_turn,
                "expired shell turn-grant cleared (older than one turn)"
            );
            self.shell_granted_this_turn.store(false, Ordering::Relaxed);
            self.shell_grant_turn.store(0, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Record that the user approved a shell tool this turn.
    ///
    /// Further shell tools skip the confirm UI only; path/shell/network hard
    /// gates and the bash deny list still run. Stamped with the current
    /// [`ToolRuntime::begin_turn`] sequence (or `1` when the host never calls
    /// `begin_turn`) so [`ToolRuntime::clear_expired_grants`] can detect creep.
    pub fn grant_shell_this_turn(&self) {
        let turn = self.turn_seq.load(Ordering::Relaxed).max(1);
        self.shell_granted_this_turn.store(true, Ordering::Relaxed);
        self.shell_grant_turn.store(turn, Ordering::Relaxed);
    }

    /// Record a shell grant for an explicit turn (hosts that track turns).
    pub fn grant_shell_for_turn(&self, turn: u64) {
        self.turn_seq.store(turn.max(1), Ordering::Relaxed);
        self.shell_granted_this_turn.store(true, Ordering::Relaxed);
        self.shell_grant_turn.store(turn.max(1), Ordering::Relaxed);
    }

    /// Whether shell was already approved once this turn.
    #[must_use]
    pub fn shell_granted_this_turn(&self) -> bool {
        self.shell_granted_this_turn.load(Ordering::Relaxed)
    }

    /// Turn sequence the current shell grant was issued in (`0` = no grant).
    ///
    /// Exposed for host diagnostics / tests of grant-age creep.
    #[must_use]
    pub fn shell_grant_turn(&self) -> u64 {
        self.shell_grant_turn.load(Ordering::Relaxed)
    }

    fn next_pending_id(&self) -> String {
        let n = self.pending_seq.fetch_add(1, Ordering::Relaxed);
        format!("p-{n}-{}", now_ms())
    }

    /// True when this invoke may skip the confirm UI (one-shot grant or
    /// turn-scoped shell grant after a prior yes).
    fn effective_skip_confirmation(&self, meta: &ToolMeta, opts: InvokeOptions) -> bool {
        if opts.skip_confirmation {
            return true;
        }
        meta.permissions.contains(&Permission::Shell)
            && self.shell_granted_this_turn.load(Ordering::Relaxed)
    }

    /// Policy-only decision (no execute). Used to plan parallel batches.
    ///
    /// Hard gates (path / shell / network) always apply. `skip_confirmation`
    /// only collapses [`PolicyDecision::NeedsConfirmation`] → Allow.
    pub fn decide_only(
        &self,
        tool: &dyn Tool,
        args: &Value,
        opts: InvokeOptions,
    ) -> PolicyDecision {
        let meta = tool.meta();
        if !args.is_object() {
            return PolicyDecision::Deny {
                reason: "tool args must be a JSON object".into(),
            };
        }
        let args = args.clone();
        apply_skip_confirmation(
            decide(&self.policy, &meta, &args, opts.confirms_used),
            self.effective_skip_confirmation(&meta, opts),
        )
    }

    /// Run policy + optional execute for one tool (async).
    pub async fn invoke(
        &self,
        tool: &dyn Tool,
        inv: ToolInvocation,
        opts: InvokeOptions,
    ) -> InvokeResult {
        let meta = tool.meta();
        if let Some(obs) = reject_invalid_args(tool, &inv.args) {
            // Invalid args never reach policy/execute, but must still audit
            // (decision=rejected) so the audit trail has no silent drops.
            let kind = obs
                .error
                .as_ref()
                .map(|e| e.code.as_str())
                .unwrap_or("invalid_args");
            self.audit_event_full(
                &inv,
                &meta,
                "rejected",
                None,
                Some(false),
                Some(kind),
                false,
                obs.bytes,
            );
            return InvokeResult::Observation(obs.to_provider_text());
        }
        let args = inv.args.clone();

        // Always evaluate hard gates; HITL grant only skips the confirm UI branch.
        let decision = apply_skip_confirmation(
            decide(&self.policy, &meta, &args, opts.confirms_used),
            self.effective_skip_confirmation(&meta, opts),
        );

        match decision {
            PolicyDecision::Deny { reason } => {
                self.audit_event(&inv, &meta, "deny", None, Some(false), Some("denied"));
                InvokeResult::Denied { reason }
            }
            PolicyDecision::NeedsConfirmation { reason: _ } => {
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
                let speak_prompt = speak_confirm_prompt(&pending);
                InvokeResult::NeedsConfirmation {
                    pending,
                    speak_prompt,
                }
            }
            PolicyDecision::Allow if meta.collects_input => self.pause_input(inv, meta, args),
            PolicyDecision::Allow => self.execute_allowed(tool, inv, meta, args, opts).await,
        }
    }

    fn pause_input(&self, inv: ToolInvocation, meta: ToolMeta, args: Value) -> InvokeResult {
        let (kind, spoken, label, max_chars) = parse_collect_args(&args);
        let pending = PendingToolCall::new(
            self.next_pending_id(),
            inv.name.clone(),
            args,
            args_summary(&inv.name, &inv.args),
            meta.risk,
            inv.call_id.clone(),
        )
        .with_input(PendingInput {
            kind,
            label,
            max_chars,
        });
        self.audit_event(&inv, &meta, "input", None, None, Some("needs_input"));
        InvokeResult::NeedsInput {
            pending,
            speak_prompt: spoken,
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
        let _permits = self
            .gate
            .acquire(tool.name(), meta.effective_max_concurrency())
            .await;
        let started = Instant::now();
        let result = run_with_timeout(tool, &ctx, args, meta.default_timeout).await;
        let duration_ms = started.elapsed().as_millis() as u64;

        let budget = meta.result_char_budget();
        match result {
            Ok(output) => {
                let cut = truncate_tool_result_detailed(output, budget);
                let bytes = cut.text.len();
                let truncated = cut.truncated;
                let structured =
                    ToolObservation::from_text(cut.text, duration_ms, truncated, cut.cursor);
                let obs = structured.to_provider_text();
                self.audit_event_full(
                    &inv,
                    &meta,
                    decision_label,
                    Some(duration_ms),
                    Some(true),
                    None,
                    truncated,
                    bytes,
                );
                InvokeResult::Observation(obs)
            }
            Err(e) => {
                let timed_out = is_timeout(&e);
                let kind = if timed_out { "timeout" } else { "error" };
                let decision = if timed_out { "timeout" } else { decision_label };
                self.audit_event_full(
                    &inv,
                    &meta,
                    decision,
                    Some(duration_ms),
                    Some(false),
                    Some(kind),
                    false,
                    0,
                );
                let bounded = bound_error_message(&e.message, MAX_ERROR_MESSAGE_CHARS);
                let structured = ToolObservation::err(
                    crate::tool::ObservationError::new(kind, timed_out, bounded),
                    duration_ms,
                );
                let obs = structured.to_provider_text();
                InvokeResult::Observation(obs)
            }
        }
    }

    /// Audit helper for unknown-tool soft-fails.
    ///
    /// `loop_/tool_batch.rs` (owned by another slice) soft-fails unknown tools
    /// without a `&dyn Tool`, so it cannot go through [`ToolRuntime::invoke`].
    /// The loop should call this so unknown tools still audit
    /// (`decision=rejected`, `error_kind=unknown_tool`) instead of vanishing
    /// from the trail. `risk` defaults to `"unknown"` when the caller has no
    /// metadata.
    pub fn audit_unknown_tool(
        &self,
        name: &str,
        args: &Value,
        session_id: Option<&str>,
        turn_id: Option<&str>,
    ) {
        self.audit.write(&AuditEvent {
            ts_ms: now_ms(),
            session_id: session_id.map(|s| s.to_string()),
            turn_id: turn_id.map(|s| s.to_string()),
            tool: name.to_string(),
            risk: "unknown".into(),
            decision: "rejected".into(),
            args_digest: args_digest(args),
            ok: Some(false),
            duration_ms: None,
            error_kind: Some("unknown_tool".into()),
            truncated: false,
            bytes: 0,
        });
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
            truncated: false,
            bytes: 0,
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
        self.audit_event_full(inv, meta, decision, duration_ms, ok, error_kind, false, 0);
    }

    #[allow(clippy::too_many_arguments)]
    fn audit_event_full(
        &self,
        inv: &ToolInvocation,
        meta: &ToolMeta,
        decision: &str,
        duration_ms: Option<u64>,
        ok: Option<bool>,
        error_kind: Option<&str>,
        truncated: bool,
        bytes: usize,
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
            truncated,
            bytes,
        });
    }
}

fn reject_invalid_args(tool: &dyn Tool, args: &Value) -> Option<ToolObservation> {
    let raw = args.to_string();
    match validate_args(&tool.parameters(), args, &raw) {
        Ok(()) => None,
        Err(inv) => Some(ToolObservation::invalid_args(inv)),
    }
}

/// After HITL grant, only skip the confirmation branch — never path/shell/network denials.
fn apply_skip_confirmation(decision: PolicyDecision, skip: bool) -> PolicyDecision {
    match decision {
        PolicyDecision::NeedsConfirmation { .. } if skip => PolicyDecision::Allow,
        other => other,
    }
}

/// Short voice-friendly confirm line. Prefer the shell command itself when
/// present so TTS stays snappy ("Run git status?" vs a long args dump).
fn speak_confirm_prompt(pending: &PendingToolCall) -> String {
    if pending.name == "bash" {
        if let Some(cmd) = pending
            .args
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let cmd = truncate_voice_chars(cmd, 56);
            return format!("Run `{cmd}`?");
        }
    }
    let summary = truncate_voice_chars(&pending.args_summary, 64);
    format!("Run {summary}?")
}

fn truncate_voice_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let head: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Max chars for a tool error message (mirrors the Ok-path head+tail style).
pub(crate) const MAX_ERROR_MESSAGE_CHARS: usize = 2_000;

/// Bound an error message to [`MAX_ERROR_MESSAGE_CHARS`] chars with head+tail
/// and a marker (char-safe). Policy/confirm logic is untouched.
fn bound_error_message(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    const MARKER: &str = "\n…[truncated; error message shortened]…\n";
    let marker_chars = MARKER.chars().count();
    if max_chars <= marker_chars {
        return s.chars().take(max_chars).collect();
    }
    let keep = max_chars - marker_chars;
    let head_chars = keep / 2;
    let tail_chars = keep - head_chars;
    let head: String = s.chars().take(head_chars).collect();
    let tail: String = s
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{head}{MARKER}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::audit::MemoryAuditSink;
    use crate::tool::{Tool, ToolError, ToolMeta, ToolRisk, MAX_TOOL_RESULT_CHARS};
    use crate::tool_context::ToolCallContext;
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
        fn meta(&self) -> ToolMeta {
            ToolMeta::safe_default()
        }
        async fn execute(&self, _ctx: &ToolCallContext, _args: Value) -> Result<String, ToolError> {
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
        async fn execute(&self, _ctx: &ToolCallContext, _args: Value) -> Result<String, ToolError> {
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
        match rt
            .invoke(&LongTool, inv("1", "long"), InvokeOptions::default())
            .await
        {
            InvokeResult::Observation(s) => {
                assert!(s.contains("[truncated"));
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

    struct ShellOnceTool {
        ran: Mutex<u32>,
    }

    #[async_trait]
    impl Tool for ShellOnceTool {
        fn name(&self) -> &str {
            "bash"
        }
        fn description(&self) -> &str {
            "shell"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]})
        }
        fn meta(&self) -> ToolMeta {
            ToolMeta::with_risk(ToolRisk::Dangerous)
                .permissions(&[crate::tool::Permission::Shell])
                .confirm(true)
        }
        async fn execute(&self, _ctx: &ToolCallContext, _args: Value) -> Result<String, ToolError> {
            *self.ran.lock().unwrap() += 1;
            Ok("ok".into())
        }
    }

    #[tokio::test]
    async fn shell_turn_grant_skips_later_shell_hitl() {
        let policy = SandboxConfig {
            shell: crate::runtime::ShellPolicy::OpenConfirm,
            ..Default::default()
        };
        let rt = ToolRuntime::new(policy, Box::new(NullAuditSink));
        let tool = ShellOnceTool { ran: Mutex::new(0) };

        // First call still needs confirm.
        let args = json!({ "command": "echo a" });
        match rt
            .invoke(
                &tool,
                ToolInvocation::new("c1", "bash", args.clone()),
                InvokeOptions::default(),
            )
            .await
        {
            InvokeResult::NeedsConfirmation { speak_prompt, .. } => {
                assert!(
                    speak_prompt.contains("echo a") || speak_prompt.contains('?'),
                    "expected short speak prompt, got: {speak_prompt}"
                );
            }
            other => panic!("expected confirm, got {other:?}"),
        }

        // After turn grant, later shell skips HITL (hard gates still run).
        rt.grant_shell_this_turn();
        match rt
            .invoke(
                &tool,
                ToolInvocation::new("c2", "bash", json!({ "command": "echo b" })),
                InvokeOptions::default(),
            )
            .await
        {
            InvokeResult::Observation(s) => {
                assert!(s.starts_with("ok"));
                assert_eq!(*tool.ran.lock().unwrap(), 1);
            }
            other => panic!("expected auto-run after shell grant, got {other:?}"),
        }

        // Clear restores confirm requirement.
        rt.clear_turn_grants();
        match rt
            .invoke(
                &tool,
                ToolInvocation::new("c3", "bash", args),
                InvokeOptions::default(),
            )
            .await
        {
            InvokeResult::NeedsConfirmation { .. } => {}
            other => panic!("expected confirm after clear, got {other:?}"),
        }
    }

    #[test]
    fn speak_confirm_prompt_prefers_bash_command() {
        let pending = PendingToolCall::new(
            "p1",
            "bash",
            json!({ "command": "git status" }),
            "bash (command=git status)",
            ToolRisk::Dangerous,
            "c1",
        );
        let s = speak_confirm_prompt(&pending);
        assert_eq!(s, "Run `git status`?");
    }

    #[tokio::test]
    async fn grant_still_enforces_path_policy() {
        use crate::tool::{Permission, ToolKind};
        use std::path::PathBuf;

        struct PathTool;
        #[async_trait]
        impl Tool for PathTool {
            fn name(&self) -> &str {
                "path_read"
            }
            fn description(&self) -> &str {
                "r"
            }
            fn parameters(&self) -> Value {
                json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::with_risk(ToolRisk::Dangerous)
                    .kind(ToolKind::Read)
                    .permissions(&[Permission::FsRead])
                    .confirm(true)
                    .read_only(true)
            }
            async fn execute(
                &self,
                _ctx: &ToolCallContext,
                _args: Value,
            ) -> Result<String, ToolError> {
                Ok("should-not-run".into())
            }
        }

        let policy = SandboxConfig {
            sandbox_root: PathBuf::from("C:\\Users\\me\\.boris\\sandbox"),
            boris_data_roots: vec![],
            allow_read: vec![],
            allow_write: vec![],
            network: crate::runtime::NetworkPolicy::Off,
            shell: crate::runtime::ShellPolicy::Denied,
            auto_allow_up_to: ToolRisk::Moderate,
            force_confirm_at_or_above: ToolRisk::Dangerous,
            max_confirms_per_turn: 12,
            trusted_auto_moderate: false,
        };
        let rt = ToolRuntime::new(policy, Box::new(MemoryAuditSink::new()));
        let inv = ToolInvocation::new(
            "1",
            "path_read",
            json!({ "path": "C:\\Windows\\System32\\config" }),
        );
        let opts = InvokeOptions {
            skip_confirmation: true,
            confirms_used: 1,
        };
        match rt.invoke(&PathTool, inv, opts).await {
            InvokeResult::Denied { reason } => {
                assert!(reason.contains("outside") || reason.contains("path"));
            }
            other => panic!("expected Denied after grant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_args_are_not_coerced() {
        struct NeedName;
        #[async_trait]
        impl Tool for NeedName {
            fn name(&self) -> &str {
                "need"
            }
            fn description(&self) -> &str {
                "n"
            }
            fn parameters(&self) -> Value {
                json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]})
            }
            async fn execute(
                &self,
                _ctx: &ToolCallContext,
                _args: Value,
            ) -> Result<String, ToolError> {
                Ok("should-not-run".into())
            }
        }
        let rt = ToolRuntime::null();
        match rt
            .invoke(
                &NeedName,
                ToolInvocation::new("1", "need", json!("not-an-object")),
                InvokeOptions::default(),
            )
            .await
        {
            InvokeResult::Observation(s) => {
                assert!(s.contains("Error ["), "{s}");
                assert!(s.contains("not_object") || s.contains("object"), "{s}");
                assert!(!s.contains("should-not-run"));
            }
            other => panic!("unexpected {other:?}"),
        }
        match rt
            .invoke(
                &NeedName,
                ToolInvocation::new("2", "need", json!({"n": 1})),
                InvokeOptions::default(),
            )
            .await
        {
            InvokeResult::Observation(s) => {
                assert!(s.contains("missing_required") || s.contains("name"), "{s}");
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
            async fn execute(&self, _ctx: &ToolCallContext, _: Value) -> Result<String, ToolError> {
                tokio::time::sleep(Duration::from_secs(2)).await;
                Ok("nope".into())
            }
        }
        let rt = ToolRuntime::null();
        match rt
            .invoke(&Slow, inv("1", "slow"), InvokeOptions::default())
            .await
        {
            InvokeResult::Observation(s) => assert!(s.contains("timed out") || s.contains("Error")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn bound_error_message_keeps_head_and_tail_within_budget() {
        let long = format!(
            "HEAD-{}MIDDLE{}TAIL-MARKER-ZZ",
            "a".repeat(3_000),
            "z".repeat(3_000)
        );
        let out = bound_error_message(&long, MAX_ERROR_MESSAGE_CHARS);
        assert!(out.chars().count() <= MAX_ERROR_MESSAGE_CHARS);
        assert!(out.contains("HEAD-"));
        assert!(out.contains("TAIL-MARKER-ZZ"));
        assert!(out.contains("[truncated"));
        assert_eq!(bound_error_message("short", MAX_ERROR_MESSAGE_CHARS), "short");
    }

    #[tokio::test]
    async fn invoke_bounds_long_error_messages() {
        struct FailBig;
        #[async_trait]
        impl Tool for FailBig {
            fn name(&self) -> &str {
                "failbig"
            }
            fn description(&self) -> &str {
                "fails big"
            }
            fn parameters(&self) -> Value {
                json!({"type":"object","properties":{}})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::safe_default()
            }
            async fn execute(
                &self,
                _ctx: &ToolCallContext,
                _: Value,
            ) -> Result<String, crate::tool::ToolError> {
                Err(crate::tool::ToolError::new(format!(
                    "boom {} tail-marker-zz",
                    "e".repeat(5_000)
                )))
            }
        }
        let rt = ToolRuntime::null();
        match rt
            .invoke(
                &FailBig,
                inv("1", "failbig"),
                InvokeOptions::default(),
            )
            .await
        {
            InvokeResult::Observation(s) => {
                assert!(s.contains("Error"), "{s}");
                assert!(s.contains("[truncated"), "{s}");
                assert!(s.contains("tail-marker-zz"), "{s}");
                // Provider text stays bounded (message 2k + framing).
                assert!(s.chars().count() <= MAX_ERROR_MESSAGE_CHARS + 512, "{s}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn invalid_args_are_audited_as_rejected() {
        struct NeedName;
        #[async_trait]
        impl Tool for NeedName {
            fn name(&self) -> &str {
                "need"
            }
            fn description(&self) -> &str {
                "n"
            }
            fn parameters(&self) -> Value {
                json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]})
            }
            async fn execute(
                &self,
                _ctx: &ToolCallContext,
                _args: Value,
            ) -> Result<String, ToolError> {
                Ok("should-not-run".into())
            }
        }
        let sink = std::sync::Arc::new(MemoryAuditSink::new());
        // MemoryAuditSink is behind Box; use a shared Arc via a tiny wrapper.
        struct Shared(std::sync::Arc<MemoryAuditSink>);
        impl crate::runtime::audit::AuditSink for Shared {
            fn write(&self, e: &crate::runtime::audit::AuditEvent) {
                self.0.write(e);
            }
        }
        let rt = ToolRuntime::new(
            SandboxConfig::default(),
            Box::new(Shared(sink.clone())),
        );
        match rt
            .invoke(
                &NeedName,
                ToolInvocation::new("1", "need", json!({"n": 1})),
                InvokeOptions::default(),
            )
            .await
        {
            InvokeResult::Observation(s) => assert!(s.contains("Error"), "{s}"),
            other => panic!("unexpected {other:?}"),
        }
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1, "invalid-args must audit, got {events:?}");
        assert_eq!(events[0].decision, "rejected");
        assert_eq!(events[0].tool, "need");
        assert!(events[0].error_kind.is_some(), "{:?}", events[0]);
    }

    #[test]
    fn unknown_tool_helper_audits_rejected() {
        let sink = MemoryAuditSink::new();
        let rt = ToolRuntime::new(SandboxConfig::default(), Box::new(NullAuditSink));
        // Call helper with a fresh runtime wired to memory sink.
        let mem = std::sync::Arc::new(MemoryAuditSink::new());
        struct Shared2(std::sync::Arc<MemoryAuditSink>);
        impl crate::runtime::audit::AuditSink for Shared2 {
            fn write(&self, e: &crate::runtime::audit::AuditEvent) {
                self.0.write(e);
            }
        }
        let rt2 = ToolRuntime::new(SandboxConfig::default(), Box::new(Shared2(mem.clone())));
        rt2.audit_unknown_tool(
            "nope_tool",
            &json!({"a": 1}),
            Some("s1"),
            Some("t1"),
        );
        let events = mem.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].decision, "rejected");
        assert_eq!(events[0].error_kind.as_deref(), Some("unknown_tool"));
        assert_eq!(events[0].tool, "nope_tool");
        let _ = (sink, rt);
    }

    #[tokio::test]
    async fn truncated_observations_record_flag_and_bytes() {
        let audit = std::sync::Arc::new(MemoryAuditSink::new());
        struct Shared3(std::sync::Arc<MemoryAuditSink>);
        impl crate::runtime::audit::AuditSink for Shared3 {
            fn write(&self, e: &crate::runtime::audit::AuditEvent) {
                self.0.write(e);
            }
        }
        let rt = ToolRuntime::new(
            SandboxConfig::default(),
            Box::new(Shared3(audit.clone())),
        );
        match rt
            .invoke(&LongTool, inv("1", "long"), InvokeOptions::default())
            .await
        {
            InvokeResult::Observation(_) => {}
            other => panic!("unexpected {other:?}"),
        }
        let events = audit.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].truncated, "long output must set truncated=true");
        assert!(events[0].bytes > 0, "{:?}", events[0]);
    }

    #[test]
    fn turn_grant_expiry_clears_stale_grant() {
        let rt = ToolRuntime::null();
        rt.begin_turn(1);
        rt.grant_shell_for_turn(1);
        assert!(rt.shell_granted_this_turn());
        assert_eq!(rt.shell_grant_turn(), 1);
        // Same turn: no expiry.
        assert!(!rt.clear_expired_grants(1));
        assert!(rt.shell_granted_this_turn());
        // Next turn: expired grant clears (and warns).
        assert!(rt.clear_expired_grants(2));
        assert!(!rt.shell_granted_this_turn());
        assert_eq!(rt.shell_grant_turn(), 0);
    }

    #[test]
    fn begin_turn_auto_clears_stale_grant() {
        let rt = ToolRuntime::null();
        rt.begin_turn(7);
        rt.grant_shell_this_turn();
        assert!(rt.shell_granted_this_turn());
        rt.begin_turn(8);
        assert!(
            !rt.shell_granted_this_turn(),
            "begin_turn(8) must auto-clear grant from turn 7"
        );
        // Grant in the same turn survives begin_turn for that turn.
        rt.begin_turn(8);
        rt.grant_shell_for_turn(8);
        rt.begin_turn(8);
        assert!(rt.shell_granted_this_turn());
    }
}
