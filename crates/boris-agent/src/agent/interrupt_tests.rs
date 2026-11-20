//! Failed or interrupted turns keep executed tool work (closing unrun calls
//! and appending a host note); turns where nothing executed roll back.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use boris_ai::{LlmClient, LlmError};
use serde_json::{json, Value};

use crate::context::Role;
use crate::tool::{Tool, ToolError, ToolMeta, ToolRisk};
use crate::tool_context::ToolCallContext;
use crate::{AgentOutcome, MessageOrigin};

#[derive(Clone, Default)]
struct Script {
    responses: Arc<Mutex<VecDeque<Result<Value, String>>>>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Script {
    fn new(responses: Vec<Result<Value, String>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            requests: Arc::default(),
        }
    }
}

#[async_trait]
impl LlmClient for Script {
    async fn complete(&self, messages: Value, _tools: Value) -> Result<Value, LlmError> {
        self.requests.lock().unwrap().push(messages);
        match self.responses.lock().unwrap().pop_front() {
            Some(Ok(message)) => Ok(message),
            Some(Err(error)) => Err(LlmError::new(error)),
            None => Err(LlmError::new("script exhausted")),
        }
    }
}

struct Probe {
    name: &'static str,
    confirm: bool,
}

#[async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{}})
    }
    fn meta(&self) -> ToolMeta {
        if self.confirm {
            ToolMeta::with_risk(ToolRisk::Dangerous)
                .confirm(true)
                .read_only(false)
        } else {
            ToolMeta::safe_default().read_only(true)
        }
    }
    async fn execute(&self, _: &ToolCallContext, _: Value) -> Result<String, ToolError> {
        Ok(format!("{} done", self.name))
    }
}

fn calls(names: &[(&str, &str)]) -> Value {
    json!({"role":"assistant","content":"","tool_calls": names.iter().map(|(id, name)| json!({
        "id": id, "type": "function", "function": {"name": name, "arguments": "{}"}
    })).collect::<Vec<_>>()})
}

fn agent_with(script: &Script) -> super::Agent {
    let mut agent = super::Agent::new(Box::new(script.clone()), "sys");
    agent.register_tool(Box::new(Probe {
        name: "note_save",
        confirm: false,
    }));
    agent.register_tool(Box::new(Probe {
        name: "shell_run",
        confirm: true,
    }));
    agent
}

fn persisted_texts(agent: &super::Agent) -> Vec<String> {
    agent
        .export_messages_for_persist()
        .iter()
        .map(|m| match &m.content {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .collect()
}

/// Every assistant tool call in a provider request has a matching tool row.
fn assert_no_orphan_calls(request: &Value) {
    let messages = request.as_array().unwrap();
    for (index, message) in messages.iter().enumerate() {
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            let id = call["id"].as_str().unwrap();
            assert!(
                messages[index + 1..]
                    .iter()
                    .any(|m| m["role"] == "tool" && m["tool_call_id"] == id),
                "orphan tool call {id}"
            );
        }
    }
}

#[tokio::test]
async fn failed_turn_keeps_executed_tools_with_a_host_note() {
    let script = Script::new(vec![
        Ok(calls(&[("c1", "note_save")])),
        Err("provider stalled".into()),
        Ok(json!({"role":"assistant","content":"Already saved."})),
    ]);
    let mut agent = agent_with(&script);
    assert!(agent.prompt("save my note").await.is_err());

    let texts = persisted_texts(&agent);
    assert!(texts.iter().any(|t| t == "save my note"));
    assert!(texts.iter().any(|t| t.contains("note_save done")));
    let note = texts.last().unwrap();
    assert!(note.contains("failed after these tools ran: note_save"), "{note}");

    // The next turn sees the executed work and a valid provider sequence.
    agent.prompt("continue").await.unwrap();
    let requests = script.requests.lock().unwrap();
    let last = requests.last().unwrap();
    assert_no_orphan_calls(last);
    let wire = last.to_string();
    assert!(wire.contains("note_save done") && wire.contains("failed after these tools ran"));
}

#[tokio::test]
async fn abandon_keeps_executed_tools_and_drops_the_undelivered_reply() {
    let script = Script::new(vec![
        Ok(calls(&[("c1", "note_save")])),
        Ok(json!({"role":"assistant","content":"Saved your note."})),
    ]);
    let mut agent = agent_with(&script);
    let checkpoint = agent.checkpoint();
    agent.prompt("save my note").await.unwrap();

    agent.abandon_turn(checkpoint);

    let texts = persisted_texts(&agent);
    assert!(texts.iter().any(|t| t == "save my note"));
    assert!(texts.iter().any(|t| t.contains("note_save done")));
    assert!(!texts.iter().any(|t| t == "Saved your note."));
    assert!(texts
        .last()
        .unwrap()
        .contains("interrupted this turn after these tools ran: note_save"));
    assert!(!agent
        .context
        .messages()
        .iter()
        .any(|m| m.origin == MessageOrigin::HostControl));
}

#[tokio::test]
async fn abandon_without_executed_tools_restores_the_checkpoint() {
    let script = Script::new(vec![Ok(json!({"role":"assistant","content":"Hello."}))]);
    let mut agent = agent_with(&script);
    let checkpoint = agent.checkpoint();
    agent.prompt("discarded objective").await.unwrap();
    agent.abandon_turn(checkpoint);
    assert!(!persisted_texts(&agent)
        .iter()
        .any(|t| t == "discarded objective"));
}

#[tokio::test]
async fn abandon_during_confirmation_keeps_ran_calls_and_closes_the_paused_one() {
    let script = Script::new(vec![Ok(calls(&[
        ("c1", "note_save"),
        ("c2", "shell_run"),
    ]))]);
    let mut agent = agent_with(&script);
    let checkpoint = agent.checkpoint();
    let outcome = agent.prompt("save then run").await.unwrap();
    assert!(matches!(outcome, AgentOutcome::NeedsConfirmation { .. }));
    assert!(agent.has_pending_confirmation());

    agent.abandon_turn(checkpoint);

    assert!(!agent.has_pending_confirmation());
    let persisted = agent.export_messages_for_persist();
    let rows: Vec<(&str, &str, Option<bool>)> = persisted
        .iter()
        .filter(|m| matches!(m.role, Role::Tool))
        .map(|m| {
            (
                m.content["tool_call_id"].as_str().unwrap(),
                m.content["content"].as_str().unwrap(),
                m.content["_tool_ok"].as_bool(),
            )
        })
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ("c1", "note_save done", Some(true)));
    assert_eq!(rows[1].0, "c2");
    assert_eq!(rows[1].2, Some(false), "the paused call never ran");
    let note = persisted_texts(&agent).last().unwrap().clone();
    assert!(note.contains("tools ran: note_save") && !note.contains("shell_run"));
}

#[tokio::test]
async fn abandon_after_a_declined_approval_restores_the_checkpoint() {
    let script = Script::new(vec![
        Ok(calls(&[("c1", "shell_run")])),
        Ok(json!({"role":"assistant","content":"Okay, I won't run it."})),
    ]);
    let mut agent = agent_with(&script);
    let checkpoint = agent.checkpoint();
    let outcome = agent.prompt("run the cleanup").await.unwrap();
    let AgentOutcome::NeedsConfirmation { pending, .. } = outcome else {
        panic!("expected a confirmation");
    };
    agent.resume_confirmation(&pending.id, false).await.unwrap();

    agent.abandon_turn(checkpoint);

    let texts = persisted_texts(&agent);
    assert!(!texts.iter().any(|t| t == "run the cleanup"), "{texts:?}");
    assert!(!texts.iter().any(|t| t.contains("tools ran")), "{texts:?}");
}
