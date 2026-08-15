//! Turn execution: prompt, resume HITL, finish/report.

use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::context::Role;
use crate::error::AgentError;
use crate::loop_::{self, LoopState};
use crate::observe::{TurnOutcomeKind, TurnReport};
use crate::outcome::AgentOutcome;
use crate::types::{AgentEvent, AgentLoopConfig, LoopResult};

use super::{log_preview, Agent, LOG_PREVIEW_CHARS};

impl Agent {
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
        let already = self.context.messages.iter().rev().take(10).any(|m| {
            m.content
                .as_str()
                .is_some_and(|s| s.contains("Person/profile research request"))
        });
        if already {
            return;
        }
        info!("injecting research skill body for person/profile find");
        self.context.push(
            Role::User,
            crate::finish_gate::person_find_skill_nudge(&body),
        );
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
        // LLM summary compact when context is large (P0).
        if self
            .context
            .needs_llm_compact_for_request(&tools_for_request)
        {
            if let Err(e) = self.maybe_llm_compact().await {
                warn!(error = %e, "llm compact skipped");
            }
        }
        self.context
            .compact_mechanical_for_request(&tools_for_request);
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

        let snapshot = self.context.messages.clone();
        self.context.push(Role::User, user_text);

        // Person/profile finds: auto-inject research skill body so the model
        // does not freestyle without the multi-query playbook.
        self.maybe_inject_research_skill(user_text);

        let ct = CancellationToken::new();
        self.cancel = Some(ct.clone());
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
                self.pending_turn = loop_out.pending_turn.clone();
                self.maybe_refresh_after_tools(&loop_out.tools_used);
                self.finish_loop(started, user_text, loop_out).await
            }
            Err(e) => {
                self.context.messages = snapshot;
                self.pending_turn = None;
                self.emit(&AgentEvent::Error {
                    message: e.to_string(),
                });
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
            let id = pending_turn.pending.id.clone();
            self.pending_turn = Some(pending_turn);
            return Err(AgentError::new(format!(
                "pending id mismatch: expected `{id}`, got `{pending_id}`"
            )));
        }

        let user_text = pending_turn.user_text.clone();
        let ct = CancellationToken::new();
        self.cancel = Some(ct.clone());
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
                error!(
                    error = %e,
                    duration_ms = started.elapsed().as_millis() as u64,
                    "agent resume failed"
                );
                self.emit(&AgentEvent::Error {
                    message: e.to_string(),
                });
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
                AgentOutcome::Silent => TurnOutcomeKind::Silent,
                AgentOutcome::Speak { .. } => TurnOutcomeKind::Speak,
            },
            approx_chars_in,
        };
        info!(
            outcome = outcome_label,
            duration_ms = duration.as_millis() as u64,
            tool_rounds = loop_out.tool_rounds,
            tools_count = loop_out.tools_used.len(),
            tools = ?loop_out.tools_used,
            approx_chars_in,
            "agent turn end"
        );

        // Duration is captured *before* maintenance so TTS is not billed for it.
        if !matches!(loop_out.outcome, AgentOutcome::NeedsConfirmation { .. }) {
            let assistant_text = match &loop_out.outcome {
                AgentOutcome::Speak { text, .. } => text.as_str(),
                AgentOutcome::Silent => "",
                AgentOutcome::NeedsConfirmation { .. } => "",
            };
            self.enqueue_post_turn(user_text, assistant_text, &loop_out.tools_used);
        }

        Ok((loop_out.outcome, report))
    }

    fn enqueue_post_turn(&mut self, user_text: &str, assistant_text: &str, tools_used: &[String]) {
        if let Some(h) = &self.maintenance {
            let mut personal_enqueued = self.personal.is_none();
            if let Some(ltm) = &self.long_term {
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
        if let Some(ltm) = &self.long_term {
            if let Err(e) = ltm.append_turn(user_text, assistant_text) {
                warn!(error = %e, "long-term memory append failed");
            }
        }
        self.learn_personal_heuristic(user_text);
    }
}
