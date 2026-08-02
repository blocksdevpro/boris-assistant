use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use crate::{
    client::LlmClient,
    context::{Context, Message, Role},
    error::AgentError,
    memory::{
        extract_heuristic, extract_with_llm, should_llm_extract, ProfileStore, UserProfile,
        PERSONAL_CONTEXT_MAX_CHARS,
    },
    observe::TurnReport,
    outcome::AgentOutcome,
    runtime::{
        InvokeOptions, InvokeResult, PendingTurn, RawToolCall, SandboxConfig, ToolInvocation,
        ToolRuntime, JsonlAuditSink, NullAuditSink,
    },
    tool::Tool,
};

/// Hard cap on tool-call rounds per user turn. Prevents unbounded ReAct loops
/// if the model keeps requesting tools (or invents unknown ones).
const MAX_TOOL_ROUNDS: usize = 5;

/// Max characters of user text included in turn-start logs (avoids dumping secrets).
const LOG_PREVIEW_CHARS: usize = 80;

/// Optional durable personal context attached to the engine.
struct PersonalMemory {
    store: ProfileStore,
    /// Shared with tools so mid-turn saves stay coherent.
    profile: Arc<Mutex<UserProfile>>,
    /// When true, may spend an extra LLM call after a turn to learn.
    llm_extract: bool,
}

pub struct AgentEngine {
    client: Box<dyn LlmClient>,
    tools: Vec<Box<dyn Tool>>,
    context: Context,
    /// Persona + channel rules without the dynamic personal_context block.
    base_system_prompt: String,
    personal: Option<PersonalMemory>,
    runtime: ToolRuntime,
    /// Set when a turn is paused for HITL confirmation.
    pending_turn: Option<PendingTurn>,
    /// Optional ids for audit correlation (host may set).
    session_id: Option<String>,
    turn_id: Option<String>,
}

/// Truncate `s` for logging; appends `…` when cut.
fn log_preview(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let preview: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

impl AgentEngine {
    /// Create a new engine.
    ///
    /// `system_prompt` sets the assistant's persona and hard rules.
    /// Tools are optional — register via [`Self::register_tool`] / builtins.
    pub fn new(client: Box<dyn LlmClient>, system_prompt: &str) -> Self {
        let mut context = Context::new(20);
        context.push(Role::System, system_prompt);
        Self {
            client,
            tools: vec![],
            context,
            base_system_prompt: system_prompt.to_string(),
            personal: None,
            runtime: ToolRuntime::null(),
            pending_turn: None,
            session_id: None,
            turn_id: None,
        }
    }

    /// Configure sandbox policy + optional JSONL audit path.
    pub fn configure_runtime(&mut self, policy: SandboxConfig, audit_path: Option<PathBuf>) {
        if let Some(path) = audit_path {
            self.runtime = ToolRuntime::new(policy, Box::new(JsonlAuditSink::new(path)));
        } else {
            self.runtime = ToolRuntime::new(policy, Box::new(NullAuditSink));
        }
    }

    pub fn set_session_id(&mut self, id: Option<String>) {
        self.session_id = id;
    }

    pub fn set_turn_id(&mut self, id: Option<String>) {
        self.turn_id = id;
    }

    /// True when a confirmation is outstanding (host must resume or cancel).
    pub fn has_pending_confirmation(&self) -> bool {
        self.pending_turn.is_some()
    }

    /// Drop pending HITL state without executing (e.g. Stop / new session).
    pub fn cancel_pending(&mut self) {
        if self.pending_turn.take().is_some() {
            info!("pending tool confirmation cancelled");
        }
    }

    /// Enable durable personal context stored at `profile_path`.
    pub fn enable_personal_context(
        &mut self,
        profile_path: impl Into<PathBuf>,
        llm_extract: bool,
    ) -> Result<Arc<Mutex<UserProfile>>, String> {
        let store = ProfileStore::new(profile_path);
        let profile = store.load()?;
        let shared = Arc::new(Mutex::new(profile));
        self.personal = Some(PersonalMemory {
            store,
            profile: shared.clone(),
            llm_extract,
        });
        self.refresh_system_prompt();
        info!(path = %self.personal.as_ref().unwrap().store.path().display(), "personal context enabled");
        Ok(shared)
    }

    pub fn personal_profile(&self) -> Option<Arc<Mutex<UserProfile>>> {
        self.personal.as_ref().map(|p| p.profile.clone())
    }

    pub fn profile_store_path(&self) -> Option<PathBuf> {
        self.personal.as_ref().map(|p| p.store.path().to_path_buf())
    }

    pub fn refresh_system_prompt(&mut self) {
        let composed = self.composed_system_prompt();
        self.context.set_system(composed);
    }

    fn composed_system_prompt(&self) -> String {
        let Some(mem) = &self.personal else {
            return self.base_system_prompt.clone();
        };
        let block = match mem.profile.lock() {
            Ok(p) => p.render_block(PERSONAL_CONTEXT_MAX_CHARS),
            Err(_) => String::new(),
        };
        if block.is_empty() {
            self.base_system_prompt.clone()
        } else {
            format!("{}\n\n{block}", self.base_system_prompt)
        }
    }

    pub fn set_base_system_prompt(&mut self, system_prompt: &str) {
        self.base_system_prompt = system_prompt.to_string();
        self.refresh_system_prompt();
    }

    pub fn register_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn register_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        for tool in tools {
            self.register_tool(tool);
        }
    }

    /// Clear conversation to a fresh system-only context (new session).
    pub fn reset_conversation(&mut self, system_prompt: &str) {
        self.cancel_pending();
        self.base_system_prompt = system_prompt.to_string();
        self.context.messages.clear();
        let composed = self.composed_system_prompt();
        self.context.push(Role::System, composed);
    }

    pub fn load_session_history(&mut self, system_prompt: &str, history: Vec<Message>) {
        self.cancel_pending();
        self.base_system_prompt = system_prompt.to_string();
        let composed = self.composed_system_prompt();
        self.context.load_history(&composed, history);
    }

    pub fn export_messages(&self) -> Vec<Message> {
        self.context.messages().to_vec()
    }

    fn tools_json(&self) -> Value {
        if self.tools.is_empty() {
            return Value::Null;
        }
        let list: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name":        t.name(),
                        "description": t.description(),
                        "parameters":  t.parameters(),
                    }
                })
            })
            .collect();
        json!(list)
    }

    /// Back-compat entry. Delegates to [`Self::run_turn`].
    pub async fn chat(&mut self, message: &str) -> Result<AgentOutcome, AgentError> {
        self.run_turn(message).await
    }

    /// Primary turn API: one user message → [`AgentOutcome`].
    pub async fn run_turn(&mut self, user_text: &str) -> Result<AgentOutcome, AgentError> {
        self.run_turn_with_report(user_text)
            .await
            .map(|(outcome, _report)| outcome)
    }

    /// Run one user turn and return both the outcome and a [`TurnReport`].
    ///
    /// May return [`AgentOutcome::NeedsConfirmation`] before the turn is fully
    /// finished — then the host must call [`Self::resume_confirmation`].
    pub async fn run_turn_with_report(
        &mut self,
        user_text: &str,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        if self.pending_turn.is_some() {
            return Err(AgentError::new(
                "cannot start a new turn while a tool confirmation is pending",
            ));
        }

        if self.personal.is_some() {
            self.refresh_system_prompt();
        }

        let started = Instant::now();
        let preview = log_preview(user_text, LOG_PREVIEW_CHARS);
        info!(
            model = %self.client.model(),
            message_len = user_text.len(),
            preview = %preview,
            "agent turn start"
        );

        let snapshot = self.context.messages.clone();
        self.context.push(Role::User, user_text);

        match self
            .execute_turn_loop(user_text, Vec::new(), 0, 0, false)
            .await
        {
            Ok(loop_out) => self.finish_loop(started, user_text, loop_out).await,
            Err(e) => {
                self.context.messages = snapshot;
                self.pending_turn = None;
                error!(
                    error = %e,
                    duration_ms = started.elapsed().as_millis() as u64,
                    "agent turn failed"
                );
                Err(e)
            }
        }
    }

    /// Continue after the host collected a yes/no for a pending tool.
    pub async fn resume_confirmation(
        &mut self,
        pending_id: &str,
        approved: bool,
    ) -> Result<AgentOutcome, AgentError> {
        self.resume_confirmation_with_report(pending_id, approved)
            .await
            .map(|(o, _)| o)
    }

    /// Same as [`Self::resume_confirmation`] with a [`TurnReport`].
    pub async fn resume_confirmation_with_report(
        &mut self,
        pending_id: &str,
        approved: bool,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        let started = Instant::now();
        let pending_turn = self
            .pending_turn
            .take()
            .ok_or_else(|| AgentError::new("no pending tool confirmation to resume"))?;

        if pending_turn.pending.id != pending_id {
            // Put it back so the host can retry with the correct id.
            let id = pending_turn.pending.id.clone();
            self.pending_turn = Some(pending_turn);
            return Err(AgentError::new(format!(
                "pending id mismatch: expected `{id}`, got `{pending_id}`"
            )));
        }

        let user_text = pending_turn.user_text.clone();
        let mut tools_used = pending_turn.tools_used;
        let mut tool_rounds = pending_turn.tool_rounds;
        let mut confirms_used = pending_turn.confirms_used;
        let remaining = pending_turn.remaining_calls;
        let pending = pending_turn.pending;

        // Resolve the confirmed/rejected tool first.
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == pending.name)
            .ok_or_else(|| {
                AgentError::unknown_tool(format!("unknown tool on resume: {}", pending.name))
            })?;

        let observation = if approved {
            let inv = ToolInvocation {
                call_id: pending.call_id.clone(),
                name: pending.name.clone(),
                args: pending.args.clone(),
                session_id: self.session_id.clone(),
                turn_id: self.turn_id.clone(),
            };
            let opts = InvokeOptions {
                skip_confirmation: true,
                confirms_used,
            };
            match self.runtime.invoke(tool.as_ref(), inv, opts).await {
                InvokeResult::Observation(s) => s,
                InvokeResult::Denied { reason } => format!("Error: {reason}"),
                InvokeResult::NeedsConfirmation { .. } => {
                    format!("Error: unexpected confirmation after grant")
                }
            }
        } else {
            self.runtime.audit_rejection(
                &pending,
                self.session_id.as_deref(),
                self.turn_id.as_deref(),
            );
            "Error: user declined this action".to_string()
        };

        tools_used.push(pending.name.clone());
        self.context.push(
            Role::Tool,
            json!({ "tool_call_id": pending.call_id, "content": observation }),
        );
        self.maybe_refresh_after_tools(&tools_used);

        // Process remaining sibling calls from the same model round.
        match self
            .process_tool_calls(
                remaining,
                &mut tools_used,
                &mut tool_rounds,
                &mut confirms_used,
                &user_text,
            )
            .await?
        {
            ToolBatchResult::Continue => {}
            ToolBatchResult::Paused(outcome) => {
                let report = TurnReport {
                    duration: started.elapsed(),
                    tool_rounds,
                    tools_used: tools_used.clone(),
                    outcome: "needs_confirm".into(),
                    approx_chars_in: self.context.as_json().to_string().len(),
                };
                return Ok((outcome, report));
            }
        }

        match self
            .execute_turn_loop(&user_text, tools_used, tool_rounds, confirms_used, true)
            .await
        {
            Ok(loop_out) => self.finish_loop(started, &user_text, loop_out).await,
            Err(e) => {
                error!(error = %e, "agent resume failed");
                Err(e)
            }
        }
    }

    async fn finish_loop(
        &mut self,
        started: Instant,
        user_text: &str,
        loop_out: TurnLoopResult,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        let duration = started.elapsed();
        let outcome_label = match &loop_out.outcome {
            AgentOutcome::Speak { expect_reply, .. } if *expect_reply => "speak_await",
            AgentOutcome::Speak { .. } => "speak",
            AgentOutcome::Silent => "silent",
            AgentOutcome::NeedsConfirmation { .. } => "needs_confirm",
        };
        let approx_chars_in = self.context.as_json().to_string().len();
        let report = TurnReport {
            duration,
            tool_rounds: loop_out.tool_rounds,
            tools_used: loop_out.tools_used.clone(),
            outcome: match &loop_out.outcome {
                AgentOutcome::NeedsConfirmation { .. } => "needs_confirm".into(),
                AgentOutcome::Silent => "silent".into(),
                AgentOutcome::Speak { .. } => "speak".into(),
            },
            approx_chars_in,
        };
        info!(
            outcome = outcome_label,
            duration_ms = duration.as_millis() as u64,
            tool_rounds = loop_out.tool_rounds,
            tools = ?loop_out.tools_used,
            approx_chars_in,
            "agent turn end"
        );

        // Only learn on finished turns (not HITL pause).
        if !matches!(loop_out.outcome, AgentOutcome::NeedsConfirmation { .. }) {
            let assistant_text = match &loop_out.outcome {
                AgentOutcome::Speak { text, .. } => text.as_str(),
                AgentOutcome::Silent => "",
                AgentOutcome::NeedsConfirmation { .. } => "",
            };
            self.after_turn_learn(user_text, assistant_text, &loop_out.tools_used)
                .await;
        }

        Ok((loop_out.outcome, report))
    }

    async fn after_turn_learn(
        &mut self,
        user_text: &str,
        assistant_text: &str,
        tools_used: &[String],
    ) {
        let Some(mem) = &self.personal else {
            return;
        };
        let llm_extract_enabled = mem.llm_extract;

        let mut delta = extract_heuristic(user_text);
        let heuristic_hit = !delta.is_empty();

        let (turns_seen, profile_summary, do_llm) = {
            let Ok(mut p) = mem.profile.lock() else {
                return;
            };
            p.turns_seen = p.turns_seen.saturating_add(1);
            let turns_seen = p.turns_seen;
            let summary = if p.is_empty() {
                "(empty)".to_string()
            } else {
                p.render_block(400)
            };
            let do_llm = llm_extract_enabled
                && should_llm_extract(user_text, tools_used, turns_seen, heuristic_hit);
            (turns_seen, summary, do_llm)
        };

        if do_llm {
            match extract_with_llm(
                self.client.as_ref(),
                user_text,
                assistant_text,
                &profile_summary,
            )
            .await
            {
                Ok(llm_delta) if !llm_delta.is_empty() => {
                    debug!(turns_seen, "personal llm extract produced updates");
                    if let Some(n) = llm_delta.preferred_name.clone() {
                        delta.preferred_name = Some(n);
                    }
                    if let Some(a) = llm_delta.address_as.clone() {
                        delta.address_as = Some(a);
                    }
                    delta.preferences_add.extend(llm_delta.preferences_add);
                    delta.facts_add.extend(llm_delta.facts_add);
                    delta
                        .facts_remove_query
                        .extend(llm_delta.facts_remove_query);
                    delta.ongoing_add.extend(llm_delta.ongoing_add);
                    if llm_delta.ongoing_replace.is_some() {
                        delta.ongoing_replace = llm_delta.ongoing_replace;
                    }
                }
                Ok(_) => {
                    debug!(turns_seen, "personal llm extract empty");
                }
                Err(e) => {
                    warn!(error = %e, "personal llm extract failed");
                }
            }
        }

        if delta.is_empty() && !heuristic_hit {
            if let Some(mem) = &self.personal {
                if let Ok(p) = mem.profile.lock() {
                    let _ = mem.store.save(&p);
                }
            }
            return;
        }

        if let Some(mem) = &self.personal {
            if let Ok(mut p) = mem.profile.lock() {
                let before_empty = p.is_empty();
                delta.apply(&mut p);
                if let Err(e) = mem.store.save(&p) {
                    warn!(error = %e, "failed to save personal profile");
                } else {
                    info!(
                        was_empty = before_empty,
                        name = ?p.preferred_name,
                        facts = p.facts.len(),
                        prefs = p.preferences.len(),
                        "personal context updated"
                    );
                }
            }
        }

        self.refresh_system_prompt();
    }

    /// Internal ReAct loop. When `resume_mid` is true, the user message is already on context.
    async fn execute_turn_loop(
        &mut self,
        user_text: &str,
        mut tools_used: Vec<String>,
        mut tool_rounds: u32,
        mut confirms_used: u32,
        resume_mid: bool,
    ) -> Result<TurnLoopResult, AgentError> {
        let _ = resume_mid; // context already contains history

        for round in 0..=MAX_TOOL_ROUNDS {
            let response = self
                .client
                .complete(self.context.as_json(), self.tools_json())
                .await?;

            let tool_calls = &response["tool_calls"];
            if let Some(calls) = tool_calls.as_array() {
                if !calls.is_empty() {
                    if round == MAX_TOOL_ROUNDS {
                        return Err(AgentError::tool_loop(format!(
                            "tool loop exceeded {MAX_TOOL_ROUNDS} rounds without a final reply"
                        )));
                    }

                    tool_rounds += 1;
                    self.context.push(Role::Assistant, response.clone());

                    let raw_calls: Vec<RawToolCall> = calls
                        .iter()
                        .map(|call| {
                            let call_id = call["id"].as_str().unwrap_or("").to_string();
                            let name = call["function"]["name"].as_str().unwrap_or("").to_string();
                            let args: Value = serde_json::from_str(
                                call["function"]["arguments"].as_str().unwrap_or("{}"),
                            )
                            .unwrap_or(json!({}));
                            RawToolCall {
                                call_id,
                                name,
                                args,
                            }
                        })
                        .collect();

                    match self
                        .process_tool_calls(
                            raw_calls,
                            &mut tools_used,
                            &mut tool_rounds,
                            &mut confirms_used,
                            user_text,
                        )
                        .await?
                    {
                        ToolBatchResult::Continue => continue,
                        ToolBatchResult::Paused(outcome) => {
                            return Ok(TurnLoopResult {
                                outcome,
                                tool_rounds,
                                tools_used,
                            });
                        }
                    }
                }
            }

            let reply = response["content"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            self.context.push(Role::Assistant, reply.clone());
            let outcome = if reply.is_empty() {
                AgentOutcome::Silent
            } else {
                AgentOutcome::speak(reply)
            };
            return Ok(TurnLoopResult {
                outcome,
                tool_rounds,
                tools_used,
            });
        }

        Err(AgentError::tool_loop("tool loop exhausted"))
    }

    async fn process_tool_calls(
        &mut self,
        calls: Vec<RawToolCall>,
        tools_used: &mut Vec<String>,
        _tool_rounds: &mut u32,
        confirms_used: &mut u32,
        user_text: &str,
    ) -> Result<ToolBatchResult, AgentError> {
        let mut iter = calls.into_iter();
        while let Some(call) = iter.next() {
            let tool = self
                .tools
                .iter()
                .find(|t| t.name() == call.name)
                .ok_or_else(|| {
                    AgentError::unknown_tool(format!(
                        "unknown tool requested by model: {}",
                        call.name
                    ))
                })?;

            let inv = ToolInvocation {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                args: call.args.clone(),
                session_id: self.session_id.clone(),
                turn_id: self.turn_id.clone(),
            };
            let opts = InvokeOptions {
                skip_confirmation: false,
                confirms_used: *confirms_used,
            };

            match self.runtime.invoke(tool.as_ref(), inv, opts).await {
                InvokeResult::Observation(result) => {
                    tools_used.push(call.name.clone());
                    self.context.push(
                        Role::Tool,
                        json!({ "tool_call_id": call.call_id, "content": result }),
                    );
                    self.maybe_refresh_after_tools(tools_used);
                }
                InvokeResult::Denied { reason } => {
                    tools_used.push(call.name.clone());
                    self.context.push(
                        Role::Tool,
                        json!({
                            "tool_call_id": call.call_id,
                            "content": format!("Error: {reason}")
                        }),
                    );
                }
                InvokeResult::NeedsConfirmation {
                    pending,
                    speak_prompt,
                } => {
                    *confirms_used = confirms_used.saturating_add(1);
                    let remaining: Vec<RawToolCall> = iter.collect();
                    self.pending_turn = Some(PendingTurn {
                        pending: pending.clone(),
                        remaining_calls: remaining,
                        tools_used: tools_used.clone(),
                        tool_rounds: *_tool_rounds,
                        confirms_used: *confirms_used,
                        user_text: user_text.to_string(),
                    });
                    return Ok(ToolBatchResult::Paused(AgentOutcome::NeedsConfirmation {
                        text: speak_prompt,
                        pending,
                    }));
                }
            }
        }
        Ok(ToolBatchResult::Continue)
    }

    fn maybe_refresh_after_tools(&mut self, tools_used: &[String]) {
        if tools_used.iter().any(|n| {
            n == "save_user_fact" || n == "update_user_profile" || n == "get_user_context"
        }) {
            self.refresh_system_prompt();
        }
    }
}

struct TurnLoopResult {
    outcome: AgentOutcome,
    tool_rounds: u32,
    tools_used: Vec<String>,
}

enum ToolBatchResult {
    Continue,
    Paused(AgentOutcome),
}
