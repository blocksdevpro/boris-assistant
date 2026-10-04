//! Pure ReAct agent loop (tau-inspired).
//!
//! No personal memory, session I/O, or host side effects — only LLM complete +
//! [`ToolRuntime`] mediation. The [`crate::Agent`] facade owns state and learning.
//!
//! # Module layout
//!
//! - [`message_parse`] — tool-call / reply text parsing from LLM JSON
//! - [`helpers`] — tool lookup, listing, invocation setup, observation writes
//! - [`tool_batch`] — sequential / parallel / wave-scheduling batch execution
//! - [`round`] — per-round LLM complete, tool-call gating, finish-gate checks
//! - [`finish`] — terminal emit helpers (speech / HITL pause)
//!
//! Tool batches that may need HITL run sequentially so remaining sibling calls
//! can be paused. Auto-allow batches (no confirmation needed) run in parallel
//! via chunked `join_all` (or read/write waves), preserving original order in
//! context and never exceeding `max_parallel_tools` concurrent invokes.

mod finish;
mod helpers;
mod message_parse;
mod round;
mod tool_batch;

use std::path::PathBuf;
use std::time::Instant;

use boris_ai::LlmClient;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::context::{Context, Role};
use crate::error::AgentError;
use crate::finish_gate::FinishGateBudget;
use crate::outcome::AgentOutcome;
use crate::runtime::{
    ActivationSet, InvokeOptions, InvokeResult, PendingTurn, RawToolCall, ToolRuntime,
};
use crate::tool::Tool;
use crate::types::{AgentEvent, AgentLoopConfig, EmitFn, LoopResult, TokenAccounting};

use finish::{finish_paused, finish_with_speech, noop_emit};
use helpers::{
    build_tool_invocation, find_tool, find_tool_opt, log_tool_done, observation_looks_ok,
    push_tool_result_messages, unknown_tool_observation, useful_research_observation_count,
};
use message_parse::{extract_reply_text, extract_tool_note, parse_raw_tool_calls};
use round::{
    cancelled, complete_round, ensure_spoken_reply_at_cap, should_reenter_finish_gate,
    tool_calls_if_runnable, NUDGE_NEAR_TOOL_CAP,
};
use tool_batch::{process_tool_calls, ToolBatchResult};

/// Mutable state the loop may write back (context + tools + runtime).
pub struct LoopState<'a> {
    pub context: &'a mut Context,
    pub tools: &'a [std::sync::Arc<dyn Tool>],
    pub runtime: &'a ToolRuntime,
    pub client: &'a dyn LlmClient,
    /// Session activation set (tool_search). Optional when progressive is off.
    pub activated: Option<&'a ActivationSet>,
}

/// Exact progressive tool payload used for a request and its context budget.
///
/// Returns `(payload, pruned_count)`; `pruned_count` is the number of tool
/// definitions dropped to fit `MAX_TOOL_SCHEMA_CHARS`. `complete_round`
/// logs a `warn!` when non-zero (`tool_search` is retained as the hatch).
pub(crate) fn listed_tools_json(
    tools: &[std::sync::Arc<dyn Tool>],
    config: &AgentLoopConfig,
    activated: Option<&ActivationSet>,
) -> (serde_json::Value, usize) {
    let list_ctx = helpers::build_list_ctx(config, activated);
    helpers::tools_json_for_llm(tools, &list_ctx)
}

/// Run the ReAct loop until a final reply, HITL pause, cancel, or error.
///
/// `user_text` is only used for pending-turn bookkeeping (post-turn learn).
/// Context must already contain the user message (and any prior history).
///
/// # Tool-round accounting (B13)
///
/// The loop iterates `0..=max_tool_rounds`, so one turn issues up to
/// `max_tool_rounds + 1` LLM completions with tools. When the cap round comes
/// back empty, [`round::ensure_spoken_reply_at_cap`] issues at most one extra
/// no-tools forced-speak completion. Total worst case is therefore
/// `max + 1` tool-capable completions plus one forced-speak. Bounds are
/// intentionally unchanged (tests depend on them); this documents the actual
/// budget so hosts can size `max_tool_rounds` correctly.
///
/// Finish gates use split budgets (see [`FinishGateBudget`]): `markup`
/// (default 2) for tool-markup re-prompts and `gate` (legacy
/// `finish_gate_left`, default 3) for research/todo re-entries.
#[allow(clippy::too_many_arguments)]
pub async fn agent_loop(
    state: LoopState<'_>,
    user_text: &str,
    config: &AgentLoopConfig,
    tools_used: Vec<String>,
    tool_rounds: u32,
    confirms_used: u32,
    cancel: Option<CancellationToken>,
    emit: Option<EmitFn>,
    todos_file: Option<std::path::PathBuf>,
    finish_gate_left: u32,
) -> Result<LoopResult, AgentError> {
    let budget = FinishGateBudget::from_legacy(finish_gate_left);
    agent_loop_with_budget(
        state,
        user_text,
        config,
        tools_used,
        tool_rounds,
        confirms_used,
        cancel,
        emit,
        todos_file,
        budget,
        TokenAccounting::default(),
    )
    .await
}

/// Budget-explicit ReAct loop (split markup vs research/todo gates).
///
/// `budget.markup` guards markup-only re-prompts; `budget.gate` guards
/// research/todo re-entries. `initial_accounting` seeds token accounting
/// (fresh turns pass `default()`; resume paths restore the pre-pause
/// accounting so HITL never discards cost).
#[allow(clippy::too_many_arguments)]
pub async fn agent_loop_with_budget(
    mut state: LoopState<'_>,
    user_text: &str,
    config: &AgentLoopConfig,
    tools_used: Vec<String>,
    tool_rounds: u32,
    confirms_used: u32,
    cancel: Option<CancellationToken>,
    emit: Option<EmitFn>,
    todos_file: Option<PathBuf>,
    budget: FinishGateBudget,
    initial_accounting: TokenAccounting,
) -> Result<LoopResult, AgentError> {
    let emit = emit.unwrap_or_else(noop_emit);
    emit(AgentEvent::AgentStart);

    let mut tools_used = tools_used;
    let mut tool_rounds = tool_rounds;
    let mut confirms_used = confirms_used;
    let mut token_accounting = initial_accounting;
    let max_rounds = config.max_tool_rounds as usize;
    // Full path to todos.json (session-bound or sandbox fallback).
    let todos_file = todos_file
        .unwrap_or_else(|| crate::finish_gate::default_sandbox_guess().join("todos.json"));
    // Split finish budgets: markup re-prompts vs research/todo gates.
    let mut markup_left = budget.markup;
    let mut gate_left = budget.gate;
    // Last non-empty spoken line this turn (finish-gate re-entry must not discard it).
    let mut last_speakable: Option<String> = None;
    // B11: consecutive unknown-tool-only rounds (no successful tools).
    let mut consecutive_unknown_rounds: u32 = 0;

    for round in 0..=max_rounds {
        if cancelled(&cancel) {
            emit(AgentEvent::Error {
                message: "cancelled".into(),
            });
            return Err(AgentError::cancelled("agent loop cancelled"));
        }

        emit(AgentEvent::TurnStart {
            round: round as u32,
        });

        // On the final allowed round, withhold tools so the model must speak.
        let at_cap = round >= max_rounds;
        let response = complete_round(
            &mut state,
            user_text,
            config,
            at_cap,
            &emit,
            &cancel,
            &mut token_accounting,
        )
        .await?;

        if let Some(batch) = tool_calls_if_runnable(&response, at_cap) {
            // One round before cap: run tools, then inject a finish nudge and
            // continue so the next iteration (at_cap) produces a spoken reply.
            let force_finish_next = round + 1 >= max_rounds;

            tool_rounds += 1;
            state.context.push(Role::Assistant, response.clone());
            emit(AgentEvent::MessageEnd {
                role: Role::Assistant,
                preview: format!("{} tool call(s)", batch.len()),
            });
            if let Some(text) = extract_tool_note(&response) {
                emit(AgentEvent::ToolNote { text });
            }

            let raw_calls = parse_raw_tool_calls(batch);
            // B11: detect unknown-only batches before execution for the breaker.
            let all_unknown = !raw_calls.is_empty()
                && raw_calls
                    .iter()
                    .all(|c| find_tool_opt(state.tools, &c.name).is_none());

            let activation_start = tools_used.len();
            let batch_result = process_tool_calls(
                &mut LoopState {
                    context: state.context,
                    tools: state.tools,
                    runtime: state.runtime,
                    client: state.client,
                    activated: state.activated,
                },
                raw_calls,
                &mut tools_used,
                tool_rounds,
                &mut confirms_used,
                user_text,
                config,
                &emit,
                cancel.clone(),
            )
            .await?;
            if let Some(activated) = state.activated {
                crate::runtime::activate_tools(
                    activated,
                    tools_used[activation_start..]
                        .iter()
                        .filter(|name| {
                            name.as_str() != "tool_search"
                                && !crate::runtime::is_core_name(name, &config.features)
                                && state.tools.iter().any(|tool| tool.name() == name.as_str())
                        })
                        .cloned(),
                );
            }

            match batch_result {
                ToolBatchResult::Continue => {
                    // B11 breaker: track consecutive unknown-only rounds.
                    if all_unknown {
                        consecutive_unknown_rounds = consecutive_unknown_rounds.saturating_add(1);
                        if consecutive_unknown_rounds >= 2 {
                            let available: Vec<&str> =
                                state.tools.iter().map(|t| t.name()).take(20).collect();
                            let has_search = state.tools.iter().any(|t| t.name() == "tool_search");
                            let hint = if has_search {
                                " Hint: use tool_search to discover the right tool."
                            } else {
                                ""
                            };
                            tracing::warn!(
                                rounds = consecutive_unknown_rounds,
                                "unknown-tool breaker: injecting available tools"
                            );
                            state.context.push_control(format!(
                                "<system-reminder>\n\
                                 You requested tools that are not available in this session. \
                                 Available tools: {}.{} \
                                 Continue with a registered tool or finish without it.\n\
                                 </system-reminder>",
                                if available.is_empty() {
                                    "(none)".to_string()
                                } else {
                                    available.join(", ")
                                },
                                hint
                            ));
                            consecutive_unknown_rounds = 0;
                        }
                    } else {
                        // Any real tool execution resets the unknown streak.
                        // (Unknown soft-fails never count as success.)
                        consecutive_unknown_rounds = 0;
                    }
                    if force_finish_next {
                        state.context.push_control(json!(NUDGE_NEAR_TOOL_CAP));
                    }
                    emit(AgentEvent::TurnEnd {
                        round: round as u32,
                    });
                    continue;
                }
                ToolBatchResult::Paused {
                    outcome,
                    mut pending_turn,
                } => {
                    // B2: preserve accounting/gates/todos across HITL.
                    pending_turn.todos_file = Some(todos_file.clone());
                    pending_turn.markup_left = markup_left;
                    pending_turn.gate_left = gate_left;
                    pending_turn.token_accounting = token_accounting.clone();
                    return finish_paused(
                        &emit,
                        round as u32,
                        outcome,
                        tool_rounds,
                        tools_used,
                        pending_turn,
                        token_accounting,
                    );
                }
            }
        }

        // Content-only response (or tools withheld / ignored at cap).
        let mut reply = extract_reply_text(&response);

        // Never speak or persist tool XML / pseudo-tool text. Do not execute it.
        if crate::speech_sanitize::contains_tool_markup(&reply) {
            let cleaned = crate::speech_sanitize::strip_tool_markup(&reply);
            if cleaned.is_empty() && !at_cap && markup_left > 0 {
                markup_left = markup_left.saturating_sub(1);
                tracing::info!(
                    left = markup_left,
                    "tool protocol: markup-only speech rejected — re-enter"
                );
                // B5: attribute the rejection to the host, not the model.
                // A `Role::Assistant` push would pollute history/transcript
                // with fake assistant text; `push_control` keeps it as
                // `MessageOrigin::HostControl` (ephemeral, never persisted).
                // Wrapped in `<system-reminder>` so transcript re-import
                // (`infer_transcript_origin`) also classifies it as host.
                state.context.push_control(
                    "<system-reminder>\n[invalid tool format in speech; use API tool_calls only]\n</system-reminder>",
                );
                state
                    .context
                    .push_control(crate::speech_sanitize::TOOL_PROTOCOL_REMINDER);
                emit(AgentEvent::TurnEnd {
                    round: round as u32,
                });
                continue;
            }
            reply = cleaned;
        }

        reply = ensure_spoken_reply_at_cap(
            &mut state,
            user_text,
            config,
            at_cap,
            reply,
            &emit,
            &cancel,
            &mut token_accounting,
        )
        .await?;

        if !reply.is_empty() {
            last_speakable = Some(reply.clone());
            // B11: valid speech breaks the unknown-tool streak.
            consecutive_unknown_rounds = 0;
        }

        // Finish gate: research may re-enter with zero tools; open todos only
        // when this turn already used tools (stale todos must not silence casual
        // replies). Uses the split `gate_left` budget (markup has its own).
        let useful_research_results = useful_research_observation_count(state.context);
        if should_reenter_finish_gate(
            at_cap,
            gate_left,
            &reply,
            &tools_used,
            useful_research_results,
            &todos_file,
            user_text,
        ) {
            gate_left = gate_left.saturating_sub(1);
            let reminder = if crate::finish_gate::should_research_gate_with(
                user_text,
                &reply,
                &tools_used,
                useful_research_results,
            ) {
                tracing::info!(
                    left = gate_left,
                    "finish gate: weak research — continue tooling"
                );
                crate::finish_gate::research_gate_reminder()
            } else if crate::finish_gate::should_local_work_gate(user_text, &tools_used) {
                tracing::info!(
                    left = gate_left,
                    "finish gate: weak local work — continue tooling"
                );
                crate::finish_gate::local_work_gate_reminder()
            } else {
                let pending = crate::finish_gate::pending_todo_count(&todos_file);
                tracing::info!(
                    pending,
                    left = gate_left,
                    "finish gate: open todos — continue tooling"
                );
                crate::finish_gate::todo_gate_reminder(pending)
            };
            state.context.push_control(reminder);
            emit(AgentEvent::TurnEnd {
                round: round as u32,
            });
            continue;
        }

        // Prefer current reply; if finish-gate re-entry went silent, keep earlier speech.
        if reply.is_empty() {
            if let Some(prev) = last_speakable.clone() {
                reply = prev;
            }
        }

        return finish_with_speech(
            &emit,
            round as u32,
            reply,
            tool_rounds,
            tools_used,
            token_accounting,
        );
    }

    // Unreachable with 0..=max_rounds + return inside, but keep a soft landing.
    Ok(LoopResult {
        outcome: AgentOutcome::speak("I ran out of steps on that. Ask me to pick it up again."),
        tool_rounds,
        tools_used,
        pending_turn: None,
        token_accounting,
    })
}

/// Continue after the host collected (or cancelled) typed input, then remaining tools.
///
/// Restores the pre-pause todos path, finish budgets, and token accounting
/// (B2/B4). Post-resume tool batches that actually execute increment
/// `tool_rounds` (B3). Re-entry uses `gate=0` (no finish-gate after HITL)
/// with the preserved markup budget.
pub async fn resume_pending_input(
    mut state: LoopState<'_>,
    pending_turn: PendingTurn,
    value: Option<String>,
    config: &AgentLoopConfig,
    emit: Option<EmitFn>,
    cancel: Option<CancellationToken>,
) -> Result<LoopResult, AgentError> {
    let emit = emit.unwrap_or_else(noop_emit);
    let mut tools_used = pending_turn.tools_used;
    let activation_start = tools_used.len();
    let mut tool_rounds = pending_turn.tool_rounds;
    let mut confirms_used = pending_turn.confirms_used;
    let remaining = pending_turn.remaining_calls;
    let pending = pending_turn.pending;
    let user_text = pending_turn.user_text;
    // B2/B4: restore pre-pause state.
    let todos_file = pending_turn.todos_file;
    let markup_left = pending_turn.markup_left;
    let restored_accounting = pending_turn.token_accounting;
    let input = pending
        .input
        .clone()
        .ok_or_else(|| AgentError::new("pending pause is not typed input"))?;

    let started = Instant::now();
    let observation = match value {
        Some(raw) => {
            let clipped: String = raw.chars().take(input.max_chars as usize).collect();
            if input.kind == crate::runtime::InputKind::Choice && !input.options.is_empty() {
                crate::tools::collect_input::format_choice_observation(&clipped, &input.options)
            } else {
                crate::tools::collect_input::format_input_observation(input.kind, &clipped)
            }
        }
        None => "Error: user cancelled typed input".into(),
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    let ok = observation_looks_ok(&observation);
    log_tool_done(&pending.name, ok, duration_ms);
    emit(AgentEvent::ToolExecutionEnd {
        call_id: pending.call_id.clone(),
        tool_name: pending.name.clone(),
        ok,
        duration_ms,
    });
    tools_used.push(pending.name.clone());
    push_tool_result_messages(
        state.context,
        &pending.name,
        &pending.call_id,
        observation,
        ok,
    );

    let tools_before_remaining = tools_used.len();
    let batch_result = process_tool_calls(
        &mut state,
        remaining,
        &mut tools_used,
        tool_rounds,
        &mut confirms_used,
        &user_text,
        config,
        &emit,
        cancel.clone(),
    )
    .await?;
    // B3: post-resume batches that actually executed tools count as a round.
    if tools_used.len() > tools_before_remaining {
        tool_rounds = tool_rounds.saturating_add(1);
    }
    if let Some(activated) = state.activated {
        crate::runtime::activate_tools(
            activated,
            tools_used[activation_start..]
                .iter()
                .filter(|name| state.tools.iter().any(|tool| tool.name() == name.as_str()))
                .cloned(),
        );
    }

    match batch_result {
        ToolBatchResult::Continue => {}
        ToolBatchResult::Paused {
            outcome,
            mut pending_turn,
        } => {
            // B2/B4: keep restored state on the second pause (gate stays 0,
            // markup/accounting preserved; tool_rounds includes post-resume).
            pending_turn.todos_file = todos_file.clone();
            pending_turn.markup_left = markup_left;
            pending_turn.gate_left = 0;
            pending_turn.token_accounting = restored_accounting.clone();
            pending_turn.tool_rounds = tool_rounds;
            pending_turn.confirms_used = confirms_used;
            return finish_paused(
                &emit,
                tool_rounds,
                outcome,
                tool_rounds,
                tools_used,
                pending_turn,
                restored_accounting,
            );
        }
    }

    agent_loop_with_budget(
        state,
        &user_text,
        config,
        tools_used,
        tool_rounds,
        confirms_used,
        cancel,
        Some(emit),
        todos_file,
        FinishGateBudget::post_hitl(markup_left),
        restored_accounting,
    )
    .await
}

/// Execute one already-approved (or rejected) pending tool (plus any
/// `batch_with` siblings covered by the same yes/no), then remaining siblings.
/// Resume after HITL with restored todos/budgets/accounting (B2/B4) and
/// post-resume round counting (B3). Post-HITL re-entry keeps `gate=0` to
/// avoid finish-gate loops after approval while preserving `markup_left`.
pub async fn resume_pending_tool(
    mut state: LoopState<'_>,
    pending_turn: PendingTurn,
    approved: bool,
    config: &AgentLoopConfig,
    emit: Option<EmitFn>,
    cancel: Option<CancellationToken>,
) -> Result<LoopResult, AgentError> {
    let emit = emit.unwrap_or_else(noop_emit);
    let mut tools_used = pending_turn.tools_used;
    let activation_start = tools_used.len();
    let mut tool_rounds = pending_turn.tool_rounds;
    let mut confirms_used = pending_turn.confirms_used;
    let batch_with = pending_turn.batch_with;
    let remaining = pending_turn.remaining_calls;
    let pending = pending_turn.pending;
    let user_text = pending_turn.user_text;
    // B2/B4: restore pre-pause state.
    let todos_file = pending_turn.todos_file;
    let markup_left = pending_turn.markup_left;
    let restored_accounting = pending_turn.token_accounting;

    let tool = find_tool(state.tools, &pending.name)?;

    emit(AgentEvent::ToolExecutionStart {
        call_id: pending.call_id.clone(),
        tool_name: pending.name.clone(),
        args_summary: pending.args_summary.clone(),
    });
    let started = Instant::now();

    // Turn-scoped shell grant: after the user says yes once to shell, later
    // bash calls this turn skip HITL (hard gates + deny list still apply).
    if approved
        && tool
            .meta()
            .permissions
            .contains(&crate::tool::Permission::Shell)
    {
        state.runtime.grant_shell_this_turn();
    }

    let observation = if approved {
        // Disjoint borrows: `tool` from `state.tools`, invoke via `state.runtime`.
        let raw = RawToolCall {
            call_id: pending.call_id.clone(),
            name: pending.name.clone(),
            args: pending.args.clone(),
        };
        let mut inv = build_tool_invocation(&raw, config, cancel.clone(), &emit);
        inv.cwd = None;
        let opts = InvokeOptions {
            skip_confirmation: true,
            confirms_used,
        };
        match state.runtime.invoke(tool, inv, opts).await {
            InvokeResult::Observation(s) => s,
            InvokeResult::Denied { reason } => format!("Error: {reason}"),
            InvokeResult::NeedsConfirmation { .. } | InvokeResult::NeedsInput { .. } => {
                "Error: unexpected pause after grant".to_string()
            }
        }
    } else {
        state.runtime.audit_rejection(
            &pending,
            config.session_id.as_deref(),
            config.turn_id.as_deref(),
        );
        "Error: user declined this action".to_string()
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    let ok = approved && observation_looks_ok(&observation);
    log_tool_done(&pending.name, ok, duration_ms);
    emit(AgentEvent::ToolExecutionEnd {
        call_id: pending.call_id.clone(),
        tool_name: pending.name.clone(),
        ok,
        duration_ms,
    });

    tools_used.push(pending.name.clone());
    push_tool_result_messages(
        state.context,
        &pending.name,
        &pending.call_id,
        observation,
        ok,
    );

    // Same yes/no covers batch_with siblings (one HITL decision).
    for call in batch_with {
        let summary = crate::runtime::args_summary(&call.name, &call.args);
        emit(AgentEvent::ToolExecutionStart {
            call_id: call.call_id.clone(),
            tool_name: call.name.clone(),
            args_summary: summary.clone(),
        });
        let started = Instant::now();
        let Some(tool) = find_tool_opt(state.tools, &call.name) else {
            let observation = unknown_tool_observation(&call.name);
            let duration_ms = started.elapsed().as_millis() as u64;
            log_tool_done(&call.name, false, duration_ms);
            emit(AgentEvent::ToolExecutionEnd {
                call_id: call.call_id.clone(),
                tool_name: call.name.clone(),
                ok: false,
                duration_ms,
            });
            tools_used.push(call.name.clone());
            push_tool_result_messages(state.context, &call.name, &call.call_id, observation, false);
            continue;
        };
        let observation = if approved {
            let mut inv = build_tool_invocation(&call, config, cancel.clone(), &emit);
            inv.cwd = None;
            let opts = InvokeOptions {
                skip_confirmation: true,
                confirms_used,
            };
            match state.runtime.invoke(tool, inv, opts).await {
                InvokeResult::Observation(s) => s,
                InvokeResult::Denied { reason } => format!("Error: {reason}"),
                InvokeResult::NeedsConfirmation { .. } | InvokeResult::NeedsInput { .. } => {
                    "Error: unexpected pause after grant".to_string()
                }
            }
        } else {
            let risk = tool.meta().risk;
            let rejected = crate::runtime::PendingToolCall::new(
                format!("batch-{}", call.call_id),
                call.name.clone(),
                call.args.clone(),
                summary,
                risk,
                call.call_id.clone(),
            );
            state.runtime.audit_rejection(
                &rejected,
                config.session_id.as_deref(),
                config.turn_id.as_deref(),
            );
            "Error: user declined this action".to_string()
        };
        let duration_ms = started.elapsed().as_millis() as u64;
        let ok = approved && observation_looks_ok(&observation);
        log_tool_done(&call.name, ok, duration_ms);
        emit(AgentEvent::ToolExecutionEnd {
            call_id: call.call_id.clone(),
            tool_name: call.name.clone(),
            ok,
            duration_ms,
        });
        tools_used.push(call.name.clone());
        push_tool_result_messages(state.context, &call.name, &call.call_id, observation, ok);
    }

    let tools_before_remaining = tools_used.len();
    let batch_result = process_tool_calls(
        &mut state,
        remaining,
        &mut tools_used,
        tool_rounds,
        &mut confirms_used,
        &user_text,
        config,
        &emit,
        cancel.clone(),
    )
    .await?;
    // B3: post-resume siblings that actually ran count as a round.
    if tools_used.len() > tools_before_remaining {
        tool_rounds = tool_rounds.saturating_add(1);
    }
    if let Some(activated) = state.activated {
        crate::runtime::activate_tools(
            activated,
            tools_used[activation_start..]
                .iter()
                .filter(|name| state.tools.iter().any(|tool| tool.name() == name.as_str()))
                .cloned(),
        );
    }

    match batch_result {
        ToolBatchResult::Continue => {}
        ToolBatchResult::Paused {
            outcome,
            mut pending_turn,
        } => {
            // B2/B4: second pause keeps restored todos/markup/accounting;
            // gate stays 0 post-HITL. Tool rounds include post-resume work.
            pending_turn.todos_file = todos_file.clone();
            pending_turn.markup_left = markup_left;
            pending_turn.gate_left = 0;
            pending_turn.token_accounting = restored_accounting.clone();
            pending_turn.tool_rounds = tool_rounds;
            pending_turn.confirms_used = confirms_used;
            emit(AgentEvent::NeedsConfirmation {
                pending: pending_turn.pending.clone(),
            });
            emit(AgentEvent::AgentEnd {
                outcome: outcome.clone(),
            });
            return Ok(LoopResult {
                outcome,
                tool_rounds,
                tools_used,
                pending_turn: Some(pending_turn),
                token_accounting: restored_accounting,
            });
        }
    }

    agent_loop_with_budget(
        state,
        &user_text,
        config,
        tools_used,
        tool_rounds,
        confirms_used,
        cancel,
        Some(emit),
        todos_file,
        FinishGateBudget::post_hitl(markup_left),
        restored_accounting,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use boris_ai::{LlmClient, LlmError};
    use serde_json::{json, Value};
    use std::sync::Mutex;

    struct ScriptedClient {
        responses: Mutex<Vec<serde_json::Value>>,
    }

    struct UsageClient;

    #[tokio::test]
    async fn presentation_failures_deliver_reports_without_progress_and_withhold_further_retries() {
        struct CapturingClient {
            responses: Mutex<Vec<Value>>,
            schemas: Mutex<Vec<Value>>,
        }
        #[async_trait]
        impl LlmClient for CapturingClient {
            async fn complete(&self, _: Value, tools: Value) -> Result<Value, LlmError> {
                self.schemas.lock().unwrap().push(tools);
                Ok(self.responses.lock().unwrap().remove(0))
            }
        }
        let call = |id: &str, artifact_id: &str| {
            json!({"role":"assistant", "content":"Preparing the report", "tool_calls":[
                {"id":id, "type":"function", "function":{"name":"present_artifact", "arguments":json!({
                    "kind":"markdown", "title":"Diagnostic", "body":"Verified tool evidence", "id":artifact_id
                }).to_string()}}
            ]})
        };
        let client = CapturingClient {
            responses: Mutex::new(vec![
                call("c1", "missing"),
                call("c2", "still-missing"),
                json!({"role":"assistant", "content":"The execution report is on screen."}),
            ]),
            schemas: Mutex::new(vec![]),
        };
        let dir = std::env::temp_dir().join(format!(
            "boris-loop-delivery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let tools: Vec<std::sync::Arc<dyn Tool>> = crate::artifact_tools_at(&dir)
            .into_iter()
            .map(std::sync::Arc::from)
            .collect();
        let runtime = ToolRuntime::new(
            crate::SandboxConfig::for_desktop_mvp(&dir).with_trusted_auto_moderate(true),
            Box::new(crate::NullAuditSink),
        );
        let mut context = Context::new(20);
        context.push(Role::System, "policy");
        context.push(Role::User, "show report");
        context.begin_task("show report");
        let events = std::sync::Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let emit: crate::types::EmitFn =
            std::sync::Arc::new(move |event| captured.lock().unwrap().push(event));
        let debug = std::sync::Arc::new(crate::DebugCapture::default());
        debug.set_enabled(true);
        let config = AgentLoopConfig {
            features: crate::ToolRuntimeFeatures {
                progress_events: false,
                ..Default::default()
            },
            debug: Some(debug.clone()),
            ..Default::default()
        };
        let activated = crate::runtime::new_activation_set();
        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: Some(&activated),
        };
        let result = agent_loop(
            state,
            "show report",
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
        assert_eq!(result.tool_rounds, 2);
        assert!(
            activated.lock().unwrap().snapshot().is_empty(),
            "core calls must not consume long-tail activation slots"
        );
        let schemas = client.schemas.lock().unwrap();
        assert!(schemas[0]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "present_artifact"));
        assert!(!schemas[2]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "present_artifact"));
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| matches!(event, crate::AgentEvent::ReportFallback { .. }))
                .count(),
            2
        );
        assert_eq!(
            crate::ArtifactStore::new(&dir).get_display(None).unwrap().1,
            "Verified tool evidence"
        );
        assert!(debug.snapshot(0).events.iter().any(
            |event| event.kind == "tool_listing" && event.data["presentation_disabled"] == true
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[async_trait]
    impl LlmClient for UsageClient {
        async fn complete(
            &self,
            _messages: serde_json::Value,
            _tools: serde_json::Value,
        ) -> Result<serde_json::Value, LlmError> {
            Ok(json!({"role":"assistant","content":"measured"}))
        }

        async fn complete_stream(
            &self,
            _messages: serde_json::Value,
            _tools: serde_json::Value,
            _opts: boris_ai::CompleteOptions,
            on_event: &mut (dyn FnMut(boris_ai::LlmStreamEvent) + Send),
        ) -> Result<serde_json::Value, LlmError> {
            let message = json!({"role":"assistant","content":"measured"});
            on_event(boris_ai::LlmStreamEvent::Usage(boris_ai::TokenUsage {
                prompt_tokens: 321,
                completion_tokens: 7,
                total_tokens: 328,
                ..Default::default()
            }));
            on_event(boris_ai::LlmStreamEvent::FinalMessage(message.clone()));
            Ok(message)
        }

        fn context_window_tokens(&self) -> Option<u32> {
            Some(64_000)
        }
    }

    #[async_trait]
    impl LlmClient for ScriptedClient {
        async fn complete(
            &self,
            _messages: serde_json::Value,
            _tools: serde_json::Value,
        ) -> Result<serde_json::Value, LlmError> {
            let mut guard = self.responses.lock().unwrap();
            if guard.is_empty() {
                return Err(LlmError::new("no more scripted responses"));
            }
            Ok(guard.remove(0))
        }

        fn model(&self) -> &str {
            "test"
        }
    }

    #[tokio::test]
    async fn loop_speaks_without_tools() {
        let client = ScriptedClient {
            responses: Mutex::new(vec![json!({
                "role": "assistant",
                "content": "Hello there."
            })]),
        };
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
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
        let result = agent_loop(state, "hi", &config, vec![], 0, 0, None, None, None, 0)
            .await
            .unwrap();
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Hello there."),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(result.tool_rounds, 0);
        assert!(result.pending_turn.is_none());
    }

    #[tokio::test]
    async fn loop_prefers_provider_usage_and_reports_client_window() {
        let client = UsageClient;
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "hi");
        let runtime = ToolRuntime::null();
        let tools: Vec<std::sync::Arc<dyn Tool>> = vec![];
        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };

        let result = agent_loop(
            state,
            "hi",
            &AgentLoopConfig::default(),
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

        assert_eq!(result.token_accounting.context_used_tokens(), 321);
        assert!(!result.token_accounting.context_is_estimated());
        assert_eq!(result.token_accounting.context_limit_tokens, Some(64_000));
    }

    #[tokio::test]
    async fn loop_runs_safe_tool_then_speaks() {
        use crate::tool::{ToolError, ToolMeta};

        struct Echo;
        #[async_trait]
        impl Tool for Echo {
            fn name(&self) -> &str {
                "echo"
            }
            fn description(&self) -> &str {
                "echo"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type":"object","properties":{},"required":[]})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::safe_default()
            }
            async fn execute(
                &self,
                _ctx: &crate::tool_context::ToolCallContext,
                _args: serde_json::Value,
            ) -> Result<String, ToolError> {
                Ok("pong".into())
            }
        }

        let client = ScriptedClient {
            responses: Mutex::new(vec![
                json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": { "name": "echo", "arguments": "{}" }
                    }]
                }),
                json!({
                    "role": "assistant",
                    "content": "Done."
                }),
            ]),
        };
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "ping");
        let runtime = ToolRuntime::null();
        let tools: Vec<std::sync::Arc<dyn Tool>> = vec![std::sync::Arc::new(Echo)];
        let config = AgentLoopConfig::default();
        let activated = crate::runtime::new_activation_set();
        crate::runtime::activate_tools(&activated, std::iter::once("echo".to_string()));
        crate::runtime::activate_tools(
            &activated,
            (0..crate::runtime::MAX_ACTIVATED - 1).map(|i| format!("filler-{i}")),
        );

        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: Some(&activated),
        };
        let result = agent_loop(state, "ping", &config, vec![], 0, 0, None, None, None, 0)
            .await
            .unwrap();
        assert_eq!(result.tools_used, vec!["echo".to_string()]);
        assert_eq!(result.tool_rounds, 1);
        // A real invocation refreshes LRU recency: one more activation evicts a
        // filler, not the previously-oldest `echo` entry.
        crate::runtime::activate_tools(&activated, std::iter::once("overflow".to_string()));
        assert!(activated.lock().unwrap().contains("echo"));
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Done."),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn loop_runs_parallel_safe_tools_then_speaks() {
        use crate::tool::{ToolError, ToolMeta};
        use std::sync::Arc;

        struct Alpha;
        #[async_trait]
        impl Tool for Alpha {
            fn name(&self) -> &str {
                "alpha"
            }
            fn description(&self) -> &str {
                "alpha"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type":"object","properties":{},"required":[]})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::safe_default()
            }
            async fn execute(
                &self,
                _ctx: &crate::tool_context::ToolCallContext,
                _args: serde_json::Value,
            ) -> Result<String, ToolError> {
                Ok("A".into())
            }
        }

        struct Beta;
        #[async_trait]
        impl Tool for Beta {
            fn name(&self) -> &str {
                "beta"
            }
            fn description(&self) -> &str {
                "beta"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type":"object","properties":{},"required":[]})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::safe_default()
            }
            async fn execute(
                &self,
                _ctx: &crate::tool_context::ToolCallContext,
                _args: serde_json::Value,
            ) -> Result<String, ToolError> {
                Ok("B".into())
            }
        }

        let client = ScriptedClient {
            responses: Mutex::new(vec![
                json!({
                    "role": "assistant",
                    "content": "I'll run the two independent checks, then report what they found.",
                    "tool_calls": [
                        {
                            "id": "c1",
                            "type": "function",
                            "function": { "name": "alpha", "arguments": "{}" }
                        },
                        {
                            "id": "c2",
                            "type": "function",
                            "function": { "name": "beta", "arguments": "{}" }
                        }
                    ]
                }),
                json!({
                    "role": "assistant",
                    "content": "Both done."
                }),
            ]),
        };
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "run both");
        let runtime = ToolRuntime::null();
        let tools: Vec<std::sync::Arc<dyn Tool>> =
            vec![std::sync::Arc::new(Alpha), std::sync::Arc::new(Beta)];
        let config = AgentLoopConfig::default();
        let events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
        let events_out = events.clone();
        let emit: EmitFn = Arc::new(move |event| {
            events_out.lock().unwrap().push(event);
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
            "run both",
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

        assert_eq!(
            result.tools_used,
            vec!["alpha".to_string(), "beta".to_string()]
        );
        assert_eq!(result.tool_rounds, 1);
        let events = events.lock().unwrap();
        let note = events
            .iter()
            .position(|event| matches!(event, AgentEvent::ToolNote { text } if text == "I'll run the two independent checks, then report what they found."))
            .expect("tool note");
        let start = events
            .iter()
            .position(|event| matches!(event, AgentEvent::ToolExecutionStart { .. }))
            .expect("tool start");
        assert!(note < start);
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Both done."),
            other => panic!("unexpected {other:?}"),
        }

        // Both tool observations present, in original batch order.
        let tool_msgs: Vec<_> = context
            .messages()
            .iter()
            .filter(|m| matches!(m.role, Role::Tool))
            .collect();
        assert_eq!(tool_msgs.len(), 2);
        assert_eq!(tool_msgs[0].content["tool_call_id"], "c1");
        assert_eq!(tool_msgs[0].content["content"], "A");
        assert_eq!(tool_msgs[1].content["tool_call_id"], "c2");
        assert_eq!(tool_msgs[1].content["content"], "B");
    }

    #[tokio::test]
    async fn markup_rejection_is_host_control_not_fake_assistant() {
        // B5: "[invalid tool format...]" must not pollute history as
        // Role::Assistant. It is host control (ephemeral, never persisted).
        let client = ScriptedClient {
            responses: Mutex::new(vec![
                json!({
                    "role": "assistant",
                    "content": "<invoke name=\"web_search\"><parameter name=\"q\">x</parameter></invoke>"
                }),
                json!({"role": "assistant", "content": "Done."}),
            ]),
        };
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
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
        let result = agent_loop_with_budget(
            state,
            "hi",
            &config,
            vec![],
            0,
            0,
            None,
            None,
            None,
            crate::finish_gate::FinishGateBudget::fresh(),
            TokenAccounting::default(),
        )
        .await
        .unwrap();
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Done."),
            other => panic!("unexpected {other:?}"),
        }
        // No fake assistant text in history or model view.
        for m in context.messages() {
            if matches!(m.role, Role::Assistant) {
                let s = m.content.to_string();
                assert!(
                    !s.contains("invalid tool format"),
                    "fake assistant must not persist, got: {s}"
                );
            }
        }
        for m in context.history() {
            let s = m.content.to_string();
            assert!(
                !s.contains("invalid tool format"),
                "fake assistant must not enter transcript, got: {s}"
            );
        }
        // Host controls carry the rejection + protocol reminder.
        let controls: Vec<_> = context
            .messages()
            .iter()
            .filter(|m| m.origin == crate::MessageOrigin::HostControl)
            .collect();
        assert!(
            controls
                .iter()
                .any(|m| m.content.to_string().contains("invalid tool format")),
            "expected HostControl rejection"
        );
        assert!(
            controls
                .iter()
                .any(|m| m.content.to_string().contains("function-calling API")),
            "expected protocol reminder"
        );
    }

    #[tokio::test]
    async fn unknown_tool_breaker_injects_available_tools() {
        // B11: two consecutive unknown-only rounds inject a HostControl with
        // available names + tool_search hint, then valid speech resets.
        let client = ScriptedClient {
            responses: Mutex::new(vec![
                json!({
                    "role": "assistant", "content": null,
                    "tool_calls": [{"id":"u1","type":"function","function":{"name":"ghost_tool","arguments":"{}"}}]
                }),
                json!({
                    "role": "assistant", "content": null,
                    "tool_calls": [{"id":"u2","type":"function","function":{"name":"ghost_tool","arguments":"{}"}}]
                }),
                json!({"role": "assistant", "content": "All set."}),
            ]),
        };
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "do work");
        let runtime = ToolRuntime::null();
        // Registered table: echo + tool_search (for the hint branch).
        struct Echo;
        #[async_trait]
        impl crate::tool::Tool for Echo {
            fn name(&self) -> &str {
                "echo"
            }
            fn description(&self) -> &str {
                "echo"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type":"object","properties":{},"required":[]})
            }
            fn meta(&self) -> crate::tool::ToolMeta {
                crate::tool::ToolMeta::safe_default()
            }
            async fn execute(
                &self,
                _ctx: &crate::tool_context::ToolCallContext,
                _args: serde_json::Value,
            ) -> Result<String, crate::tool::ToolError> {
                Ok("ok".into())
            }
        }
        struct Search;
        #[async_trait]
        impl crate::tool::Tool for Search {
            fn name(&self) -> &str {
                "tool_search"
            }
            fn description(&self) -> &str {
                "search"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type":"object","properties":{},"required":[]})
            }
            fn meta(&self) -> crate::tool::ToolMeta {
                crate::tool::ToolMeta::safe_default()
            }
            async fn execute(
                &self,
                _ctx: &crate::tool_context::ToolCallContext,
                _args: serde_json::Value,
            ) -> Result<String, crate::tool::ToolError> {
                Ok("ok".into())
            }
        }
        let tools: Vec<std::sync::Arc<dyn crate::tool::Tool>> =
            vec![std::sync::Arc::new(Echo), std::sync::Arc::new(Search)];
        let config = AgentLoopConfig::default();
        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        let result = agent_loop(state, "do work", &config, vec![], 0, 0, None, None, None, 0)
            .await
            .unwrap();
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "All set."),
            other => panic!("unexpected {other:?}"),
        }
        let controls: Vec<String> = context
            .messages()
            .iter()
            .filter(|m| m.origin == crate::MessageOrigin::HostControl)
            .map(|m| m.content.to_string())
            .collect();
        assert!(
            controls.iter().any(|s| s.contains("Available tools")
                && s.contains("echo")
                && s.contains("tool_search")),
            "breaker must list tools, got: {controls:?}"
        );
    }

    #[tokio::test]
    async fn markup_budget_survives_when_gate_is_zero() {
        // B4: split budgets — markup re-entry works even with gate=0.
        let client = ScriptedClient {
            responses: Mutex::new(vec![
                json!({
                    "role": "assistant",
                    "content": "<invoke name=\"web_search\"><parameter name=\"q\">x</parameter></invoke>"
                }),
                json!({"role": "assistant", "content": "Recovered."}),
            ]),
        };
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "hi");
        let runtime = ToolRuntime::null();
        let tools: Vec<std::sync::Arc<dyn crate::tool::Tool>> = vec![];
        let config = AgentLoopConfig::default();
        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        let result = agent_loop_with_budget(
            state,
            "hi",
            &config,
            vec![],
            0,
            0,
            None,
            None,
            None,
            crate::finish_gate::FinishGateBudget { markup: 2, gate: 0 },
            TokenAccounting::default(),
        )
        .await
        .unwrap();
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Recovered."),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn resume_preserves_accounting_todos_and_increments_rounds() {
        // B2+B3: resume restores todos/accounting/budgets; post-resume safe
        // tools increment tool_rounds.
        use crate::tool::{ToolError, ToolMeta};
        struct Safe;
        #[async_trait]
        impl crate::tool::Tool for Safe {
            fn name(&self) -> &str {
                "safe"
            }
            fn description(&self) -> &str {
                "safe"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type":"object","properties":{},"required":[]})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::safe_default()
            }
            async fn execute(
                &self,
                _ctx: &crate::tool_context::ToolCallContext,
                _args: serde_json::Value,
            ) -> Result<String, ToolError> {
                Ok("ok".into())
            }
        }
        let client = ScriptedClient {
            responses: Mutex::new(vec![json!({"role":"assistant","content":"Finished."})]),
        };
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "resume me");
        let runtime = ToolRuntime::null();
        let tools: Vec<std::sync::Arc<dyn crate::tool::Tool>> = vec![std::sync::Arc::new(Safe)];
        let config = AgentLoopConfig::default();
        let todos = std::env::temp_dir().join(format!(
            "boris-resume-todos-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut accounting = TokenAccounting::default();
        accounting.record_request_estimate(9000, 64_000);
        let pending = PendingTurn {
            pending: crate::runtime::PendingToolCall::new(
                "p1",
                "safe",
                json!({}),
                "safe",
                crate::tool::ToolRisk::Safe,
                "call-pending",
            ),
            batch_with: vec![],
            remaining_calls: vec![crate::runtime::RawToolCall {
                call_id: "r1".into(),
                name: "safe".into(),
                args: json!({}),
            }],
            tools_used: vec!["safe".into()],
            tool_rounds: 1,
            confirms_used: 0,
            user_text: "resume me".into(),
            todos_file: Some(todos.clone()),
            markup_left: 1,
            gate_left: 3,
            token_accounting: accounting.clone(),
        };
        // Pending is an input-style pause? Use tool resume with unknown pending
        // name would error on find_tool; instead test input resume path with a
        // collect_input pending + safe remaining.
        let pending_input = crate::runtime::PendingToolCall::new(
            "p-in",
            "collect_input",
            json!({"spoken":"Type it.","label":"L"}),
            "collect",
            crate::tool::ToolRisk::Safe,
            "call-in",
        )
        .with_input(crate::runtime::PendingInput {
            kind: crate::runtime::InputKind::Exact,
            label: "L".into(),
            max_chars: 512,
            options: Vec::new(),
        });
        let tools2: Vec<std::sync::Arc<dyn crate::tool::Tool>> = vec![
            std::sync::Arc::new(crate::tools::collect_input::CollectInputTool),
            std::sync::Arc::new(Safe),
        ];
        let pending2 = PendingTurn {
            pending: pending_input,
            batch_with: vec![],
            remaining_calls: vec![crate::runtime::RawToolCall {
                call_id: "r1".into(),
                name: "safe".into(),
                args: json!({}),
            }],
            tools_used: vec![],
            tool_rounds: 1,
            confirms_used: 0,
            user_text: "resume me".into(),
            todos_file: Some(todos.clone()),
            markup_left: 1,
            gate_left: 3,
            token_accounting: accounting.clone(),
        };
        let state = LoopState {
            context: &mut context,
            tools: &tools2,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        let result =
            resume_pending_input(state, pending2, Some("hello".into()), &config, None, None)
                .await
                .unwrap();
        // Post-resume safe tool ran => tool_rounds 1 -> 2.
        assert_eq!(result.tool_rounds, 2, "post-resume batch must count");
        // Accounting restored, not default.
        assert_eq!(
            result.token_accounting.peak_estimated_request_tokens, 9000,
            "pre-pause accounting must survive resume"
        );
        // Tools used preserved + new.
        assert!(result.tools_used.contains(&"safe".to_string()));
        // Todos file would have been used for any gate; ensure no fallback to
        // default guess corrupted state (pending2 had explicit path).
        let _ = pending;
        let _ = tools;
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Finished."),
            other => panic!("unexpected {other:?}"),
        }
    }
}
