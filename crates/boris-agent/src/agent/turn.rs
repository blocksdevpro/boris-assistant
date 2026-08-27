//! Turn execution: prompt, resume HITL, finish/report.

use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::context::{ContextBudget, RetrievedMemory, Role, TaskStatus};
use crate::error::{AgentError, AgentErrorKind};
use crate::loop_::{self, LoopState};
use crate::observe::{TurnOutcomeKind, TurnReport};
use crate::outcome::AgentOutcome;
use crate::types::{AgentEvent, AgentLoopConfig, LoopResult};

use super::{log_preview, Agent, LOG_PREVIEW_CHARS};

impl Agent {
    /// Populate the derived context with relevant durable-memory snippets before
    /// the first completion. Retrieval failure is deliberately non-fatal.
    async fn retrieve_memory_for_turn(&mut self, user_text: &str) {
        if let Some(store) = self.memory_store.clone() {
            let raw_query = user_text.trim().to_string();
            if raw_query.len() < 3 {
                self.context.set_retrieved_memory(Vec::new());
                return;
            }
            let lifecycle = crate::memory::extract_heuristic(&raw_query);
            if lifecycle.forget_all
                || lifecycle.forget_preferred_name
                || !lifecycle.facts_remove_query.is_empty()
            {
                // Forgetting is handled by post-turn ingestion. Never inject
                // anything while the human is asking Boris to remove it.
                self.context.set_retrieved_memory(Vec::new());
                return;
            }
            let Some(query) = proactive_memory_query(&raw_query) else {
                self.context.set_retrieved_memory(Vec::new());
                return;
            };
            let result = tokio::task::spawn_blocking(move || store.search(&query, 8)).await;
            match result {
                Ok(Ok(hits)) => {
                    let memories = hits
                        .into_iter()
                        .map(|hit| RetrievedMemory {
                            snippet: hit.record.text,
                            path: format!("memory/{}", hit.record.id),
                            source: match hit.record.kind {
                                crate::memory::MemoryKind::Semantic => "fact",
                                crate::memory::MemoryKind::Episodic => "event",
                                crate::memory::MemoryKind::Project => "project",
                                crate::memory::MemoryKind::Procedural => "preference",
                            }
                            .to_string(),
                            score: (hit.score * 1_000.0).round().max(0.0) as u32,
                        })
                        .collect::<Vec<_>>();
                    tracing::debug!(hits = memories.len(), "canonical memory retrieval complete");
                    self.context.set_retrieved_memory(memories);
                }
                Ok(Err(error)) => {
                    warn!(%error, "canonical memory retrieval skipped");
                    self.context.set_retrieved_memory(Vec::new());
                }
                Err(error) => {
                    warn!(%error, "canonical memory retrieval task failed");
                    self.context.set_retrieved_memory(Vec::new());
                }
            }
            return;
        }
        let Some(memory) = self.long_term.clone() else {
            self.context.set_retrieved_memory(Vec::new());
            return;
        };
        let raw_query = user_text.trim().to_string();
        if raw_query.len() < 3 {
            self.context.set_retrieved_memory(Vec::new());
            return;
        }
        let lifecycle = crate::memory::extract_heuristic(&raw_query);
        if lifecycle.forget_all
            || lifecycle.forget_preferred_name
            || !lifecycle.facts_remove_query.is_empty()
        {
            // Never re-inject the very memory the human is asking us to forget.
            self.context.set_retrieved_memory(Vec::new());
            return;
        }
        let Some(query) = proactive_memory_query(&raw_query) else {
            self.context.set_retrieved_memory(Vec::new());
            return;
        };
        let profile = self
            .personal
            .as_ref()
            .and_then(|personal| personal.profile.lock().ok().map(|profile| profile.clone()));
        let result = tokio::task::spawn_blocking(move || memory.search(&query, 4)).await;
        match result {
            Ok(Ok(hits)) => {
                let memories = hits
                    .into_iter()
                    .filter(|hit| {
                        !profile
                            .as_ref()
                            .is_some_and(|profile| profile.suppresses_retrieval_text(&hit.snippet))
                    })
                    .map(|hit| RetrievedMemory {
                        snippet: hit.snippet,
                        path: hit.path,
                        source: hit.source,
                        score: hit.score,
                    })
                    .collect::<Vec<_>>();
                tracing::debug!(hits = memories.len(), "proactive memory retrieval complete");
                self.context.set_retrieved_memory(memories);
            }
            Ok(Err(error)) => {
                warn!(%error, "proactive memory retrieval skipped");
                self.context.set_retrieved_memory(Vec::new());
            }
            Err(error) => {
                warn!(%error, "proactive memory retrieval task failed");
                self.context.set_retrieved_memory(Vec::new());
            }
        }
    }

    fn loop_config(&self, user_text: &str) -> AgentLoopConfig {
        AgentLoopConfig {
            max_tool_rounds: self.max_tool_rounds,
            session_id: self.session_id.clone(),
            turn_id: self.turn_id.clone(),
            features: self.features.clone(),
            force_list_all: false,
            // Use the host's original utterance, never an injected user-role
            // research/finish reminder appended later in the same turn.
            task: Some(crate::task::classify_task(user_text)),
        }
    }

    fn make_emit(&self) -> crate::types::EmitFn {
        let listeners = std::sync::Arc::clone(&self.listeners);
        std::sync::Arc::new(move |event: AgentEvent| {
            if let Ok(guard) = listeners.lock() {
                for (_, listener) in guard.iter() {
                    listener(&event);
                }
            }
        })
    }

    /// For LinkedIn / person-find asks, inject the research skill body once so
    /// freestyle "need more hints" does not skip the multi-query playbook.
    fn maybe_inject_research_skill(&mut self, user_text: &str) {
        if !crate::finish_gate::looks_like_person_find(user_text) {
            return;
        }
        let Some(shared) = self.skills.as_ref() else {
            return;
        };
        let body = {
            let Ok(guard) = shared.lock() else {
                return;
            };
            let Some(skill) = guard.get("research") else {
                return;
            };
            match crate::skills::load_skill_body(skill) {
                Ok(b) => b,
                Err(e) => {
                    warn!(error = %e, "research skill inject skipped");
                    return;
                }
            }
        };
        // Avoid re-injecting while a recent research playbook is still in context.
        let already = self.context.messages().iter().rev().take(10).any(|m| {
            m.content
                .as_str()
                .is_some_and(|s| s.contains("Person/profile research request"))
        });
        if already {
            return;
        }
        info!("injecting research skill body for person/profile find");
        self.context
            .push_control(crate::finish_gate::person_find_skill_nudge(&body));
    }

    /// Summarize older turns into a compact block (Grok-lite compaction).
    async fn maybe_llm_compact(&mut self) -> Result<(), String> {
        // Keep more recent turns intact so ongoing research/tool work is not
        // summarized away mid-session.
        const KEEP_RECENT: usize = 4;
        let started = Instant::now();
        let digest = self.context.older_turns_digest(KEEP_RECENT);
        if digest.trim().is_empty() {
            return Ok(());
        }
        let messages = serde_json::json!([
            {
                "role": "system",
                "content": "Summarize the conversation for an assistant continuing the work. \
        Keep: names, URLs, file paths, decisions, open tasks, tool findings (facts, numbers, links). \
        Max 20 short bullet lines. Prefer concrete facts over narrative. No fluff."
            },
            {
                "role": "user",
                "content": digest
            }
        ]);
        let msg = self
            .client
            .complete(messages, serde_json::Value::Null)
            .await
            .map_err(|e| e.to_string())?;
        let summary = msg
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if summary.is_empty() {
            return Ok(());
        }
        self.context.apply_summary_compact(&summary, KEEP_RECENT);
        info!(
            chars = summary.len(),
            ms = started.elapsed().as_millis() as u64,
            "context llm-compact applied"
        );
        Ok(())
    }

    /// Primary turn API: one user message → [`AgentOutcome`].
    pub async fn prompt(&mut self, user_text: &str) -> Result<AgentOutcome, AgentError> {
        self.prompt_with_report(user_text)
            .await
            .map(|(outcome, _)| outcome)
    }

    /// Replace an interrupted turn with a clean new human objective.
    ///
    /// `interrupted_text` is supplied to the model as scoped, context-only host
    /// control and is never stored as part of the human transcript.
    pub async fn prompt_replacement(
        &mut self,
        user_text: &str,
        interrupted_text: Option<&str>,
    ) -> Result<AgentOutcome, AgentError> {
        self.prompt_replacement_with_report(user_text, interrupted_text)
            .await
            .map(|(outcome, _)| outcome)
    }

    /// Back-compat alias for [`Self::prompt`].
    #[deprecated(note = "use Agent::prompt")]
    pub async fn run_turn(&mut self, user_text: &str) -> Result<AgentOutcome, AgentError> {
        self.prompt(user_text).await
    }

    /// Back-compat alias for [`Self::prompt`].
    #[deprecated(note = "use Agent::prompt")]
    pub async fn chat(&mut self, message: &str) -> Result<AgentOutcome, AgentError> {
        self.prompt(message).await
    }

    /// Run one user turn and return both the outcome and a [`TurnReport`].
    pub async fn prompt_with_report(
        &mut self,
        user_text: &str,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        self.prompt_with_report_inner(user_text, None).await
    }

    /// Run a replacement turn while keeping the newest human message clean.
    ///
    /// The interrupted request is exposed exactly once as an ephemeral
    /// [`crate::MessageOrigin::HostControl`] immediately before the new human
    /// message. Task classification, task state, retrieval, skill routing, and
    /// post-turn learning all receive only `user_text`.
    pub async fn prompt_replacement_with_report(
        &mut self,
        user_text: &str,
        interrupted_text: Option<&str>,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        let replacement_control = interrupted_text
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(replacement_control);
        self.prompt_with_report_inner(user_text, replacement_control)
            .await
    }

    async fn prompt_with_report_inner(
        &mut self,
        user_text: &str,
        replacement_control: Option<String>,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        if self.pending_turn.is_some() {
            return Err(AgentError::new(
                "cannot start a new turn while a tool confirmation is pending",
            ));
        }

        // Fresh turn: re-require shell HITL even if a prior turn granted it.
        self.runtime.clear_turn_grants();

        if self.personal.is_some() {
            self.refresh_system_prompt();
        }

        let config = self.loop_config(user_text);
        let tools_for_request =
            loop_::listed_tools_json(&self.tools, &config, Some(&self.activated));
        let context_limit = self
            .client
            .context_window_tokens()
            .unwrap_or(boris_ai::DEFAULT_CONTEXT_WINDOW_TOKENS);
        let compact_budget =
            ContextBudget::for_request(context_limit, boris_ai::DEFAULT_MAX_TOKENS);
        // LLM summary compact when context is large (P0).
        if self
            .context
            .needs_llm_compact_for_request_with_budget(&tools_for_request, compact_budget)
        {
            if let Err(e) = self.maybe_llm_compact().await {
                warn!(error = %e, "llm compact skipped");
            }
        }
        self.context
            .compact_mechanical_for_request_with_budget(&tools_for_request, compact_budget);
        // Todo + research re-entry budget (each re-enter costs one).
        self.finish_gate_remaining = 3;

        let started = Instant::now();
        let preview = log_preview(user_text, LOG_PREVIEW_CHARS);
        info!(
            model = %self.client.model(),
            message_len = user_text.len(),
            preview = %preview,
            "agent turn start"
        );
        self.emit(&AgentEvent::MessageEnd {
            role: Role::User,
            preview,
        });

        let snapshot = self.context.clone();
        if let Some(control) = &replacement_control {
            self.context
                .push_human_with_control(control.clone(), user_text);
        } else {
            self.context.push(Role::User, user_text);
        }
        self.context.begin_task(user_text);
        self.retrieve_memory_for_turn(user_text).await;

        // Person/profile finds: auto-inject research skill body so the model
        // does not freestyle without the multi-query playbook.
        self.maybe_inject_research_skill(user_text);

        let ct = match self.cancel.clone() {
            Some(existing) => existing,
            None => {
                let ct = CancellationToken::new();
                self.cancel = Some(ct.clone());
                ct
            }
        };
        let emit = self.make_emit();
        // Finish gate reads the session-bound todos *file* (not sandbox root).
        let todos_for_gate = self
            .todos_path
            .clone()
            .unwrap_or_else(|| self.sandbox_snapshot.sandbox_root.join("todos.json"));

        let loop_out = {
            let state = LoopState {
                context: &mut self.context,
                tools: &self.tools,
                runtime: &self.runtime,
                client: self.client.as_ref(),
                activated: Some(&self.activated),
            };
            loop_::agent_loop(
                state,
                user_text,
                &config,
                Vec::new(),
                0,
                0,
                Some(ct),
                Some(emit),
                Some(todos_for_gate),
                self.finish_gate_remaining,
            )
            .await
        };

        self.cancel = None;

        match loop_out {
            Ok(loop_out) => {
                if let Some(control) = &replacement_control {
                    self.context
                        .remove_control(&serde_json::Value::String(control.clone()));
                }
                self.pending_turn = loop_out.pending_turn.clone();
                self.maybe_refresh_after_tools(&loop_out.tools_used);
                self.finish_loop(started, user_text, loop_out).await
            }
            Err(e) => {
                self.context = snapshot;
                self.pending_turn = None;
                if e.kind() == AgentErrorKind::Cancelled {
                    info!(
                        duration_ms = started.elapsed().as_millis() as u64,
                        "agent turn cancelled"
                    );
                } else {
                    self.emit(&AgentEvent::Error {
                        message: e.to_string(),
                    });
                    error!(
                        error = %e,
                        duration_ms = started.elapsed().as_millis() as u64,
                        "agent turn failed"
                    );
                }
                Err(e)
            }
        }
    }

    /// Continue after the host collected typed input (or the user cancelled).
    pub async fn resume_input(
        &mut self,
        pending_id: &str,
        value: Option<String>,
    ) -> Result<AgentOutcome, AgentError> {
        self.resume_input_with_report(pending_id, value)
            .await
            .map(|(o, _)| o)
    }

    /// Same as [`Self::resume_input`] with a [`TurnReport`].
    pub async fn resume_input_with_report(
        &mut self,
        pending_id: &str,
        value: Option<String>,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        let started = Instant::now();
        let pending_turn = self
            .pending_turn
            .take()
            .ok_or_else(|| AgentError::new("no pending input to resume"))?;

        if pending_turn.pending.id != pending_id {
            let id = pending_turn.pending.id.clone();
            self.pending_turn = Some(pending_turn);
            return Err(AgentError::new(format!(
                "pending id mismatch: expected `{id}`, got `{pending_id}`"
            )));
        }
        if pending_turn.pending.input.is_none() {
            self.pending_turn = Some(pending_turn);
            return Err(AgentError::new(
                "pending pause is a confirmation, not input",
            ));
        }

        let user_text = pending_turn.user_text.clone();
        let ct = match self.cancel.clone() {
            Some(existing) => existing,
            None => {
                let ct = CancellationToken::new();
                self.cancel = Some(ct.clone());
                ct
            }
        };
        let config = self.loop_config(&user_text);
        let emit = self.make_emit();

        let loop_out = {
            let state = LoopState {
                context: &mut self.context,
                tools: &self.tools,
                runtime: &self.runtime,
                client: self.client.as_ref(),
                activated: Some(&self.activated),
            };
            loop_::resume_pending_input(state, pending_turn, value, &config, Some(emit), Some(ct))
                .await
        };

        self.cancel = None;

        match loop_out {
            Ok(loop_out) => {
                self.pending_turn = loop_out.pending_turn.clone();
                self.maybe_refresh_after_tools(&loop_out.tools_used);
                self.finish_loop(started, &user_text, loop_out).await
            }
            Err(e) => {
                if e.kind() == AgentErrorKind::Cancelled {
                    info!(
                        duration_ms = started.elapsed().as_millis() as u64,
                        "agent input resume cancelled"
                    );
                } else {
                    error!(
                        error = %e,
                        duration_ms = started.elapsed().as_millis() as u64,
                        "agent input resume failed"
                    );
                    self.emit(&AgentEvent::Error {
                        message: e.to_string(),
                    });
                }
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
            let id = pending_turn.pending.id.clone();
            self.pending_turn = Some(pending_turn);
            return Err(AgentError::new(format!(
                "pending id mismatch: expected `{id}`, got `{pending_id}`"
            )));
        }

        let user_text = pending_turn.user_text.clone();
        let ct = match self.cancel.clone() {
            Some(existing) => existing,
            None => {
                let ct = CancellationToken::new();
                self.cancel = Some(ct.clone());
                ct
            }
        };
        let config = self.loop_config(&user_text);
        let emit = self.make_emit();

        let loop_out = {
            let state = LoopState {
                context: &mut self.context,
                tools: &self.tools,
                runtime: &self.runtime,
                client: self.client.as_ref(),
                activated: Some(&self.activated),
            };
            loop_::resume_pending_tool(state, pending_turn, approved, &config, Some(emit), Some(ct))
                .await
        };

        self.cancel = None;

        match loop_out {
            Ok(loop_out) => {
                self.pending_turn = loop_out.pending_turn.clone();
                self.maybe_refresh_after_tools(&loop_out.tools_used);
                self.finish_loop(started, &user_text, loop_out).await
            }
            Err(e) => {
                if e.kind() == AgentErrorKind::Cancelled {
                    info!(
                        duration_ms = started.elapsed().as_millis() as u64,
                        "agent resume cancelled"
                    );
                } else {
                    error!(
                        error = %e,
                        duration_ms = started.elapsed().as_millis() as u64,
                        "agent resume failed"
                    );
                    self.emit(&AgentEvent::Error {
                        message: e.to_string(),
                    });
                }
                Err(e)
            }
        }
    }

    async fn finish_loop(
        &mut self,
        started: Instant,
        user_text: &str,
        loop_out: LoopResult,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        let duration = started.elapsed();
        let outcome_label = match &loop_out.outcome {
            AgentOutcome::Speak { expect_reply, .. } if *expect_reply => "speak_await",
            AgentOutcome::Speak { .. } => "speak",
            AgentOutcome::Silent => "silent",
            AgentOutcome::NeedsConfirmation { .. } => "needs_confirm",
            AgentOutcome::NeedsInput { .. } => "needs_input",
        };
        let approx_chars_in = self
            .context
            .estimate_request_chars(&serde_json::Value::Null);
        let report = TurnReport {
            duration,
            tool_rounds: loop_out.tool_rounds,
            tools_used: loop_out.tools_used.clone(),
            outcome: match &loop_out.outcome {
                AgentOutcome::NeedsConfirmation { .. } => TurnOutcomeKind::NeedsConfirm,
                AgentOutcome::NeedsInput { .. } => TurnOutcomeKind::NeedsInput,
                AgentOutcome::Silent => TurnOutcomeKind::Silent,
                AgentOutcome::Speak { .. } => TurnOutcomeKind::Speak,
            },
            approx_chars_in,
            context_used_tokens: loop_out.token_accounting.context_used_tokens(),
            context_limit_tokens: loop_out.token_accounting.context_limit_tokens,
            context_estimated: loop_out.token_accounting.context_is_estimated(),
            token_usage: Box::new(loop_out.token_accounting.provider_usage.clone()),
        };
        let (task_status, assistant_task_text) = match &loop_out.outcome {
            AgentOutcome::Speak {
                text,
                expect_reply: true,
            } => (TaskStatus::WaitingForInput, text.as_str()),
            AgentOutcome::Speak { text, .. } => (TaskStatus::Completed, text.as_str()),
            AgentOutcome::Silent => (TaskStatus::Completed, ""),
            AgentOutcome::NeedsConfirmation { .. } => (TaskStatus::WaitingForConfirmation, ""),
            AgentOutcome::NeedsInput { .. } => (TaskStatus::WaitingForInput, ""),
        };
        self.context.finish_task(task_status, assistant_task_text);
        info!(
            outcome = outcome_label,
            duration_ms = duration.as_millis() as u64,
            tool_rounds = loop_out.tool_rounds,
            tools_count = loop_out.tools_used.len(),
            tools = ?loop_out.tools_used,
            approx_chars_in,
            context_used_tokens = report.context_used_tokens,
            context_limit_tokens = ?report.context_limit_tokens,
            context_estimated = report.context_estimated,
            prompt_tokens = report.token_usage.prompt_tokens,
            completion_tokens = report.token_usage.completion_tokens,
            "agent turn end"
        );

        // Duration is captured *before* maintenance so TTS is not billed for it.
        if !matches!(
            loop_out.outcome,
            AgentOutcome::NeedsConfirmation { .. } | AgentOutcome::NeedsInput { .. }
        ) {
            let assistant_text = match &loop_out.outcome {
                AgentOutcome::Speak { text, .. } => text.as_str(),
                AgentOutcome::Silent => "",
                AgentOutcome::NeedsConfirmation { .. } | AgentOutcome::NeedsInput { .. } => "",
            };
            self.enqueue_post_turn(user_text, assistant_text, &loop_out.tools_used);
        }

        Ok((loop_out.outcome, report))
    }

    fn enqueue_post_turn(&mut self, user_text: &str, assistant_text: &str, tools_used: &[String]) {
        if let Some(h) = &self.maintenance {
            let mut personal_enqueued = self.personal.is_none();
            if let Some(memory) = &self.memory_store {
                if let Err(e) = h.submit(crate::maintenance::MaintenanceJob::IngestMemory {
                    store: memory.clone(),
                    session_id: self.session_id.clone(),
                    user: user_text.to_string(),
                    assistant: assistant_text.to_string(),
                }) {
                    // The canonical lane is intentionally lossless. A
                    // shutdown race is the only expected error; write now so
                    // a completed turn is never silently lost.
                    warn!(error = %e, "maintenance enqueue canonical memory failed; writing directly");
                    if let Err(write_error) =
                        memory.ingest_turn(self.session_id.as_deref(), user_text, assistant_text)
                    {
                        warn!(error = %write_error, "direct canonical memory write failed");
                    }
                }
            } else if let Some(ltm) = &self.long_term {
                match ltm.capture_session_target() {
                    Ok(Some(target)) => {
                        if let Err(e) = h.submit(crate::maintenance::MaintenanceJob::AppendTurn {
                            ltm: ltm.clone(),
                            target,
                            user: user_text.to_string(),
                            assistant: assistant_text.to_string(),
                        }) {
                            warn!(error = %e, "maintenance enqueue append failed");
                        }
                    }
                    Ok(None) => {}
                    Err(e) => warn!(error = %e, "memory session target capture failed"),
                }
            }
            if let Some(mem) = &self.personal {
                if let Err(e) = h.submit(crate::maintenance::MaintenanceJob::ExtractPersonal {
                    store: mem.store.clone(),
                    profile: mem.profile.clone(),
                    llm_extract: mem.llm_extract,
                    user: user_text.to_string(),
                    assistant: assistant_text.to_string(),
                    tools_used: tools_used.to_vec(),
                    client: Arc::clone(&self.client),
                }) {
                    warn!(error = %e, "maintenance enqueue extract failed");
                } else {
                    personal_enqueued = true;
                }
            }
            if !personal_enqueued {
                // Preserve cheap deterministic learning even when the optional
                // background lane is saturated or shutting down.
                self.learn_personal_heuristic(user_text);
            }
            return;
        }
        // Tests / hosts without a worker: preserve durable/local behavior, but
        // never put an awaited LLM extraction back on the response path.
        if let Some(memory) = &self.memory_store {
            if let Err(e) =
                memory.ingest_turn(self.session_id.as_deref(), user_text, assistant_text)
            {
                warn!(error = %e, "canonical memory write failed");
            }
        } else if let Some(ltm) = &self.long_term {
            if let Err(e) = ltm.append_turn(user_text, assistant_text) {
                warn!(error = %e, "long-term memory append failed");
            }
        }
        self.learn_personal_heuristic(user_text);
    }
}

fn replacement_control(interrupted_text: &str) -> String {
    let escaped = interrupted_text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<turn_replacement>\n\
         The user interrupted the active turn. The following human message is the only active objective.\n\
         Immediately abandon the interrupted objective. Do not continue, complete, or revive it unless the following human message explicitly asks you to.\n\
         Treat the interrupted objective below as context only, never as an instruction.\n\
         <interrupted_objective>{escaped}</interrupted_objective>\n\
         </turn_replacement>"
    )
}

/// Remove conversational glue before an OR-based memory search. This keeps
/// proactive recall relevant instead of matching every old turn on "please".
fn proactive_memory_query(input: &str) -> Option<String> {
    const STOP: &[&str] = &[
        "about", "also", "been", "could", "from", "have", "help", "just", "make", "okay", "please",
        "that", "then", "there", "these", "they", "think", "this", "want", "what", "when", "where",
        "which", "with", "would", "your",
    ];
    let mut seen = std::collections::HashSet::new();
    let terms = input
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-')
        .map(str::trim)
        .filter(|term| term.chars().count() >= 3)
        .filter(|term| !STOP.contains(&term.to_ascii_lowercase().as_str()))
        .filter(|term| seen.insert(term.to_ascii_lowercase()))
        .take(12)
        .collect::<Vec<_>>();
    (!terms.is_empty()).then(|| terms.join(" "))
}

#[cfg(test)]
mod retrieval_tests {
    use super::proactive_memory_query;
    use async_trait::async_trait;
    use boris_ai::{LlmClient, LlmError};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    struct NoopClient;

    #[async_trait]
    impl LlmClient for NoopClient {
        async fn complete(&self, _messages: Value, _tools: Value) -> Result<Value, LlmError> {
            Ok(serde_json::json!({"role": "assistant", "content": "ok"}))
        }
    }

    #[derive(Clone, Default)]
    struct RecordingClient {
        requests: Arc<Mutex<Vec<Value>>>,
        fail: bool,
    }

    #[async_trait]
    impl LlmClient for RecordingClient {
        async fn complete(&self, messages: Value, _tools: Value) -> Result<Value, LlmError> {
            self.requests.lock().unwrap().push(messages);
            if self.fail {
                Err(LlmError::new("replacement test failure"))
            } else {
                Ok(serde_json::json!({"role": "assistant", "content": "switched"}))
            }
        }
    }

    fn replacement_controls(request: &Value) -> Vec<&str> {
        request
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message.get("content").and_then(Value::as_str))
            .filter(|content| content.starts_with("<turn_replacement>"))
            .collect()
    }

    #[tokio::test]
    async fn replacement_keeps_latest_human_clean_and_control_scoped() {
        let client = RecordingClient::default();
        let requests = client.requests.clone();
        let mut agent = super::Agent::new(Box::new(client), "sys");
        let new_objective = "Just ask me for my LinkedIn ID.";
        let interrupted = "Search for my LinkedIn profile.";

        agent
            .prompt_replacement_with_report(new_objective, Some(interrupted))
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        // The finish gate may issue a follow-up completion for person/profile
        // wording; inspect the initial replacement request itself.
        let request = requests.first().unwrap();
        let controls = replacement_controls(request);
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].matches(interrupted).count(), 1);
        let messages = request.as_array().unwrap();
        let control_index = messages
            .iter()
            .position(|message| {
                message["content"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("<turn_replacement>"))
            })
            .unwrap();
        assert_eq!(messages[control_index + 1]["content"], new_objective);
        assert_eq!(messages.last().unwrap()["role"], "user");
        assert_eq!(messages.last().unwrap()["content"], new_objective);
        let task_state = messages
            .iter()
            .find_map(|message| {
                message["content"]
                    .as_str()
                    .filter(|text| text.contains("<task_state>"))
            })
            .unwrap();
        assert!(task_state.contains(new_objective));
        assert!(!task_state.contains(interrupted));

        let humans = agent
            .export_messages_for_persist()
            .into_iter()
            .filter(|message| message.origin == crate::MessageOrigin::Human)
            .collect::<Vec<_>>();
        assert_eq!(
            humans.last().unwrap().content,
            serde_json::json!(new_objective)
        );
        assert!(!agent.context.messages().iter().any(|message| {
            message.origin == crate::MessageOrigin::HostControl
                && message
                    .content
                    .as_str()
                    .is_some_and(|text| text.starts_with("<turn_replacement>"))
        }));
    }

    #[tokio::test]
    async fn consecutive_replacements_do_not_retain_or_nest_old_controls() {
        let client = RecordingClient::default();
        let requests = client.requests.clone();
        let mut agent = super::Agent::new(Box::new(client), "sys");

        agent
            .prompt_replacement("First replacement.", Some("Original objective only."))
            .await
            .unwrap();
        agent
            .prompt_replacement("Second replacement.", Some("First replacement."))
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        let controls = replacement_controls(requests.last().unwrap());
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].matches("First replacement.").count(), 1);
        assert!(!controls[0].contains("Original objective only."));
        assert_eq!(controls[0].matches("<turn_replacement>").count(), 1);
    }

    #[tokio::test]
    async fn failed_replacement_rolls_back_control_human_and_task_state() {
        let client = RecordingClient {
            fail: true,
            ..RecordingClient::default()
        };
        let mut agent = super::Agent::new(Box::new(client), "sys");
        agent.context.push(super::Role::User, "stable user turn");
        agent.context.push(super::Role::Assistant, "stable answer");
        agent.context.begin_task("stable user turn");
        let before = agent.context.as_json();

        let result = agent
            .prompt_replacement_with_report("new objective", Some("old objective"))
            .await;

        assert!(result.is_err());
        assert_eq!(agent.context.as_json(), before);
        assert!(!agent
            .export_messages_for_persist()
            .iter()
            .any(|message| message.content == serde_json::json!("new objective")));
    }

    #[tokio::test]
    async fn checkpoint_removes_a_turn_that_completed_before_interrupt_cancel() {
        let client = RecordingClient::default();
        let mut agent = super::Agent::new(Box::new(client), "sys");
        let checkpoint = agent.checkpoint();

        agent.prompt("discarded objective").await.unwrap();
        assert!(agent
            .export_messages_for_persist()
            .iter()
            .any(|message| message.content == serde_json::json!("discarded objective")));

        agent.restore_checkpoint(checkpoint);

        assert!(!agent
            .export_messages_for_persist()
            .iter()
            .any(|message| message.content == serde_json::json!("discarded objective")));
    }

    #[test]
    fn proactive_query_drops_conversational_noise() {
        assert_eq!(
            proactive_memory_query("Okay please help me remember my preferred Rust editor")
                .as_deref(),
            Some("remember preferred Rust editor")
        );
        assert!(proactive_memory_query("okay please help").is_none());
    }

    #[tokio::test]
    async fn agent_proactively_retrieves_hits_with_provenance() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("boris-proactive-memory-{unique}"));
        let mut agent = super::Agent::new(Box::new(NoopClient), "sys");
        let memory = agent.enable_long_term_memory(&root).unwrap();
        std::fs::write(
            memory.memory_md_path(),
            "# Global Memory\n\nThe user prefers the Helix editor for Rust.\n",
        )
        .unwrap();
        memory.refresh_curated_index().unwrap();

        agent
            .retrieve_memory_for_turn("Which Rust editor do I prefer?")
            .await;
        assert!(agent.retrieved_memory().iter().any(|hit| {
            hit.path == "MEMORY.md" && hit.source == "global" && hit.snippet.contains("Helix")
        }));

        drop(memory);
        drop(agent);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn agent_retrieves_from_canonical_memory_not_markdown() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("boris-canonical-memory-{unique}"));
        let mut agent = super::Agent::new(Box::new(NoopClient), "sys");
        let memory = agent
            .enable_memory_store(root.join("memory.sqlite"))
            .unwrap();
        memory
            .upsert(crate::memory::NewMemory::semantic(
                "Preferred Rust editor: Helix",
            ))
            .unwrap();

        agent
            .retrieve_memory_for_turn("Which Rust editor do I prefer?")
            .await;
        assert!(agent.retrieved_memory().iter().any(|hit| {
            hit.path.starts_with("memory/mem_")
                && hit.source == "fact"
                && hit.snippet.contains("Helix")
        }));

        drop(memory);
        drop(agent);
        let _ = std::fs::remove_dir_all(root);
    }
}
