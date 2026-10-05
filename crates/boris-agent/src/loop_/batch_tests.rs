//! Deterministic concurrency and protocol regressions; no live environment.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::tool_batch::{process_tool_calls, ToolBatchResult};
use super::{agent_loop, resume_pending_input, resume_pending_tool, LoopState};
use crate::context::{Context, Role};
use crate::eval::ScriptedClient;
use crate::runtime::{RawToolCall, ToolRuntime};
use crate::tool::{Permission, Tool, ToolError, ToolMeta, ToolRisk};
use crate::types::{AgentEvent, AgentLoopConfig, EmitFn};

struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct PoolRead {
    active: Arc<AtomicUsize>,
    peak: AtomicUsize,
    release: Notify,
    started: Notify,
    rolling: bool,
    block: bool,
    tool_limit: u32,
}

impl PoolRead {
    fn new(tool_limit: u32, rolling: bool, block: bool) -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            peak: AtomicUsize::new(0),
            release: Notify::new(),
            started: Notify::new(),
            rolling,
            block,
            tool_limit,
        }
    }
}

#[async_trait]
impl Tool for PoolRead {
    fn name(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        "read a numbered value"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object", "properties":{"n":{"type":"integer"}}, "required":["n"]})
    }
    fn meta(&self) -> ToolMeta {
        ToolMeta::safe_default()
            .read_only(true)
            .max_concurrency(self.tool_limit)
    }
    async fn execute(
        &self,
        _: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        let _active = Active(self.active.clone());
        self.peak.fetch_max(active, Ordering::SeqCst);
        self.started.notify_one();
        let n = args["n"].as_u64().unwrap();
        if self.block || (self.rolling && n == 0) {
            self.release.notified().await;
        } else {
            tokio::task::yield_now().await;
            if self.rolling && n == 2 {
                // A fixed chunk would wait forever for call 0 and never start 2.
                self.release.notify_one();
            }
        }
        Ok(n.to_string())
    }
}

struct StateTool {
    name: &'static str,
    read_only: bool,
    confirm: bool,
    permissions: &'static [Permission],
    value: Arc<AtomicUsize>,
    executions: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for StateTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "observe or increment test state"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object", "properties":{"path":{"type":"string"}, "url":{"type":"string"}, "command":{"type":"string"}}})
    }
    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(if self.confirm {
            ToolRisk::Dangerous
        } else {
            ToolRisk::Safe
        })
        .read_only(self.read_only)
        .confirm(self.confirm)
        .permissions(self.permissions)
    }
    async fn execute(
        &self,
        _: &crate::tool_context::ToolCallContext,
        _: Value,
    ) -> Result<String, ToolError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        if !self.read_only {
            self.value.fetch_add(1, Ordering::SeqCst);
        }
        Ok(self.value.load(Ordering::SeqCst).to_string())
    }
}

fn call(id: &str, name: &str, args: Value) -> RawToolCall {
    RawToolCall {
        call_id: id.into(),
        name: name.into(),
        args,
    }
}

fn response(calls: &[RawToolCall], explicit: bool) -> Value {
    let leaves: Vec<_> = calls.iter().map(|call| json!({
        "id":call.call_id, "type":"function", "function":{"name":call.name, "arguments":call.args.to_string()}
    })).collect();
    if explicit {
        let uses: Vec<_> = calls
            .iter()
            .map(|call| json!({"recipient_name":call.name, "parameters":call.args}))
            .collect();
        json!({"role":"assistant", "content":"Checking the requested values.", "tool_calls":[{
            "id":"outer", "type":"function", "function":{"name":"parallel", "arguments":json!({"tool_uses":uses}).to_string()}
        }]})
    } else {
        json!({"role":"assistant", "content":"Checking the requested values.", "tool_calls":leaves})
    }
}

fn tool_results(context: &Context) -> Vec<&Value> {
    context
        .messages()
        .iter()
        .filter(|m| matches!(m.role, Role::Tool))
        .map(|m| &m.content)
        .collect()
}

fn assert_paired(context: &Context) {
    let calls: Vec<_> = context
        .messages()
        .iter()
        .filter(|m| matches!(m.role, Role::Assistant))
        .filter_map(|m| m.content["tool_calls"].as_array())
        .flatten()
        .collect();
    let results = tool_results(context);
    assert_eq!(calls.len(), results.len());
    for call in calls {
        assert_eq!(
            results
                .iter()
                .filter(|result| result["tool_call_id"] == call["id"])
                .count(),
            1
        );
        assert_ne!(call["function"]["name"], "parallel");
    }
}

async fn run_batch(
    tools: &[Arc<dyn Tool>],
    calls: Vec<RawToolCall>,
    config: &AgentLoopConfig,
    runtime: &ToolRuntime,
    cancel: Option<CancellationToken>,
    emit: &EmitFn,
) -> Result<(Context, Vec<String>, ToolBatchResult), crate::AgentError> {
    let client = ScriptedClient::new("test", vec![]);
    let mut context = Context::new(20);
    context.push(Role::System, "sys");
    context.push(Role::User, "check values");
    context.push(Role::Assistant, response(&calls, false));
    let mut used = Vec::new();
    let mut confirms = 0;
    let result = process_tool_calls(
        &mut LoopState {
            context: &mut context,
            tools,
            runtime,
            client: &client,
            activated: None,
        },
        calls,
        &mut used,
        1,
        &mut confirms,
        "check values",
        config,
        emit,
        cancel,
    )
    .await?;
    Ok((context, used, result))
}

#[tokio::test]
async fn namespaced_calls_execute_and_leave_valid_paired_history() {
    for explicit in [false, true] {
        for waves in [false, true] {
            let executions = Arc::new(AtomicUsize::new(0));
            let tools: Vec<Arc<dyn Tool>> = vec![
                Arc::new(crate::tools::parallel::ParallelTool),
                Arc::new(StateTool {
                    name: "web_search",
                    read_only: true,
                    confirm: false,
                    permissions: &[Permission::None],
                    value: Arc::new(AtomicUsize::new(0)),
                    executions: executions.clone(),
                }),
                Arc::new(StateTool {
                    name: "web_fetch",
                    read_only: true,
                    confirm: false,
                    permissions: &[Permission::None],
                    value: Arc::new(AtomicUsize::new(0)),
                    executions: executions.clone(),
                }),
            ];
            let calls = vec![
                call("search", "functions.web_search", json!({})),
                call("fetch", "functions.web_fetch", json!({})),
                call("unknown", "functions.absent", json!({})),
            ];
            let client = ScriptedClient::new(
                "test",
                vec![
                    response(&calls, explicit),
                    json!({"role":"assistant", "content":"The requested checks finished."}),
                ],
            );
            let mut context = Context::new(20);
            context.push(Role::System, "sys");
            context.push(Role::User, "check values");
            let runtime = ToolRuntime::null();
            let config = AgentLoopConfig {
                features: crate::ToolRuntimeFeatures {
                    wave_scheduling: waves,
                    ..Default::default()
                },
                ..Default::default()
            };
            let result = agent_loop(
                LoopState {
                    context: &mut context,
                    tools: &tools,
                    runtime: &runtime,
                    client: &client,
                    activated: None,
                },
                "check values",
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
            assert_eq!(executions.load(Ordering::SeqCst), 2);
            assert_eq!(result.tools_used, ["web_search", "web_fetch", "absent"]);
            assert_eq!(client.calls.lock().unwrap().len(), 2);
            assert_paired(&context);
            let wire = context.as_json();
            for message in wire.as_array().unwrap() {
                if let Some(calls) = message["tool_calls"].as_array() {
                    for call in calls {
                        assert!(boris_ai::is_valid_tool_name(
                            call["function"]["name"].as_str().unwrap()
                        ));
                    }
                }
            }
            assert!(tool_results(&context)[2]["content"]
                .as_str()
                .unwrap()
                .contains("absent"));
        }
    }
}

#[tokio::test]
async fn rolling_pool_refills_before_slowest_call_and_keeps_result_order() {
    let read = Arc::new(PoolRead::new(8, true, false));
    let tools: Vec<Arc<dyn Tool>> = vec![read.clone()];
    let mut config = AgentLoopConfig::default();
    config.features.max_parallel_tools = 2;
    let calls = (0..6)
        .map(|n| call(&format!("r{n}"), "read", json!({"n":n})))
        .collect();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let emit: EmitFn = Arc::new(move |event| captured.lock().unwrap().push(event));
    let runtime = ToolRuntime::null();
    let (context, used, result) = tokio::time::timeout(
        Duration::from_secs(2),
        run_batch(&tools, calls, &config, &runtime, None, &emit),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(result, ToolBatchResult::Continue));
    assert_eq!(used.len(), 6);
    assert_eq!(read.peak.load(Ordering::SeqCst), 2);
    assert_eq!(read.active.load(Ordering::SeqCst), 0);
    assert_eq!(
        tool_results(&context)
            .iter()
            .map(|r| r["content"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["0", "1", "2", "3", "4", "5"]
    );
    assert_paired(&context);
    let events = events.lock().unwrap();
    assert!(matches!(
        events.first(),
        Some(AgentEvent::ToolBatchStart {
            max_parallel: 2,
            ..
        })
    ));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::ToolBatchEnd {
            tool_count: 6,
            failed: 0,
            paused: 0,
            ..
        })
    ));
    let end = |id: &str| {
        events.iter().position(|event| matches!(event, AgentEvent::ToolExecutionEnd {call_id, ..} if call_id == id)).unwrap()
    };
    assert!(
        end("r1") < end("r0"),
        "completion events follow actual completion, not transcript order"
    );
}

#[tokio::test]
async fn per_tool_limit_is_respected_with_a_larger_pool() {
    let read = Arc::new(PoolRead::new(1, false, false));
    let tools: Vec<Arc<dyn Tool>> = vec![read.clone()];
    let mut config = AgentLoopConfig::default();
    config.features.max_parallel_tools = 4;
    let calls = (0..8)
        .map(|n| call(&format!("r{n}"), "read", json!({"n":n})))
        .collect();
    let emit: EmitFn = Arc::new(|_| {});
    let (context, _, _) = run_batch(&tools, calls, &config, &ToolRuntime::null(), None, &emit)
        .await
        .unwrap();
    assert_eq!(read.peak.load(Ordering::SeqCst), 1);
    assert_paired(&context);
}

#[tokio::test]
async fn reads_do_not_overtake_writes_and_resume_parallelism_after_barriers() {
    let value = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(AtomicUsize::new(0));
    let tools: Vec<Arc<dyn Tool>> = [true, false]
        .into_iter()
        .map(|ro| {
            Arc::new(StateTool {
                name: if ro { "read" } else { "write" },
                read_only: ro,
                confirm: false,
                permissions: &[Permission::None],
                value: value.clone(),
                executions: executions.clone(),
            }) as Arc<dyn Tool>
        })
        .collect();
    let calls = ["read", "read", "write", "read", "read", "write", "read"]
        .into_iter()
        .enumerate()
        .map(|(n, name)| call(&format!("c{n}"), name, json!({})))
        .collect();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let emit: EmitFn = Arc::new(move |event| captured.lock().unwrap().push(event));
    let (context, _, _) = run_batch(
        &tools,
        calls,
        &AgentLoopConfig::default(),
        &ToolRuntime::null(),
        None,
        &emit,
    )
    .await
    .unwrap();
    assert_eq!(
        tool_results(&context)
            .iter()
            .map(|r| r["content"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["0", "0", "1", "1", "1", "2", "2"]
    );
    assert_eq!(executions.load(Ordering::SeqCst), 7);
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, AgentEvent::ToolBatchStart { .. }))
            .count(),
        2
    );
    assert_paired(&context);
}

#[tokio::test]
async fn cancellation_drops_inflight_reads_and_does_not_start_later_writes() {
    let read = Arc::new(PoolRead::new(8, false, true));
    let executions = Arc::new(AtomicUsize::new(0));
    let tools: Vec<Arc<dyn Tool>> = vec![
        read.clone(),
        Arc::new(StateTool {
            name: "write",
            read_only: false,
            confirm: false,
            permissions: &[Permission::None],
            value: Arc::new(AtomicUsize::new(0)),
            executions: executions.clone(),
        }),
    ];
    let token = CancellationToken::new();
    let config = AgentLoopConfig::default();
    let runtime = ToolRuntime::null();
    let emit: EmitFn = Arc::new(|_| {});
    let run = run_batch(
        &tools,
        vec![
            call("r0", "read", json!({"n":0})),
            call("r1", "read", json!({"n":1})),
            call("w", "write", json!({})),
        ],
        &config,
        &runtime,
        Some(token.clone()),
        &emit,
    );
    let cancel = async {
        read.started.notified().await;
        token.cancel();
    };
    let (result, _) =
        tokio::time::timeout(Duration::from_secs(2), async { tokio::join!(run, cancel) })
            .await
            .unwrap();
    assert!(result.is_err());
    assert_eq!(read.active.load(Ordering::SeqCst), 0);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_and_explicit_batches_use_one_round_and_pair_individual_errors() {
    for explicit in [false, true] {
        let read = Arc::new(PoolRead::new(8, false, false));
        let executions = Arc::new(AtomicUsize::new(0));
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(crate::tools::parallel::ParallelTool),
            read.clone(),
            Arc::new(StateTool {
                name: "network",
                read_only: true,
                confirm: false,
                permissions: &[Permission::Network],
                value: Arc::new(AtomicUsize::new(0)),
                executions: executions.clone(),
            }),
            Arc::new(StateTool {
                name: "file",
                read_only: true,
                confirm: false,
                permissions: &[Permission::FsRead],
                value: Arc::new(AtomicUsize::new(0)),
                executions: executions.clone(),
            }),
            Arc::new(StateTool {
                name: "shell",
                read_only: false,
                confirm: true,
                permissions: &[Permission::Shell],
                value: Arc::new(AtomicUsize::new(0)),
                executions: executions.clone(),
            }),
        ];
        let calls = vec![
            call("good", "read", json!({"n":1})),
            call("bad", "read", json!({"n":"wrong"})),
            call("unknown", "absent", json!({})),
            call("net", "network", json!({"url":"https://example.com"})),
            call(
                "path",
                "file",
                json!({"path":std::env::current_dir().unwrap().join("outside-boris-root.txt")}),
            ),
            call("sh", "shell", json!({"command":"git status"})),
        ];
        let client = ScriptedClient::new(
            "test",
            vec![
                response(&calls, explicit),
                json!({"role":"assistant","content":"The checks finished with limitations."}),
            ],
        );
        let runtime = ToolRuntime::null();
        let config = AgentLoopConfig::default();
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "check values");
        let result = agent_loop(
            LoopState {
                context: &mut context,
                tools: &tools,
                runtime: &runtime,
                client: &client,
                activated: None,
            },
            "check values",
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
        assert_eq!(result.tool_rounds, 1);
        assert_eq!(client.calls.lock().unwrap().len(), 2);
        assert_eq!(result.tools_used.len(), 6);
        assert_eq!(
            executions.load(Ordering::SeqCst),
            0,
            "hard-denied leaf tools cannot execute through a wrapper"
        );
        assert_eq!(
            read.peak.load(Ordering::SeqCst),
            1,
            "invalid arguments cannot execute"
        );
        assert_eq!(
            tool_results(&context)
                .iter()
                .filter(|r| r["content"].as_str().unwrap().starts_with("Error"))
                .count(),
            5
        );
        assert_paired(&context);
    }
}

#[tokio::test]
async fn namespaced_explicit_batch_preserves_approval_verdicts_and_remaining_siblings() {
    for approved in [false, true] {
        let reads = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        let value = Arc::new(AtomicUsize::new(0));
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(crate::tools::parallel::ParallelTool),
            Arc::new(StateTool {
                name: "read",
                read_only: true,
                confirm: false,
                permissions: &[Permission::None],
                value: value.clone(),
                executions: reads.clone(),
            }),
            Arc::new(StateTool {
                name: "write",
                read_only: false,
                confirm: true,
                permissions: &[Permission::None],
                value: value.clone(),
                executions: writes.clone(),
            }),
        ];
        let calls = ["read", "read", "write", "read", "read"]
            .into_iter()
            .enumerate()
            .map(|(n, name)| call(&format!("c{n}"), &format!("functions.{name}"), json!({})))
            .collect::<Vec<_>>();
        let client = ScriptedClient::new(
            "test",
            vec![
                response(&calls, true),
                json!({"role":"assistant","content":"Finished the requested checks."}),
            ],
        );
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "check values");
        let runtime = ToolRuntime::null();
        let config = AgentLoopConfig::default();
        let paused = agent_loop(
            LoopState {
                context: &mut context,
                tools: &tools,
                runtime: &runtime,
                client: &client,
                activated: None,
            },
            "check values",
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
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        let pending = paused.pending_turn.unwrap();
        assert_eq!(pending.pending.name, "write");
        assert_eq!(pending.remaining_calls.len(), 2);
        resume_pending_tool(
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
        assert_eq!(reads.load(Ordering::SeqCst), 4, "prefix must not replay");
        assert_eq!(writes.load(Ordering::SeqCst), usize::from(approved));
        assert_paired(&context);
    }
}

#[tokio::test]
async fn explicit_batch_preserves_typed_input_pause_and_tail() {
    let read = Arc::new(PoolRead::new(8, false, false));
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(crate::tools::parallel::ParallelTool),
        read,
        Arc::new(crate::tools::collect_input::CollectInputTool),
    ];
    let calls = vec![
        call("r0", "read", json!({"n":0})),
        call(
            "input",
            "collect_input",
            json!({"spoken":"Type the value.","kind":"exact"}),
        ),
        call("r1", "read", json!({"n":1})),
    ];
    let client = ScriptedClient::new(
        "test",
        vec![
            response(&calls, true),
            json!({"role":"assistant","content":"The checks finished."}),
        ],
    );
    let mut context = Context::new(20);
    context.push(Role::System, "sys");
    context.push(Role::User, "check values");
    let runtime = ToolRuntime::null();
    let config = AgentLoopConfig::default();
    let paused = agent_loop(
        LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        },
        "check values",
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
        crate::AgentOutcome::NeedsInput { .. }
    ));
    let pending = paused.pending_turn.unwrap();
    assert_eq!(pending.remaining_calls.len(), 1);
    resume_pending_input(
        LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        },
        pending,
        Some("chosen value".into()),
        &config,
        None,
        None,
    )
    .await
    .unwrap();
    assert_paired(&context);
}

#[tokio::test]
async fn late_pause_keeps_completed_siblings_and_untouched_tail() {
    struct ChangingRead {
        changed: Arc<std::sync::atomic::AtomicBool>,
        executions: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl Tool for ChangingRead {
        fn name(&self) -> &str {
            "changing_read"
        }
        fn description(&self) -> &str {
            "read whose approval requirement changes after preflight"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object"})
        }
        fn meta(&self) -> ToolMeta {
            let changed = self.changed.load(Ordering::SeqCst);
            ToolMeta::with_risk(if changed {
                ToolRisk::Dangerous
            } else {
                ToolRisk::Safe
            })
            .read_only(true)
            .confirm(changed)
        }
        async fn execute(
            &self,
            _: &crate::tool_context::ToolCallContext,
            _: Value,
        ) -> Result<String, ToolError> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok("checked".into())
        }
    }
    struct TriggerRead {
        changed: Arc<std::sync::atomic::AtomicBool>,
        executions: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl Tool for TriggerRead {
        fn name(&self) -> &str {
            "trigger"
        }
        fn description(&self) -> &str {
            "read that changes a sibling's approval requirement"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object"})
        }
        fn meta(&self) -> ToolMeta {
            ToolMeta::safe_default().read_only(true)
        }
        async fn execute(
            &self,
            _: &crate::tool_context::ToolCallContext,
            _: Value,
        ) -> Result<String, ToolError> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            self.changed.store(true, Ordering::SeqCst);
            Ok("checked".into())
        }
    }
    let changing = Arc::new(AtomicUsize::new(0));
    let changed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let executions = Arc::new(AtomicUsize::new(0));
    let value = Arc::new(AtomicUsize::new(0));
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ChangingRead {
            changed: changed.clone(),
            executions: changing.clone(),
        }),
        Arc::new(TriggerRead {
            changed,
            executions: executions.clone(),
        }),
        Arc::new(StateTool {
            name: "read",
            read_only: true,
            confirm: false,
            permissions: &[Permission::None],
            value: value.clone(),
            executions: executions.clone(),
        }),
        Arc::new(StateTool {
            name: "write",
            read_only: false,
            confirm: false,
            permissions: &[Permission::None],
            value,
            executions: executions.clone(),
        }),
    ];
    let calls = vec![
        call("finished", "trigger", json!({})),
        call("late", "changing_read", json!({})),
        call("untouched", "write", json!({})),
        call("after", "read", json!({})),
    ];
    let config = AgentLoopConfig::default();
    let runtime = ToolRuntime::null();
    let emit: EmitFn = Arc::new(|_| {});
    let (mut context, used, result) = run_batch(&tools, calls, &config, &runtime, None, &emit)
        .await
        .unwrap();
    assert_eq!(used, ["trigger"]);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(changing.load(Ordering::SeqCst), 0);
    let ToolBatchResult::Paused { pending_turn, .. } = result else {
        panic!("expected late approval pause")
    };
    assert_eq!(
        pending_turn
            .remaining_calls
            .iter()
            .map(|c| c.call_id.as_str())
            .collect::<Vec<_>>(),
        ["untouched", "after"]
    );
    let client = ScriptedClient::new(
        "test",
        vec![json!({"role":"assistant","content":"The checks finished."})],
    );
    resume_pending_tool(
        LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        },
        pending_turn,
        true,
        &config,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        executions.load(Ordering::SeqCst),
        3,
        "completed read must not replay"
    );
    assert_eq!(changing.load(Ordering::SeqCst), 1);
    assert_paired(&context);
}

#[tokio::test]
async fn malformed_parallel_executes_no_children_and_returns_outer_error() {
    let read = Arc::new(PoolRead::new(8, false, false));
    let tools: Vec<Arc<dyn Tool>> =
        vec![Arc::new(crate::tools::parallel::ParallelTool), read.clone()];
    let invalid = json!({"role":"assistant","content":null,"tool_calls":[{
        "id":"outer","type":"function","function":{"name":"parallel","arguments":json!({"tool_uses":[
            {"recipient_name":"read","parameters":{"n":1}},
            {"recipient_name":"parallel","parameters":{}}
        ]}).to_string()}
    }]});
    let client = ScriptedClient::new(
        "test",
        vec![
            invalid,
            json!({"role":"assistant","content":"The batch needs corrected arguments."}),
        ],
    );
    let runtime = ToolRuntime::null();
    let config = AgentLoopConfig::default();
    let mut context = Context::new(20);
    context.push(Role::System, "sys");
    context.push(Role::User, "check values");
    agent_loop(
        LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        },
        "check values",
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
    assert_eq!(read.peak.load(Ordering::SeqCst), 0);
    let results = tool_results(&context);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["tool_call_id"], "outer");
    assert!(results[0]["content"].as_str().unwrap().contains("nest"));
}

#[tokio::test]
async fn abort_closes_every_expanded_child_without_reexecuting_finished_reads() {
    let read = Arc::new(PoolRead::new(8, false, false));
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(crate::tools::parallel::ParallelTool),
        read.clone(),
        Arc::new(crate::tools::collect_input::CollectInputTool),
    ];
    let calls = vec![
        call("read", "read", json!({"n":1})),
        call(
            "input",
            "collect_input",
            json!({"spoken":"Type the value."}),
        ),
        call("tail", "read", json!({"n":2})),
    ];
    let client = ScriptedClient::new("test", vec![response(&calls, true)]);
    let runtime = ToolRuntime::null();
    let config = AgentLoopConfig::default();
    let mut context = Context::new(20);
    context.push(Role::System, "sys");
    context.push(Role::User, "check values");
    let result = agent_loop(
        LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        },
        "check values",
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
    assert!(result.pending_turn.is_some());
    assert_eq!(context.resolve_pending_tool_calls_as_cancelled(), 2);
    assert_paired(&context);
}
