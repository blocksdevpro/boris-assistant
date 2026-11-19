//! Offline harness eval: scripted scenarios against a mock `LlmClient`.
//!
//! These assert harness behaviour (tool listing, reasoning round-trip,
//! interruption bookkeeping, follow-ups, precheck-before-HITL, round budget,
//! reminders, hooks, streamed speech, silence), never model quality. No
//! network, audio, or provider is involved:
//! `cargo test -p boris-agent --test harness_eval`
//!
//! `live_eval_tool_list_is_stable` is an opt-in variant against a real
//! OpenRouter model. It is `#[ignore]`d and also needs
//! `BORIS_LIVE_EVAL=1` and `OPENROUTER_API_KEY`:
//! `BORIS_LIVE_EVAL=1 cargo test -p boris-agent --test harness_eval -- --ignored`

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use boris_agent::{
    register_builtin_tools_with_preset, Agent, AgentEvent, AgentOutcome, BuiltinToolPaths,
    CapabilityPreset, HookEvent, HookSpec, Hooks, LlmClient, LlmError, ReminderStore,
    SandboxConfig, SpeechSignal, TaskStatus, TurnCancel,
};
use serde_json::{json, Value};

// ── Mock provider ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Call {
    messages: Value,
    tools: Value,
}

impl Call {
    fn has_tools(&self) -> bool {
        self.tools.as_array().is_some_and(|t| !t.is_empty())
    }

    fn tool_names(&self) -> Vec<String> {
        self.tools
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn text(&self) -> String {
        self.messages.to_string()
    }
}

type Script = dyn Fn(usize, &Call) -> Result<Value, LlmError> + Send + Sync;

/// Replies come from a per-scenario closure; every request is recorded.
struct MockLlm {
    script: Box<Script>,
    calls: Arc<Mutex<Vec<Call>>>,
    /// Cancel this turn when the given call index arrives (then stall).
    cancel_at: Option<(usize, Arc<Mutex<Option<TurnCancel>>>)>,
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        let call = Call { messages, tools };
        let index = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(call.clone());
            calls.len() - 1
        };
        if let Some((at, handle)) = &self.cancel_at {
            if *at == index {
                if let Some(cancel) = handle.lock().unwrap().as_ref() {
                    cancel.cancel();
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
        (self.script)(index, &call)
    }

    fn model(&self) -> &str {
        "mock/eval"
    }
}

fn mock(
    script: impl Fn(usize, &Call) -> Result<Value, LlmError> + Send + Sync + 'static,
) -> (MockLlm, Arc<Mutex<Vec<Call>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    (
        MockLlm {
            script: Box::new(script),
            calls: calls.clone(),
            cancel_at: None,
        },
        calls,
    )
}

fn say(text: &str) -> Result<Value, LlmError> {
    Ok(json!({"role": "assistant", "content": text}))
}

fn tool_call(id: &str, name: &str, args: Value) -> Value {
    json!({
        "id": id,
        "type": "function",
        "function": {"name": name, "arguments": args.to_string()}
    })
}

fn call_tools(calls: Vec<Value>) -> Result<Value, LlmError> {
    Ok(json!({"role": "assistant", "content": null, "tool_calls": calls}))
}

// ── Agent fixture ────────────────────────────────────────────────────────────

struct TempHome(PathBuf);

impl TempHome {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("boris-eval-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(dir.join("sandbox")).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Real builtin registration (Full preset), no trusted auto-allow, so file
/// writes need a yes/no exactly as on a fresh desktop install.
fn agent_with_builtins(client: MockLlm, home: &Path) -> Agent {
    let mut agent = Agent::new(
        Box::new(client),
        "You are Boris, a concise voice assistant.",
    );
    let mut sandbox = SandboxConfig::for_desktop_mvp(home);
    let paths = BuiltinToolPaths {
        notes_path: home.join("memory/notes.jsonl"),
        profile_path: home.join("memory/profile.json"),
        sandbox_root: home.join("sandbox"),
        data_roots: vec![home.join("memory")],
        allow_read: vec![home.to_path_buf()],
        allow_write: vec![home.join("sandbox")],
        boris_home: home.to_path_buf(),
    };
    register_builtin_tools_with_preset(
        &mut agent,
        paths,
        false,
        true,
        &mut sandbox,
        CapabilityPreset::Full,
    );
    agent.configure_runtime(sandbox, None);
    agent
}

fn tool_results(agent: &Agent) -> Vec<String> {
    agent
        .export_messages()
        .iter()
        .filter(|m| matches!(m.role, boris_agent::Role::Tool))
        .map(|m| m.content.to_string())
        .collect()
}

fn spoken(outcome: &AgentOutcome) -> &str {
    match outcome {
        AgentOutcome::Speak { text, .. } => text,
        other => panic!("expected speech, got {other:?}"),
    }
}

// ── Scenarios ────────────────────────────────────────────────────────────────

/// 1. Every request in a multi-round turn carries the identical tool table.
#[tokio::test]
async fn tool_list_is_stable_across_rounds() {
    let home = TempHome::new("tools");
    let (client, calls) = mock(|i, _| match i {
        0 => call_tools(vec![tool_call("c1", "todo_read", json!({}))]),
        1 => call_tools(vec![tool_call("c2", "reminders_list", json!({}))]),
        _ => say("Nothing is pending."),
    });
    let mut agent = agent_with_builtins(client, home.path());
    let outcome = agent.prompt("what's on my plate").await.unwrap();
    assert_eq!(spoken(&outcome), "Nothing is pending.");

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    let first = calls[0].tools.clone();
    assert!(calls[0].has_tools());
    for call in calls.iter() {
        assert_eq!(call.tools, first, "tool table changed between rounds");
    }
    let names = calls[0].tool_names();
    for expected in [
        "todo_read",
        "file_edit",
        "bash",
        "reminder_set",
        "timer_set",
    ] {
        assert!(names.iter().any(|n| n == expected), "{expected} not listed");
    }
}

/// 2. Provider reasoning_details come back unmodified on the next round.
#[tokio::test]
async fn reasoning_details_survive_tool_rounds() {
    let home = TempHome::new("reasoning");
    let details = json!([{"type": "reasoning.encrypted", "data": "opaque-123", "index": 0}]);
    let sent = details.clone();
    let (client, calls) = mock(move |i, _| match i {
        0 => Ok(json!({
            "role": "assistant",
            "content": null,
            "reasoning_details": sent,
            "tool_calls": [tool_call("c1", "todo_read", json!({}))]
        })),
        _ => say("Done."),
    });
    let mut agent = agent_with_builtins(client, home.path());
    agent.prompt("check my todos").await.unwrap();

    let calls = calls.lock().unwrap();
    let assistant = calls[1]
        .messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant" && m.get("tool_calls").is_some());
    assert_eq!(
        assistant.expect("tool-call message resent")["reasoning_details"],
        details
    );
}

/// 3. A turn interrupted after a tool ran keeps that work and a host note.
#[tokio::test]
async fn interrupted_turn_keeps_executed_work() {
    let home = TempHome::new("interrupt");
    let handle: Arc<Mutex<Option<TurnCancel>>> = Arc::new(Mutex::new(None));
    let (mut client, _calls) = mock(|i, _| match i {
        0 => call_tools(vec![tool_call(
            "c1",
            "reminder_set",
            json!({"when": "in 2 hours", "message": "water the plants"}),
        )]),
        _ => say("unreachable"),
    });
    client.cancel_at = Some((1, handle.clone()));
    let mut agent = agent_with_builtins(client, home.path());
    *handle.lock().unwrap() = Some(agent.arm_cancel());

    let result = agent.prompt("remind me to water the plants later").await;
    assert!(result.is_err(), "the turn was cancelled");

    // The side effect is real and stays.
    let store = ReminderStore::for_home(home.path());
    assert!(store.list().iter().any(|r| r.message == "water the plants"));
    // So does its record in context, plus a host note naming the tool.
    let results = tool_results(&agent);
    assert!(
        results.iter().any(|r| r.contains("Reminder set")),
        "{results:?}"
    );
    let history: Vec<String> = agent
        .export_messages()
        .iter()
        .map(|m| m.content.to_string())
        .collect();
    assert!(
        history
            .iter()
            .any(|m| m.contains("[Host note:") && m.contains("reminder_set")),
        "{history:?}"
    );
}

/// 4. "continue" restates the earlier objective verbatim to the model.
#[tokio::test]
async fn continue_restates_the_task() {
    let home = TempHome::new("continue");
    let (client, calls) = mock(|i, _| match i {
        0 => say("I drafted the first two sections. Say continue for the rest."),
        _ => say("Here are the remaining sections."),
    });
    let mut agent = agent_with_builtins(client, home.path());
    agent
        .prompt("Draft an outline for the quarterly planning memo")
        .await
        .unwrap();
    agent.prompt("continue").await.unwrap();

    let calls = calls.lock().unwrap();
    let second = calls.last().unwrap().text();
    assert!(
        second.contains(
            "Earlier objective (verbatim): \\\"Draft an outline for the quarterly planning memo\\\""
        ),
        "{second}"
    );
}

/// 5. An edit of a file never read is refused before any yes/no.
#[tokio::test]
async fn unread_edit_is_refused_before_hitl() {
    let home = TempHome::new("edit");
    let file = home.path().join("sandbox").join("notes.md");
    std::fs::write(&file, "hello world\n").unwrap();
    let path = file.to_string_lossy().to_string();
    let (client, _calls) = mock(move |i, _| match i {
        0 => call_tools(vec![tool_call(
            "c1",
            "file_edit",
            json!({"path": path, "old_string": "hello", "new_string": "goodbye"}),
        )]),
        _ => say("I need to read it first."),
    });
    let mut agent = agent_with_builtins(client, home.path());
    let outcome = agent
        .prompt("change hello to goodbye in notes.md")
        .await
        .unwrap();

    assert!(
        !matches!(outcome, AgentOutcome::NeedsConfirmation { .. }),
        "the user must not be asked to approve a doomed edit"
    );
    assert_eq!(spoken(&outcome), "I need to read it first.");
    let results = tool_results(&agent);
    assert!(
        results.iter().any(|r| r.contains("file_read")),
        "{results:?}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello world\n");
}

/// 6. Hitting the round budget withholds tools and asks for a check-in, and
/// the task stays open for "continue".
#[tokio::test]
async fn round_budget_leads_to_a_check_in() {
    let home = TempHome::new("budget");
    let (client, calls) = mock(|i, call| {
        if call.has_tools() {
            call_tools(vec![tool_call(&format!("c{i}"), "todo_read", json!({}))])
        } else {
            say("I checked twice and I'm not done; say continue to keep going.")
        }
    });
    let mut agent = agent_with_builtins(client, home.path());
    agent.set_max_tool_rounds(2);
    let outcome = agent.prompt("check my todos").await.unwrap();

    assert!(spoken(&outcome).contains("say continue"));
    let calls = calls.lock().unwrap();
    let last = calls.last().unwrap();
    assert!(!last.has_tools(), "tools are withheld at the budget");
    assert!(last.text().contains("work budget is used up"));
    assert_eq!(agent.task_state().status, TaskStatus::Active);
}

/// 7. A reminder the model sets is persisted and fires when due.
#[tokio::test]
async fn reminder_fires_when_due() {
    let home = TempHome::new("reminder");
    let (client, _calls) = mock(|i, _| match i {
        0 => call_tools(vec![tool_call(
            "c1",
            "timer_set",
            json!({"duration": "1s", "message": "tea"}),
        )]),
        _ => say("Timer's on."),
    });
    let mut agent = agent_with_builtins(client, home.path());
    agent.prompt("one second tea timer").await.unwrap();

    let store = ReminderStore::for_home(home.path());
    let now = boris_agent::now_unix_ms();
    assert!(!store.has_due(now - 1_000));
    let due = store.take_due(now + 2_000);
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].spoken(now + 2_000), "Your timer is done: tea.");
    assert!(store.take_due(now + 2_000).is_empty(), "fires once");
    // A restart sees the delivered state.
    let file = std::fs::read_to_string(ReminderStore::path_for_home(home.path())).unwrap();
    assert!(!file.contains("tea"), "{file}");
}

/// 8. A failing pre_tool hook blocks the call; the model sees a failed result.
#[tokio::test]
async fn pre_tool_hook_blocks_a_call() {
    let home = TempHome::new("hook");
    let (client, _calls) = mock(|i, _| match i {
        0 => call_tools(vec![tool_call(
            "c1",
            "timer_set",
            json!({"duration": "5m"}),
        )]),
        _ => say("A hook blocked that."),
    });
    let mut agent = agent_with_builtins(client, home.path());
    agent.set_hooks(Hooks::new(vec![HookSpec {
        event: HookEvent::PreTool,
        tool: Some("timer_*".into()),
        command: "exit 7".into(),
        timeout_secs: 10,
    }]));
    let outcome = agent.prompt("five minute timer").await.unwrap();

    assert_eq!(spoken(&outcome), "A hook blocked that.");
    let results = tool_results(&agent);
    assert!(
        results.iter().any(|r| r.contains("hook_blocked")),
        "{results:?}"
    );
    assert!(
        ReminderStore::for_home(home.path()).list().is_empty(),
        "the blocked tool never ran"
    );
}

/// 9. With streamed speech on, a note sent beside tool calls is never
/// committed; the final answer streams and commits.
#[tokio::test]
async fn tool_note_is_never_spoken() {
    let home = TempHome::new("speech");
    let (client, _calls) = mock(|i, _| match i {
        0 => Ok(json!({
            "role": "assistant",
            "content": "Let me look at your todo list first.",
            "tool_calls": [tool_call("c1", "todo_read", json!({}))]
        })),
        _ => say("Your todo list is empty."),
    });
    let mut agent = agent_with_builtins(client, home.path());
    agent.set_stream_speech(true);
    let signals = Arc::new(Mutex::new(Vec::new()));
    let sink = signals.clone();
    let _unsub = agent.subscribe(move |event| {
        if let AgentEvent::Speech { signal } = event {
            sink.lock().unwrap().push(signal.clone());
        }
    });
    agent.prompt("anything on my list").await.unwrap();

    let signals = signals.lock().unwrap();
    assert_eq!(
        *signals,
        vec![
            SpeechSignal::Chunk("Your todo list is empty.".into()),
            SpeechSignal::Commit,
        ]
    );
}

/// 10. `[silent]` with no tool work stays silent.
#[tokio::test]
async fn silent_reply_stays_silent() {
    let home = TempHome::new("silent");
    let (client, _calls) = mock(|_, _| say("[silent]"));
    let mut agent = agent_with_builtins(client, home.path());
    let outcome = agent
        .prompt("and then she said the movie was fine")
        .await
        .unwrap();
    assert!(matches!(outcome, AgentOutcome::Silent), "{outcome:?}");
}

// ── Opt-in live variant ──────────────────────────────────────────────────────

/// Records every request of a real client.
struct Recording {
    inner: boris_agent::OpenRouterClient,
    calls: Arc<Mutex<Vec<Call>>>,
}

#[async_trait]
impl LlmClient for Recording {
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        self.calls.lock().unwrap().push(Call {
            messages: messages.clone(),
            tools: tools.clone(),
        });
        self.inner.complete(messages, tools).await
    }

    async fn complete_with_options(
        &self,
        messages: Value,
        tools: Value,
        options: boris_agent::CompleteOptions,
    ) -> Result<Value, LlmError> {
        self.calls.lock().unwrap().push(Call {
            messages: messages.clone(),
            tools: tools.clone(),
        });
        self.inner
            .complete_with_options(messages, tools, options)
            .await
    }

    fn model(&self) -> &str {
        self.inner.model()
    }
}

/// Live: a real model's multi-round turn still sees one stable tool table.
/// Off by default; never part of routine checks.
#[tokio::test]
#[ignore = "live: calls a real OpenRouter model; needs BORIS_LIVE_EVAL=1 and OPENROUTER_API_KEY"]
async fn live_eval_tool_list_is_stable() {
    if std::env::var("BORIS_LIVE_EVAL").ok().as_deref() != Some("1") {
        eprintln!("skipping: set BORIS_LIVE_EVAL=1 to run the live eval");
        return;
    }
    let Ok(key) = std::env::var("OPENROUTER_API_KEY") else {
        eprintln!("skipping: OPENROUTER_API_KEY is not set");
        return;
    };
    let home = TempHome::new("live");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let client = Recording {
        inner: boris_agent::OpenRouterClient::new(key, std::env::var("OPENROUTER_MODEL").ok()),
        calls: calls.clone(),
    };
    let mut agent = Agent::new(
        Box::new(client),
        "You are Boris, a concise voice assistant.",
    );
    let mut sandbox = SandboxConfig::for_desktop_mvp(home.path());
    register_builtin_tools_with_preset(
        &mut agent,
        BuiltinToolPaths {
            notes_path: home.path().join("memory/notes.jsonl"),
            profile_path: home.path().join("memory/profile.json"),
            sandbox_root: home.path().join("sandbox"),
            data_roots: vec![],
            allow_read: vec![home.path().to_path_buf()],
            allow_write: vec![home.path().join("sandbox")],
            boris_home: home.path().to_path_buf(),
        },
        false,
        false,
        &mut sandbox,
        CapabilityPreset::VoiceSafe,
    );
    agent.configure_runtime(sandbox, None);
    let outcome = agent
        .prompt("List my reminders, then set a 10 minute timer called tea.")
        .await
        .expect("live turn");
    eprintln!("live outcome: {outcome:?}");
    let calls = calls.lock().unwrap();
    let with_tools: Vec<&Call> = calls.iter().filter(|c| c.has_tools()).collect();
    assert!(!with_tools.is_empty());
    for call in &with_tools {
        assert_eq!(call.tools, with_tools[0].tools, "tool table changed");
    }
}
