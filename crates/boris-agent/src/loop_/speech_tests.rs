//! Streamed speech: content beside a tool call is never committed, a tool
//! call that arrives after the first sentence was released cancels it, and
//! the final answer streams and commits.

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use boris_ai::{CompleteOptions, LlmClient, LlmError, LlmStreamEvent};
use serde_json::{json, Value};

use super::*;
use crate::speech_stream::{SpeechSignal, DEFAULT_RELEASE_GRACE};
use crate::tool::{ToolError, ToolMeta};

struct Ping;

#[async_trait]
impl Tool for Ping {
    fn name(&self) -> &str {
        "ping"
    }
    fn description(&self) -> &str {
        "ping"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{}})
    }
    fn meta(&self) -> ToolMeta {
        ToolMeta::safe_default().read_only(true)
    }
    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        _args: Value,
    ) -> Result<String, ToolError> {
        Ok("pong".into())
    }
}

/// Round 1 streams a full sentence, pauses past the release grace, then
/// starts a tool call. Round 2 streams the answer.
struct LateToolClient {
    round: Mutex<u32>,
}

#[async_trait]
impl LlmClient for LateToolClient {
    async fn complete(&self, _: Value, _: Value) -> Result<Value, LlmError> {
        Err(LlmError::new("streaming only"))
    }

    async fn complete_stream(
        &self,
        _messages: Value,
        _tools: Value,
        _opts: CompleteOptions,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<Value, LlmError> {
        let round = {
            let mut guard = self.round.lock().unwrap();
            *guard += 1;
            *guard
        };
        if round == 1 {
            let note = "Let me check that for you. ";
            on_event(LlmStreamEvent::ContentDelta { text: note.into() });
            std::thread::sleep(DEFAULT_RELEASE_GRACE + Duration::from_millis(50));
            on_event(LlmStreamEvent::ContentDelta { text: "One".into() });
            on_event(LlmStreamEvent::ToolCallDelta {
                index: 0,
                id: Some("c1".into()),
                name: Some("ping".into()),
                arguments_delta: "{}".into(),
            });
            return Ok(json!({
                "role": "assistant",
                "content": "Let me check that for you. One",
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": {"name": "ping", "arguments": "{}"}
                }]
            }));
        }
        on_event(LlmStreamEvent::ContentDelta {
            text: "It answered pong. ".into(),
        });
        on_event(LlmStreamEvent::ContentDelta {
            text: "All good.".into(),
        });
        Ok(json!({"role": "assistant", "content": "It answered pong. All good."}))
    }

    fn model(&self) -> &str {
        "test"
    }
}

#[tokio::test]
async fn late_tool_call_cancels_streamed_note_and_final_answer_commits() {
    let client = LateToolClient {
        round: Mutex::new(0),
    };
    let mut context = Context::new(20);
    context.push(Role::System, "sys");
    context.push(Role::User, "ping it");
    let runtime = ToolRuntime::null();
    let tools: Vec<std::sync::Arc<dyn Tool>> = vec![std::sync::Arc::new(Ping)];
    let config = AgentLoopConfig {
        stream_speech: true,
        ..Default::default()
    };
    let signals = std::sync::Arc::new(Mutex::new(Vec::new()));
    let captured = signals.clone();
    let emit: crate::types::EmitFn = std::sync::Arc::new(move |event| {
        if let AgentEvent::Speech { signal } = event {
            captured.lock().unwrap().push(signal);
        }
    });
    let state = LoopState {
        context: &mut context,
        tools: &tools,
        runtime: &runtime,
        client: &client,
        activated: None,
    };
    let result = agent_loop(
        state,
        "ping it",
        &config,
        vec![],
        0,
        0,
        None,
        Some(emit),
        None,
        0,
    )
    .await
    .unwrap();

    match result.outcome {
        AgentOutcome::Speak { text, .. } => assert_eq!(text, "It answered pong. All good."),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        *signals.lock().unwrap(),
        vec![
            SpeechSignal::Chunk("Let me check that for you.".into()),
            SpeechSignal::Cancel,
            SpeechSignal::Chunk("It answered pong. All good.".into()),
            SpeechSignal::Commit,
        ]
    );
}

#[tokio::test]
async fn speech_stays_off_unless_the_host_opts_in() {
    let client = LateToolClient {
        round: Mutex::new(1),
    };
    let mut context = Context::new(20);
    context.push(Role::System, "sys");
    context.push(Role::User, "hi");
    let runtime = ToolRuntime::null();
    let tools: Vec<std::sync::Arc<dyn Tool>> = vec![];
    let seen = std::sync::Arc::new(Mutex::new(0usize));
    let counter = seen.clone();
    let emit: crate::types::EmitFn = std::sync::Arc::new(move |event| {
        if matches!(event, AgentEvent::Speech { .. }) {
            *counter.lock().unwrap() += 1;
        }
    });
    let state = LoopState {
        context: &mut context,
        tools: &tools,
        runtime: &runtime,
        client: &client,
        activated: None,
    };
    agent_loop(
        state,
        "hi",
        &AgentLoopConfig::default(),
        vec![],
        0,
        0,
        None,
        Some(emit),
        None,
        0,
    )
    .await
    .unwrap();
    assert_eq!(*seen.lock().unwrap(), 0);
}
