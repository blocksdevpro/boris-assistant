//! User hooks: shell commands the user lists under `[[hooks]]` in
//! `~/.boris/config.toml`, run around tool calls and at the end of a turn.
//!
//! **Hooks run arbitrary user commands with the user's own rights.** They are
//! a customization point, not a sandbox, and they add no protection beyond
//! what the command itself does.
//!
//! | Event | When | Effect |
//! |-------|------|--------|
//! | `pre_tool` | after policy and HITL approval, right before execute | non-zero exit, timeout, or spawn failure blocks the call; stderr becomes the tool error |
//! | `post_tool` | after execute | informational; failures are logged |
//! | `turn_end` | after the agent finishes a turn | informational, runs in the background |
//!
//! Each hook gets one JSON object on stdin and has a timeout (default
//! 10 s, at most 120 s). A blocked call never reaches `execute`, and hooks
//! never bypass the runtime's hard path / shell / network gates, which run
//! first. With no hooks configured nothing changes and no process starts.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 10;
pub const MAX_HOOK_TIMEOUT_SECS: u64 = 120;
/// Longest hook stderr returned to the model.
const MAX_HOOK_MESSAGE_CHARS: usize = 2_000;
/// Most hook stderr bytes kept in memory; the rest is drained and dropped.
const MAX_HOOK_STDERR_BYTES: u64 = 16 * 1024;
/// Longest tool output passed to `post_tool` hooks.
const MAX_HOOK_OUTPUT_CHARS: usize = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    PreTool,
    PostTool,
    TurnEnd,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreTool => "pre_tool",
            Self::PostTool => "post_tool",
            Self::TurnEnd => "turn_end",
        }
    }
}

/// One `[[hooks]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookSpec {
    pub event: HookEvent,
    /// Tool-name glob (`*`, `?`) for `pre_tool` / `post_tool`; all tools when unset.
    #[serde(default)]
    pub tool: Option<String>,
    /// Shell command, run like the `bash` tool's shell (Git Bash, else PowerShell).
    pub command: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    DEFAULT_HOOK_TIMEOUT_SECS
}

/// The configured hooks. Cheap to clone; empty by default.
#[derive(Debug, Clone, Default)]
pub struct Hooks {
    specs: Arc<[HookSpec]>,
    cwd: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
enum HookRun {
    Exited {
        success: bool,
        code: Option<i32>,
        stderr: String,
    },
    TimedOut,
    SpawnFailed(String),
}

/// Facts about one tool call, for `pre_tool` / `post_tool` payloads.
#[derive(Debug, Clone, Copy)]
pub struct HookCall<'a> {
    pub tool: &'a str,
    pub call_id: &'a str,
    pub args: &'a Value,
    pub session_id: Option<&'a str>,
    pub turn_id: Option<&'a str>,
}

impl Hooks {
    /// Hooks with a blank command are dropped.
    pub fn new(specs: Vec<HookSpec>) -> Self {
        let specs: Vec<HookSpec> = specs
            .into_iter()
            .filter(|spec| !spec.command.trim().is_empty())
            .map(|mut spec| {
                spec.timeout_secs = spec.timeout_secs.clamp(1, MAX_HOOK_TIMEOUT_SECS);
                spec
            })
            .collect();
        Self {
            specs: specs.into(),
            cwd: None,
        }
    }

    /// Working directory for hook commands (default: the OS temp dir).
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.specs.len()
    }

    fn matching<'s>(
        &'s self,
        event: HookEvent,
        tool: Option<&'s str>,
    ) -> impl Iterator<Item = &'s HookSpec> + 's {
        self.specs.iter().filter(move |spec| {
            spec.event == event
                && match (spec.tool.as_deref(), tool) {
                    (None, _) | (_, None) => true,
                    (Some(pattern), Some(name)) => glob_match(pattern, name),
                }
        })
    }

    /// Run matching `pre_tool` hooks in order. `Err` carries the reason the
    /// call is blocked (the first hook that exits non-zero, times out, or
    /// cannot start).
    pub async fn pre_tool(&self, call: HookCall<'_>) -> Result<(), String> {
        if self.is_empty() {
            return Ok(());
        }
        let input = json!({
            "event": "pre_tool",
            "tool": call.tool,
            "call_id": call.call_id,
            "args": call.args,
            "session_id": call.session_id,
            "turn_id": call.turn_id,
        });
        for spec in self.matching(HookEvent::PreTool, Some(call.tool)) {
            let reason = match self.run(spec, &input).await {
                HookRun::Exited { success: true, .. } => continue,
                HookRun::Exited { code, stderr, .. } if stderr.is_empty() => format!(
                    "Blocked by a pre_tool hook (`{}` exited with {}).",
                    spec.command,
                    code.map_or_else(|| "a signal".to_string(), |c| format!("code {c}"))
                ),
                HookRun::Exited { stderr, .. } => format!("Blocked by a pre_tool hook: {stderr}"),
                HookRun::TimedOut => format!(
                    "Blocked: the pre_tool hook `{}` timed out after {} s.",
                    spec.command, spec.timeout_secs
                ),
                HookRun::SpawnFailed(error) => {
                    format!(
                        "Blocked: the pre_tool hook `{}` could not run: {error}",
                        spec.command
                    )
                }
            };
            tracing::info!(tool = call.tool, hook = %spec.command, %reason, "pre_tool hook blocked a call");
            return Err(bound(&reason, MAX_HOOK_MESSAGE_CHARS));
        }
        Ok(())
    }

    /// Run matching `post_tool` hooks. Failures are logged only.
    pub async fn post_tool(&self, call: HookCall<'_>, ok: bool, output: &str) {
        if self.is_empty() {
            return;
        }
        let input = json!({
            "event": "post_tool",
            "tool": call.tool,
            "call_id": call.call_id,
            "args": call.args,
            "session_id": call.session_id,
            "turn_id": call.turn_id,
            "ok": ok,
            "output": bound(output, MAX_HOOK_OUTPUT_CHARS),
        });
        for spec in self.matching(HookEvent::PostTool, Some(call.tool)) {
            self.log_failure(spec, self.run(spec, &input).await);
        }
    }

    /// Run `turn_end` hooks with `payload` (an object; `event` is added).
    pub async fn turn_end(&self, mut payload: Value) {
        if self.is_empty() {
            return;
        }
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("event".into(), json!("turn_end"));
        }
        for spec in self.matching(HookEvent::TurnEnd, None) {
            self.log_failure(spec, self.run(spec, &payload).await);
        }
    }

    fn log_failure(&self, spec: &HookSpec, run: HookRun) {
        if !matches!(run, HookRun::Exited { success: true, .. }) {
            tracing::warn!(event = spec.event.as_str(), hook = %spec.command, ?run, "hook failed");
        }
    }

    async fn run(&self, spec: &HookSpec, input: &Value) -> HookRun {
        use std::process::Stdio;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let cwd = self.cwd.clone().unwrap_or_else(std::env::temp_dir);
        let mut std_cmd = crate::tools::bash::hook_command(&spec.command, &cwd);
        crate::tools::bash::TreeGuard::prepare(&mut std_cmd);
        let mut cmd = tokio::process::Command::from(std_cmd);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => return HookRun::SpawnFailed(error.to_string()),
        };
        // A timed-out hook's whole tree dies, not just the shell. Without a
        // tree handle the hook does not run (fail closed; on Windows it is
        // still suspended and dropping it kills it).
        let tree = match crate::tools::bash::TreeGuard::attach(&child) {
            Ok(tree) => tree,
            Err(error) => {
                return HookRun::SpawnFailed(format!(
                    "could not track the hook's processes: {error}"
                ))
            }
        };
        let body = input.to_string();
        let mut stdin = child.stdin.take();
        let stderr = child.stderr.take();
        let work = async move {
            if let Some(stdin) = stdin.as_mut() {
                // A hook that ignores stdin may close it early; that is fine.
                let _ = stdin.write_all(body.as_bytes()).await;
                let _ = stdin.shutdown().await;
            }
            drop(stdin);
            // Keep a bounded prefix of stderr; drain the rest so the hook
            // never blocks on a full pipe.
            let mut err = Vec::new();
            if let Some(mut stderr) = stderr {
                let _ = (&mut stderr)
                    .take(MAX_HOOK_STDERR_BYTES)
                    .read_to_end(&mut err)
                    .await;
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            }
            child.wait().await.map(|status| (status, err))
        };
        // On timeout the future (and the child, kill_on_drop) is dropped and
        // the tree guard kills anything the hook started.
        match tokio::time::timeout(Duration::from_secs(spec.timeout_secs), work).await {
            Ok(Ok((status, stderr))) => {
                tree.disarm();
                HookRun::Exited {
                    success: status.success(),
                    code: status.code(),
                    stderr: String::from_utf8_lossy(&stderr).trim().to_string(),
                }
            }
            Ok(Err(error)) => HookRun::SpawnFailed(error.to_string()),
            Err(_) => HookRun::TimedOut,
        }
    }
}

fn bound(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// `*` matches any run, `?` one character; case-sensitive like tool names.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(event: HookEvent, tool: Option<&str>, command: &str) -> HookSpec {
        HookSpec {
            event,
            tool: tool.map(Into::into),
            command: command.into(),
            timeout_secs: 10,
        }
    }

    fn call<'a>(tool: &'a str, args: &'a Value) -> HookCall<'a> {
        HookCall {
            tool,
            call_id: "c1",
            args,
            session_id: None,
            turn_id: None,
        }
    }

    fn real_bash() -> bool {
        crate::tools::bash::shell_label().starts_with("bash")
    }

    #[test]
    fn globs_match_tool_names() {
        assert!(glob_match("*", "bash"));
        assert!(glob_match("file_*", "file_write"));
        assert!(!glob_match("file_*", "bash"));
        assert!(glob_match("ba?h", "bash"));
        assert!(glob_match("*_set", "timer_set"));
        assert!(!glob_match("Bash", "bash"));
    }

    #[test]
    fn config_entries_parse_with_defaults() {
        let parsed: HookSpec =
            serde_json::from_value(json!({"event": "pre_tool", "command": "check.sh"})).unwrap();
        assert_eq!(parsed.tool, None);
        assert_eq!(parsed.timeout_secs, DEFAULT_HOOK_TIMEOUT_SECS);
        let hooks = Hooks::new(vec![
            spec(HookEvent::PreTool, None, "  "),
            HookSpec {
                timeout_secs: 10_000,
                ..spec(HookEvent::TurnEnd, None, "x")
            },
        ]);
        assert_eq!(hooks.len(), 1, "blank commands are dropped");
        assert_eq!(hooks.specs[0].timeout_secs, MAX_HOOK_TIMEOUT_SECS);
    }

    #[tokio::test]
    async fn empty_hooks_allow_everything() {
        let args = json!({});
        assert_eq!(Hooks::default().pre_tool(call("bash", &args)).await, Ok(()));
    }

    #[tokio::test]
    async fn failing_pre_tool_hook_blocks_with_its_stderr() {
        let args = json!({"command": "rm -rf /"});
        let command = if real_bash() {
            "cat >/dev/null; echo 'no shell today' >&2; exit 3"
        } else {
            "[Console]::Error.WriteLine('no shell today'); exit 3"
        };
        let hooks = Hooks::new(vec![spec(HookEvent::PreTool, Some("bash"), command)]);
        let reason = hooks.pre_tool(call("bash", &args)).await.unwrap_err();
        assert!(reason.contains("no shell today"), "{reason}");
        // The glob does not match other tools.
        assert_eq!(hooks.pre_tool(call("file_read", &args)).await, Ok(()));
    }

    #[tokio::test]
    async fn pre_tool_hook_reads_the_call_on_stdin() {
        if !real_bash() {
            return; // stdin plumbing check needs a POSIX shell
        }
        let args = json!({"path": "notes.md"});
        let hooks = Hooks::new(vec![spec(
            HookEvent::PreTool,
            None,
            "grep -q '\"tool\":\"file_write\"' && exit 0; exit 1",
        )]);
        assert_eq!(hooks.pre_tool(call("file_write", &args)).await, Ok(()));
        assert!(hooks.pre_tool(call("file_edit", &args)).await.is_err());
    }

    struct Probe {
        ran: std::sync::Arc<std::sync::atomic::AtomicBool>,
        confirm: bool,
    }

    #[async_trait::async_trait]
    impl crate::Tool for Probe {
        fn name(&self) -> &str {
            if self.confirm {
                "probe_write"
            } else {
                "probe"
            }
        }
        fn description(&self) -> &str {
            "probe"
        }
        fn parameters(&self) -> Value {
            json!({"type": "object", "properties": {}})
        }
        fn meta(&self) -> crate::ToolMeta {
            if self.confirm {
                crate::ToolMeta::with_risk(crate::ToolRisk::Dangerous)
                    .confirm(true)
                    .read_only(false)
            } else {
                crate::ToolMeta::safe_default()
            }
        }
        async fn execute(
            &self,
            _ctx: &crate::ToolCallContext,
            _args: Value,
        ) -> Result<String, crate::ToolError> {
            self.ran.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok("ran".into())
        }
    }

    #[tokio::test]
    async fn runtime_blocks_before_execute_and_keeps_failed_status() {
        use crate::runtime::{InvokeOptions, InvokeResult, ToolInvocation};
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tool = Probe {
            ran: ran.clone(),
            confirm: false,
        };
        let mut runtime = crate::ToolRuntime::null();
        runtime.set_hooks(Hooks::new(vec![spec(
            HookEvent::PreTool,
            Some("probe"),
            "exit 1",
        )]));
        let result = runtime
            .invoke(
                &tool,
                ToolInvocation::new("c1", "probe", json!({})),
                InvokeOptions::default(),
            )
            .await;
        let InvokeResult::Observation(obs) = result else {
            panic!("expected an observation");
        };
        assert!(!obs.looks_ok(), "blocked call keeps failed status");
        assert_eq!(obs.error.as_ref().unwrap().code, "hook_blocked");
        assert!(
            !ran.load(std::sync::atomic::Ordering::SeqCst),
            "never executed"
        );

        // HITL comes first: a call that needs a yes/no pauses before any hook.
        let writer = Probe {
            ran: ran.clone(),
            confirm: true,
        };
        runtime.set_hooks(Hooks::new(vec![spec(HookEvent::PreTool, None, "exit 1")]));
        let result = runtime
            .invoke(
                &writer,
                ToolInvocation::new("c2", "probe_write", json!({})),
                InvokeOptions::default(),
            )
            .await;
        assert!(matches!(result, InvokeResult::NeedsConfirmation { .. }));
    }

    #[tokio::test]
    async fn hung_pre_tool_hook_times_out_and_blocks() {
        let command = if real_bash() {
            "sleep 30"
        } else {
            "Start-Sleep 30"
        };
        let hooks = Hooks::new(vec![HookSpec {
            timeout_secs: 1,
            ..spec(HookEvent::PreTool, None, command)
        }]);
        let args = json!({});
        let started = std::time::Instant::now();
        let reason = hooks.pre_tool(call("bash", &args)).await.unwrap_err();
        assert!(reason.contains("timed out"), "{reason}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
