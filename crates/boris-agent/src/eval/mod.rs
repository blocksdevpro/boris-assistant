//! Deterministic agent-loop eval harness (no live provider / audio).
//!
//! Scenarios cover request budgets, tool execution, malformed-args repair,
//! research evidence, note-memory recall, context pruning, empty scripts, and
//! spoken-unit ordering.

use std::sync::Mutex;

use async_trait::async_trait;
use boris_ai::{CompleteOptions, LlmClient, LlmError, LlmStreamEvent};
use serde_json::Value;

#[cfg(test)]
use crate::context::{Context, Role};
#[cfg(test)]
use crate::loop_::{agent_loop, resume_pending_tool, LoopState};
#[cfg(test)]
use crate::runtime::ToolRuntime;
#[cfg(test)]
use crate::tool::{Tool, ToolError, ToolMeta, ToolRisk};
#[cfg(test)]
use crate::types::{AgentLoopConfig, LoopResult};
#[cfg(test)]
use serde_json::json;

/// One scripted LLM response (assembled message).
#[derive(Debug, Clone)]
pub struct ScriptedTurn {
    pub message: Value,
}

/// A recorded `complete` call for assertions.
#[derive(Debug, Clone)]
pub struct RecordedCall {
    pub tools_len: usize,
    pub model: String,
    pub options: CompleteOptions,
}

pub struct ScriptedClient {
    pub model: String,
    responses: Mutex<Vec<Value>>,
    pub calls: Mutex<Vec<RecordedCall>>,
}

impl ScriptedClient {
    pub fn new(model: impl Into<String>, responses: Vec<Value>) -> Self {
        Self {
            model: model.into(),
            responses: Mutex::new(responses),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn complete_recorded(&self, tools: Value, options: CompleteOptions) -> Result<Value, LlmError> {
        self.calls.lock().unwrap().push(RecordedCall {
            tools_len: tools.as_array().map(|a| a.len()).unwrap_or(0),
            model: self.model.clone(),
            options,
        });
        let mut guard = self.responses.lock().unwrap();
        if guard.is_empty() {
            return Err(LlmError::new("empty script"));
        }
        Ok(guard.remove(0))
    }
}

#[async_trait]
impl LlmClient for ScriptedClient {
    async fn complete(&self, _messages: Value, tools: Value) -> Result<Value, LlmError> {
        self.complete_recorded(tools, CompleteOptions::default())
    }

    async fn complete_with_options(
        &self,
        _messages: Value,
        tools: Value,
        options: CompleteOptions,
    ) -> Result<Value, LlmError> {
        self.complete_recorded(tools, options)
    }

    async fn complete_stream(
        &self,
        messages: Value,
        tools: Value,
        opts: CompleteOptions,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<Value, LlmError> {
        on_event(LlmStreamEvent::ModelSend {
            model: self.model.clone(),
        });
        let msg = self.complete_with_options(messages, tools, opts).await?;
        if let Some(s) = msg.get("content").and_then(|c| c.as_str()) {
            if !s.is_empty() {
                on_event(LlmStreamEvent::FirstDelta { ttfb_ms: 1 });
                on_event(LlmStreamEvent::ContentDelta {
                    text: s.to_string(),
                });
            }
        }
        on_event(LlmStreamEvent::FinalMessage(msg.clone()));
        Ok(msg)
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
struct EchoTool {
    executions: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "echo"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"x":{"type":"string"}},"required":["x"]})
    }
    fn meta(&self) -> ToolMeta {
        ToolMeta::safe_default().read_only(true)
    }
    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        self.executions
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(args
            .get("x")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }
}

#[cfg(test)]
struct FailedSearchTool;

#[cfg(test)]
#[async_trait]
impl Tool for FailedSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }
    fn description(&self) -> &str {
        "deterministic failed search"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]})
    }
    fn meta(&self) -> ToolMeta {
        ToolMeta::safe_default().read_only(true)
    }
    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        _args: Value,
    ) -> Result<String, ToolError> {
        Err(ToolError::failed("search backend unavailable"))
    }
}

#[cfg(test)]
struct ConfirmedSideEffectTool {
    executions: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
#[async_trait]
impl Tool for ConfirmedSideEffectTool {
    fn name(&self) -> &str {
        "dangerous_action"
    }

    fn description(&self) -> &str {
        "perform a deterministic side effect after explicit approval"
    }

    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{}})
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Dangerous)
            .confirm(true)
            .read_only(false)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        _args: Value,
    ) -> Result<String, ToolError> {
        self.executions
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok("side effect completed".to_string())
    }
}

#[cfg(test)]
async fn run_loop_full(
    client: &ScriptedClient,
    user: &str,
) -> (
    LoopResult,
    Context,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    let mut context = Context::new(20);
    context.push(Role::System, "sys");
    context.push(Role::User, user);
    let runtime = ToolRuntime::null();
    let executions = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tools: Vec<std::sync::Arc<dyn Tool>> = vec![std::sync::Arc::new(EchoTool {
        executions: std::sync::Arc::clone(&executions),
    })];
    let config = AgentLoopConfig::default();
    let state = LoopState {
        context: &mut context,
        tools: &tools,
        runtime: &runtime,
        client,
        activated: None,
    };
    let result = agent_loop(state, user, &config, vec![], 0, 0, None, None, None, 0)
        .await
        .unwrap();
    (result, context, executions)
}

#[cfg(test)]
async fn run_loop(client: &ScriptedClient, user: &str) -> LoopResult {
    run_loop_full(client, user).await.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::AgentOutcome;
    use crate::routing::{classify_route, RouteMode};
    use crate::task::classify_task;

    #[test]
    fn simple_fast_route() {
        assert_eq!(classify_route("hello"), RouteMode::Fast);
        assert!(classify_task("what time is it").is_simple_voice());
    }

    #[tokio::test]
    async fn simple_fast_loop() {
        let client = ScriptedClient::new(
            "fast",
            vec![json!({"role":"assistant","content":"Hi there."})],
        );
        let result = run_loop(&client, "hello").await;
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Hi there."),
            other => panic!("{other:?}"),
        }
        assert_eq!(result.tool_rounds, 0);
        let calls = client.calls.lock().unwrap();
        assert!(
            calls[0].tools_len > 0,
            "regression requires advertised tools"
        );
        assert_eq!(
            calls[0].options,
            CompleteOptions::for_stage(boris_ai::RequestStage::SimpleVoice)
        );
    }

    #[tokio::test]
    async fn tool_then_speak() {
        let client = ScriptedClient::new(
            "strong",
            vec![
                json!({
                    "role":"assistant",
                    "content":null,
                    "tool_calls":[{
                        "id":"c1",
                        "type":"function",
                        "function":{"name":"echo","arguments":"{\"x\":\"pong\"}"}
                    }]
                }),
                json!({"role":"assistant","content":"Done."}),
            ],
        );
        let result = run_loop(&client, "ping").await;
        assert_eq!(result.tools_used, vec!["echo".to_string()]);
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Done."),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_args_do_not_execute() {
        let client = ScriptedClient::new(
            "strong",
            vec![
                json!({
                    "role":"assistant",
                    "content":null,
                    "tool_calls":[{
                        "id":"c1",
                        "type":"function",
                        "function":{"name":"echo","arguments":"not-json"}
                    }]
                }),
                json!({"role":"assistant","content":"Fixed."}),
            ],
        );
        let (result, context, executions) = run_loop_full(&client, "echo please").await;
        assert_eq!(result.tools_used, vec!["echo".to_string()]);
        assert_eq!(
            executions.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "schema-invalid arguments must never enter Tool::execute"
        );
        let repair = context
            .messages()
            .iter()
            .find(|m| matches!(m.role, Role::Tool))
            .and_then(|m| m.content.get("content"))
            .and_then(Value::as_str)
            .expect("repairable tool observation");
        assert!(repair.starts_with("Error ["), "{repair}");
        assert!(
            repair.contains("object") || repair.contains("arguments"),
            "{repair}"
        );
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Fixed."),
            other => panic!("{other:?}"),
        }
        let calls = client.calls.lock().unwrap();
        assert_eq!(
            calls[0].options.stage,
            Some(boris_ai::RequestStage::SimpleVoice)
        );
        assert_eq!(
            calls[1].options,
            CompleteOptions::for_stage(boris_ai::RequestStage::Complex),
            "a current-turn invalid-args observation must escalate the next round"
        );
    }

    async fn run_confirmation_branch(approved: bool) -> (LoopResult, Context, usize) {
        let final_text = if approved {
            "Approved action completed."
        } else {
            "Okay, I left it unchanged."
        };
        let client = ScriptedClient::new(
            "strong",
            vec![
                json!({
                    "role":"assistant",
                    "content":null,
                    "tool_calls":[{
                        "id":"danger-1",
                        "type":"function",
                        "function":{"name":"dangerous_action","arguments":"{}"}
                    }]
                }),
                json!({"role":"assistant","content":final_text}),
            ],
        );
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "perform the dangerous action");
        let runtime = ToolRuntime::null();
        let executions = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tools: Vec<std::sync::Arc<dyn Tool>> =
            vec![std::sync::Arc::new(ConfirmedSideEffectTool {
                executions: std::sync::Arc::clone(&executions),
            })];
        let config = AgentLoopConfig::default();

        let paused = agent_loop(
            LoopState {
                context: &mut context,
                tools: &tools,
                runtime: &runtime,
                client: &client,
                activated: None,
            },
            "perform the dangerous action",
            &config,
            vec![],
            0,
            0,
            None,
            None,
            None,
            0,
        )
        .await
        .unwrap();
        assert!(matches!(
            paused.outcome,
            AgentOutcome::NeedsConfirmation { .. }
        ));
        assert_eq!(
            executions.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "a paused call must not execute before the user's decision"
        );

        let pending = paused.pending_turn.expect("pending confirmation state");
        let finished = resume_pending_tool(
            LoopState {
                context: &mut context,
                tools: &tools,
                runtime: &runtime,
                client: &client,
                activated: None,
            },
            pending,
            approved,
            &config,
            None,
            None,
        )
        .await
        .unwrap();
        let count = executions.load(std::sync::atomic::Ordering::Relaxed);
        (finished, context, count)
    }

    #[tokio::test]
    async fn hitl_pause_reject_and_approve_have_real_execution_semantics() {
        let (rejected, rejected_context, rejected_count) = run_confirmation_branch(false).await;
        assert_eq!(rejected_count, 0, "reject must never execute the tool");
        assert!(rejected_context.messages().iter().any(|message| {
            matches!(message.role, Role::Tool)
                && message.content["content"] == "Error: user declined this action"
        }));
        match rejected.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Okay, I left it unchanged."),
            other => panic!("{other:?}"),
        }

        let (approved, approved_context, approved_count) = run_confirmation_branch(true).await;
        assert_eq!(approved_count, 1, "approval must execute exactly once");
        assert!(approved_context.messages().iter().any(|message| {
            matches!(message.role, Role::Tool)
                && message.content["content"] == "side effect completed"
        }));
        match approved.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Approved action completed."),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn deep_research_loop_gets_complex_budget() {
        let client = ScriptedClient::new(
            "strong",
            vec![json!({"role":"assistant","content":"Here is the result."})],
        );
        let _ = run_loop(&client, "find Jane Doe on LinkedIn").await;
        let calls = client.calls.lock().unwrap();
        assert_eq!(
            calls[0].options,
            CompleteOptions::for_stage(boris_ai::RequestStage::Complex)
        );
    }

    #[tokio::test]
    async fn failed_searches_do_not_satisfy_research_gate() {
        let failed_calls = (1..=3)
            .map(|i| {
                json!({
                    "id": format!("s{i}"),
                    "type": "function",
                    "function": {
                        "name": "web_search",
                        "arguments": format!(r#"{{"query":"Jane angle {i}"}}"#)
                    }
                })
            })
            .collect::<Vec<_>>();
        let client = ScriptedClient::new(
            "strong",
            vec![
                json!({"role":"assistant","content":null,"tool_calls":failed_calls}),
                json!({"role":"assistant","content":"I couldn't find a verified profile."}),
                json!({"role":"assistant","content":"Still no verified result after retrying."}),
            ],
        );
        let user = "find Jane Doe on LinkedIn";
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, user);
        let runtime = ToolRuntime::null();
        let tools: Vec<std::sync::Arc<dyn Tool>> = vec![std::sync::Arc::new(FailedSearchTool)];
        let config = AgentLoopConfig::default();
        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        let result = agent_loop(state, user, &config, vec![], 0, 0, None, None, None, 1)
            .await
            .unwrap();

        assert_eq!(client.calls.lock().unwrap().len(), 3);
        assert!(context.messages().iter().any(|m| {
            matches!(m.role, Role::User)
                && m.content
                    .as_str()
                    .is_some_and(|s| s.contains("under-tooled"))
        }));
        assert!(context
            .messages()
            .iter()
            .filter(|m| matches!(m.role, Role::Tool))
            .all(|m| m.content["content"]
                .as_str()
                .is_some_and(|s| s.starts_with("Error ["))));
        match result.outcome {
            AgentOutcome::Speak { text, .. } => {
                assert_eq!(text, "Still no verified result after retrying.")
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn note_memory_round_trip_is_visible_to_the_followup_round() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("boris-eval-memory-{unique}"));
        let notes_path = dir.join("notes.jsonl");
        let client = ScriptedClient::new(
            "fast",
            vec![
                json!({
                    "role":"assistant","content":null,"tool_calls":[{
                        "id":"remember","type":"function","function":{
                            "name":"remember_note",
                            "arguments":"{\"note\":\"I prefer jasmine tea\"}"
                        }
                    }]
                }),
                json!({
                    "role":"assistant","content":null,"tool_calls":[{
                        "id":"recall","type":"function","function":{
                            "name":"recall_notes",
                            "arguments":"{\"query\":\"jasmine\"}"
                        }
                    }]
                }),
                json!({"role":"assistant","content":"You prefer jasmine tea."}),
            ],
        );
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "remember and recall my tea preference");
        let runtime = ToolRuntime::null();
        let tools: Vec<std::sync::Arc<dyn Tool>> = vec![
            std::sync::Arc::new(crate::tools::notes::RememberNoteTool::new(&notes_path)),
            std::sync::Arc::new(crate::tools::notes::RecallNotesTool::new(&notes_path)),
        ];
        let config = AgentLoopConfig::default();
        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        let result = agent_loop(
            state,
            "remember and recall my tea preference",
            &config,
            vec![],
            0,
            0,
            None,
            None,
            None,
            0,
        )
        .await
        .unwrap();

        let observations = context
            .messages()
            .iter()
            .filter(|m| matches!(m.role, Role::Tool))
            .filter_map(|m| m.content["content"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(observations.len(), 2);
        assert!(observations[1].contains("jasmine tea"));
        assert!(notes_path.exists());
        assert!(matches!(result.outcome, AgentOutcome::Speak { .. }));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn context_prune_never_leaves_an_orphan_tool_observation() {
        let mut context = Context::new(2);
        context.push(Role::System, "sys");
        context.push(Role::User, "old turn");
        context.push(
            Role::Assistant,
            json!({"tool_calls":[{
                "id":"old-call","type":"function",
                "function":{"name":"echo","arguments":"{}"}
            }]}),
        );
        context.push(
            Role::Tool,
            json!({"tool_call_id":"old-call","content":"old result"}),
        );
        context.push(Role::Assistant, "old done");
        context.push(Role::User, "kept turn");
        context.push(Role::Assistant, "kept answer");
        context.push(Role::User, "current turn");

        let wire = context.as_json();
        let rows = wire.as_array().unwrap();
        assert!(!rows.iter().any(|m| m["role"] == "tool"));
        assert!(!rows.iter().any(|m| {
            m.get("tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|calls| calls.iter().any(|c| c["id"] == "old-call"))
        }));
        assert!(rows.iter().any(|m| m["content"] == "kept turn"));
        assert!(rows.iter().any(|m| m["content"] == "current turn"));
    }

    #[tokio::test]
    async fn empty_script_is_error() {
        let client = ScriptedClient::new("fast", vec![]);
        let mut context = Context::new(20);
        context.push(Role::User, "hi");
        let runtime = ToolRuntime::null();
        let tools: Vec<std::sync::Arc<dyn Tool>> = vec![];
        let config = AgentLoopConfig::default();
        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        assert!(
            agent_loop(state, "hi", &config, vec![], 0, 0, None, None, None, 0)
                .await
                .is_err()
        );
    }

    #[test]
    fn streamed_speech_units_keep_order() {
        let text = "First sentence. Second sentence.";
        let units: Vec<&str> = text.split(". ").collect();
        assert_eq!(units.len(), 2);
        assert!(units[0].starts_with("First"));
        assert!(units[1].starts_with("Second"));
    }
}
