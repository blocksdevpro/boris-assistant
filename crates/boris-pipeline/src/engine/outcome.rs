//! HITL confirmation path: speak prompt → freeform yes/no → resume agent.
//!
//! Called from the main turn loop after `agent.prompt_with_report` returns
//! [`AgentOutcome::NeedsConfirmation`]. Nested confirms are capped (also in agent policy).

use std::sync::mpsc::Receiver;
use std::sync::{atomic::AtomicBool, Arc};

use boris_agent::{Agent, AgentCheckpoint, AgentErrorKind, AgentOutcome, PendingToolCall};
use boris_agent::{SessionId, SessionStore, TurnReport};
use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;
use boris_core::{ArcAudioBuffer, TurnId};
use boris_sense::{SileroVad, WakeWord};

use crate::hear::{self, CaptureKind, HearBreak};
use crate::liveness::WakeLiveness;
use crate::status::{InputPeek, Phase};

use super::barge::BargeWatch;
use super::confirm::interpret_yes_no;
use super::device_switch::{apply_input_switch, apply_output_switch};
use super::models::{maybe_unload_stt, release_voice_models, ModelResidency, SttBox, TtsBox};
use super::picture::Picture;
use super::playback::{
    drain_output_events, wait_playback_or_stop, wait_playback_started, PlaybackWait,
};
use super::session::{end_session, go_off};
use super::think::{run_thinking, AgentWork, ThinkCtx, ThinkResolve};
use super::EngineCommand;

pub(super) enum OutcomeResolve {
    /// Boxed: `AgentOutcome` carries pending-call batches (~200B+) and this
    /// enum crosses several `Result` boundaries per turn.
    Done(Box<ResolveDone>),
    ReArm,
    Stopped,
    TakeTurn {
        user_text: String,
        interrupted_text: Option<String>,
    },
}

/// Resolved turn payload: final outcome plus the latest HITL-resume report
/// (if any confirm/input round ran) for the context meter + turn trace.
pub(super) struct ResolveDone {
    pub outcome: AgentOutcome,
    pub resume_report: Option<TurnReport>,
}

/// Mutable + shared context for the confirm resolution loop (avoids 18-param functions).
pub(super) struct ConfirmCtx<'a> {
    pub agent: &'a mut Agent,
    pub agent_rt: &'a tokio::runtime::Runtime,
    pub tts: &'a mut TtsBox,
    pub stt: &'a mut SttBox,
    pub mic: &'a crossbeam_channel::Receiver<ArcAudioBuffer>,
    pub wake: &'a mut dyn WakeWord,
    pub vad: &'a mut SileroVad,
    pub liveness: &'a mut WakeLiveness,
    pub barge_in: bool,
    pub audio: &'a mut AudioService,
    pub output_events: &'a mut crossbeam_channel::Receiver<OutputEvent>,
    pub cmd_rx: &'a Receiver<EngineCommand>,
    pub running: &'a mut bool,
    pub picture: &'a mut Picture,
    pub activity_events_enabled: Arc<AtomicBool>,
    pub store: &'a SessionStore,
    pub active_session: &'a mut Option<SessionId>,
    pub transcript_len: &'a mut usize,
    pub original_heard: Option<String>,
    pub turn_checkpoint: AgentCheckpoint,
    pub turn: TurnId,
    /// STT eviction policy for confirm captures (mirrors the turn loop:
    /// `LowMemory` unloads after each transcribe, otherwise models stay warm
    /// across the re-prompt / re-ask rounds).
    pub residency: ModelResidency,
    /// Voice HITL budget for this turn — mirrors the agent policy
    /// (`SandboxConfig::max_confirms_per_turn`, default 12). The loop must not
    /// cap below the policy or budgeted confirms become unreachable by voice.
    pub max_confirms: u32,
    /// Latest `(outcome, report)` from a HITL resume this turn, if any. The
    /// caller merges it into the context meter + turn trace — without this the
    /// post-confirm tool rounds are invisible to the UI meter.
    pub last_resume_report: Option<TurnReport>,
}

impl ConfirmCtx<'_> {
    /// Shared teardown for the several near-identical `go_off(...)` call sites
    /// below: end session, stop audio, release STT/TTS, flip UI to Off.
    fn go_off(&mut self) {
        go_off(
            self.picture,
            self.audio,
            self.store,
            self.active_session,
            self.transcript_len,
            self.stt.as_mut(),
            self.tts.as_mut(),
            self.agent,
        )
    }
}

/// Drive NeedsConfirmation / NeedsInput pauses until Speak/Silent.
pub(super) fn resolve_agent_outcome(
    mut outcome: AgentOutcome,
    ctx: &mut ConfirmCtx<'_>,
) -> OutcomeResolve {
    // Mirror the agent policy budget (default 12). A hardcoded cap below the
    // policy would strand budgeted confirms: the agent would allow them but
    // voice would abort first with "too many confirmations".
    let budget = ctx.max_confirms.max(1);
    for _ in 0..budget {
        if let AgentOutcome::NeedsInput { text, pending } = outcome {
            outcome = match collect_typed_input(ctx, text, pending) {
                Ok(o) => o,
                Err(res) => return res,
            };
            continue;
        }
        let AgentOutcome::NeedsConfirmation {
            text: prompt,
            pending,
        } = outcome
        else {
            return OutcomeResolve::Done(Box::new(ResolveDone {
                outcome,
                resume_report: ctx.last_resume_report.take(),
            }));
        };

        tracing::info!(
            turn = %ctx.turn,
            tool = %pending.name,
            pending_id = %pending.id,
            "agent needs confirmation"
        );
        // Never put confirm text in `detail` (overlay treats detail as error).
        // Drop prior-turn `heard` so the UI does not show the last STT line as
        // "You" while waiting for yes/no. Showing the spoken "yes" as `heard`
        // afterwards is intended; the ORIGINAL utterance is preserved
        // separately in `ConfirmCtx.original_heard` for barge `TakeTurn`
        // replacement, so the next turn still quotes the real request.
        ctx.picture.detail = None;
        ctx.picture.heard = None;
        ctx.picture.activity = Some(confirm_activity(&pending.name, &pending.args_summary));
        ctx.picture.said = Some(prompt.clone());
        ctx.picture.publish();

        // Speak the confirm prompt. Preload STT while TTS runs so the user is
        // not stuck waiting on model load after the prompt finishes.
        if let Err(e) = ctx.tts.load() {
            tracing::error!(error = %e, "tts load failed for confirm");
            ctx.agent.abort();
            ctx.picture.detail = Some(format!("tts: {e}"));
            ctx.picture.set_phase(Phase::Armed);
            return OutcomeResolve::ReArm;
        }
        // Fire STT load early (best-effort); confirm capture needs it ready.
        if let Err(e) = ctx.stt.load() {
            tracing::error!(error = %e, "stt load failed for confirm");
            ctx.agent.abort();
            ctx.picture.detail = Some(format!("stt: {e}"));
            ctx.picture.clear_activity();
            ctx.picture.set_phase(Phase::Armed);
            return OutcomeResolve::ReArm;
        }
        let pcm = match ctx.tts.synthesize(&prompt) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "tts synth failed for confirm");
                ctx.agent.abort();
                // Eager evict: a failed synth may mean a wedged model — force
                // reload next turn instead of reusing it.
                if let Err(e) = ctx.tts.unload() {
                    tracing::warn!(error = %e, "confirm tts unload after synth failure failed");
                }
                ctx.picture.detail = Some(format!("tts: {e}"));
                ctx.picture.set_phase(Phase::Armed);
                return OutcomeResolve::ReArm;
            }
        };
        if drain_output_events(ctx.output_events) {
            // Audio worker is gone: `play` below fails and the wait reports
            // Stopped, which ends in `go_off`. Fall through rather than
            // special-casing here.
            tracing::warn!(turn = %ctx.turn, "output channel dead before confirm prompt");
        }
        // UI: show confirm context while Boris is speaking so the user knows
        // a yes/no is coming (not a freeform reply).
        ctx.picture.set_phase(Phase::AwaitingConfirm);
        if let Err(e) = ctx.audio.play(pcm) {
            tracing::error!(error = %e, "confirm prompt play failed");
        }
        // Barge-in owns this prompt: a wake word mid-prompt stops speaking and
        // falls through to listening — the user's live speech becomes the
        // confirm answer instead of an auto-deny. The watch is scoped per wait
        // so its `&mut` borrow of wake ends before the `go_off` arms below.
        let started = {
            let mut watch = ctx.barge_in.then(|| BargeWatch::new(ctx.mic, ctx.wake));
            wait_playback_started(
                ctx.output_events,
                ctx.cmd_rx,
                ctx.running,
                ctx.audio,
                ctx.picture,
                watch.as_mut(),
            )
        };
        match started {
            PlaybackWait::Stopped => {
                ctx.agent.abort();
                ctx.go_off();
                return OutcomeResolve::Stopped;
            }
            // Barged in: skip the rest of the prompt — listen now.
            PlaybackWait::BargedIn => {}
            PlaybackWait::Aborted => {}
            PlaybackWait::Finished => {
                ctx.picture.set_phase(Phase::Talking);
                // Keep activity as confirm so overlay still reads as yes/no.
                ctx.picture.activity = Some(confirm_activity(&pending.name, &pending.args_summary));
                let drained = {
                    let mut watch = ctx.barge_in.then(|| BargeWatch::new(ctx.mic, ctx.wake));
                    wait_playback_or_stop(
                        ctx.output_events,
                        ctx.cmd_rx,
                        ctx.running,
                        ctx.audio,
                        ctx.picture,
                        watch.as_mut(),
                    )
                };
                match drained {
                    PlaybackWait::Finished | PlaybackWait::BargedIn => {}
                    PlaybackWait::Stopped => {
                        ctx.agent.abort();
                        ctx.go_off();
                        return OutcomeResolve::Stopped;
                    }
                    PlaybackWait::Aborted => {
                        // Speaker switched mid-prompt (or a stuck-job flush):
                        // the user never heard the question. `abort()` resolves
                        // the pending tool as cancelled inside the agent, so
                        // nothing dangles — re-arm and let them wake again.
                        // Hint goes to `activity`, not `detail`: the overlay
                        // renders `detail` as an error and this is transient.
                        ctx.agent.abort();
                        ctx.picture.activity =
                            Some("confirmation interrupted — wake me and try again".into());
                        ctx.picture.set_phase(Phase::Armed);
                        return OutcomeResolve::ReArm;
                    }
                }
            }
        }
        if !*ctx.running {
            ctx.agent.abort();
            ctx.go_off();
            return OutcomeResolve::Stopped;
        }

        // Brief post-TTS settle, then open the mic — phase already AwaitingConfirm.
        ctx.picture.set_phase(Phase::AwaitingConfirm);
        ctx.picture.activity = Some("confirm · say yes or no".into());
        ctx.picture.publish();
        if let Err(e) = hear::settle_after_confirm(ctx.mic, ctx.cmd_rx, ctx.running) {
            ctx.agent.abort();
            ctx.picture.clear_activity();
            return match e {
                HearBreak::Stopped if !*ctx.running => {
                    ctx.go_off();
                    OutcomeResolve::Stopped
                }
                HearBreak::Disconnected => {
                    end_session(ctx.store, ctx.active_session, ctx.transcript_len, ctx.agent);
                    release_voice_models(ctx.stt.as_mut(), ctx.tts.as_mut(), "disconnected");
                    ctx.picture.mark_devices_dead();
                    ctx.picture.set_phase(Phase::Off);
                    OutcomeResolve::Stopped
                }
                // Device switch mid-confirm: apply, then re-enter this budget
                // iteration so the prompt is re-spoken on the new device
                // instead of capturing on a stale mic. (`prompt`/`pending`
                // were moved out of `outcome` above, so rebuild it — a plain
                // `continue` would use a moved value.)
                HearBreak::SwitchInput { device_id } => {
                    apply_input_switch(ctx.audio, ctx.picture, &device_id);
                    outcome = AgentOutcome::NeedsConfirmation {
                        text: prompt,
                        pending,
                    };
                    continue;
                }
                HearBreak::SwitchOutput { device_id } => {
                    apply_output_switch(ctx.audio, ctx.output_events, ctx.picture, &device_id);
                    outcome = AgentOutcome::NeedsConfirmation {
                        text: prompt,
                        pending,
                    };
                    continue;
                }
                // Teach page stole the mic mid-confirm: yield. `abort()`
                // cancels the pending tool inside the agent — nothing dangles.
                HearBreak::StartWakeEnroll { .. } | HearBreak::ClearWakeProfile => {
                    tracing::info!(turn = %ctx.turn, "wake enroll during confirm — yielding turn");
                    ctx.picture.set_phase(Phase::Armed);
                    OutcomeResolve::ReArm
                }
                HearBreak::Stopped => {
                    ctx.picture.set_phase(Phase::Armed);
                    OutcomeResolve::ReArm
                }
            };
        }

        ctx.picture.set_phase(Phase::Hearing);
        ctx.picture.activity = Some("confirm · listening".into());
        ctx.picture.publish();
        let clip = match hear::capture_utterance(
            ctx.mic,
            ctx.vad,
            ctx.cmd_rx,
            ctx.running,
            CaptureKind::AwaitConfirm,
        ) {
            Ok(c) => c,
            Err(HearBreak::Stopped) if !*ctx.running => {
                ctx.agent.abort();
                ctx.go_off();
                return OutcomeResolve::Stopped;
            }
            Err(HearBreak::Disconnected) => {
                ctx.agent.abort();
                end_session(ctx.store, ctx.active_session, ctx.transcript_len, ctx.agent);
                release_voice_models(ctx.stt.as_mut(), ctx.tts.as_mut(), "disconnected");
                ctx.picture.mark_devices_dead();
                ctx.picture.set_phase(Phase::Off);
                return OutcomeResolve::Stopped;
            }
            Err(HearBreak::SwitchInput { device_id }) => {
                apply_input_switch(ctx.audio, ctx.picture, &device_id);
                outcome = AgentOutcome::NeedsConfirmation {
                    text: prompt,
                    pending,
                };
                continue;
            }
            Err(HearBreak::SwitchOutput { device_id }) => {
                apply_output_switch(ctx.audio, ctx.output_events, ctx.picture, &device_id);
                outcome = AgentOutcome::NeedsConfirmation {
                    text: prompt,
                    pending,
                };
                continue;
            }
            Err(HearBreak::StartWakeEnroll { .. } | HearBreak::ClearWakeProfile) => {
                tracing::info!(turn = %ctx.turn, "wake enroll during confirm — yielding turn");
                ctx.agent.abort();
                ctx.picture.set_phase(Phase::Armed);
                return OutcomeResolve::ReArm;
            }
            Err(HearBreak::Stopped) => {
                // Silence: re-prompt once instead of silently rejecting.
                tracing::info!(turn = %ctx.turn, "confirm capture empty — re-prompt");
                let reask = "I need a yes or no on that.";
                ctx.picture.said = Some(reask.into());
                ctx.picture.activity = Some("confirm · say yes or no".into());
                ctx.picture.publish();
                if let Ok(pcm) = ctx.tts.synthesize(reask) {
                    drain_output_events(ctx.output_events);
                    if let Err(e) = ctx.audio.play(pcm) {
                        tracing::warn!(error = %e, "confirm reask play failed");
                    }
                    // Barge watch included: a wake hit stops the re-ask and
                    // falls through to capture (returns are intentionally
                    // ignored — any outcome proceeds to listening).
                    let mut watch = ctx.barge_in.then(|| BargeWatch::new(ctx.mic, ctx.wake));
                    let _ = wait_playback_started(
                        ctx.output_events,
                        ctx.cmd_rx,
                        ctx.running,
                        ctx.audio,
                        ctx.picture,
                        watch.as_mut(),
                    );
                    wait_playback_or_stop(
                        ctx.output_events,
                        ctx.cmd_rx,
                        ctx.running,
                        ctx.audio,
                        ctx.picture,
                        watch.as_mut(),
                    );
                }
                if !*ctx.running {
                    ctx.agent.abort();
                    ctx.go_off();
                    return OutcomeResolve::Stopped;
                }
                if let Err(e) = hear::settle_after_confirm(ctx.mic, ctx.cmd_rx, ctx.running) {
                    tracing::warn!(error = ?e, turn = %ctx.turn, "confirm re-prompt settle failed");
                }
                ctx.picture.set_phase(Phase::Hearing);
                ctx.picture.activity = Some("confirm · listening".into());
                ctx.picture.publish();
                match hear::capture_utterance(
                    ctx.mic,
                    ctx.vad,
                    ctx.cmd_rx,
                    ctx.running,
                    CaptureKind::AwaitConfirm,
                ) {
                    Ok(c) => c,
                    Err(_) => {
                        tracing::info!(turn = %ctx.turn, "confirm second capture failed — reject");
                        // Resume via the same barge-aware `run_thinking` path as
                        // every other agent call (not a bare `block_on`), so
                        // Stop/wake barge-in and `TurnCancel` apply uniformly
                        // while the reject round runs.
                        let pending_id = pending.id.clone();
                        match run_thinking(ThinkCtx {
                            agent: ctx.agent,
                            agent_rt: ctx.agent_rt,
                            mic: ctx.mic,
                            wake: ctx.wake,
                            vad: ctx.vad,
                            stt: ctx.stt,
                            liveness: ctx.liveness,
                            barge_in: ctx.barge_in,
                            audio: ctx.audio,
                            output_events: ctx.output_events,
                            picture: ctx.picture,
                            activity_events_enabled: ctx.activity_events_enabled.clone(),
                            cmd_rx: ctx.cmd_rx,
                            running: ctx.running,
                            work: AgentWork::Resume {
                                pending_id: &pending_id,
                                approved: false,
                            },
                            turn: ctx.turn,
                        }) {
                            ThinkResolve::Finished(Ok((o, report))) => {
                                ctx.picture.update_context(
                                    report.context_used_tokens,
                                    report.context_limit_tokens,
                                    report.context_estimated,
                                );
                                ctx.last_resume_report = Some(report);
                                outcome = o;
                            }
                            ThinkResolve::Finished(Err(e)) => {
                                tracing::error!(error = %e, "resume reject failed");
                                ctx.agent.abort();
                                ctx.picture.detail = Some(format!("agent: {e}"));
                                ctx.picture.clear_activity();
                                ctx.picture.set_phase(Phase::Armed);
                                return OutcomeResolve::ReArm;
                            }
                            ThinkResolve::StopTurn => {
                                ctx.agent.restore_checkpoint(ctx.turn_checkpoint.clone());
                                ctx.picture.clear_activity();
                                ctx.picture.set_phase(Phase::Armed);
                                return OutcomeResolve::ReArm;
                            }
                            ThinkResolve::TakeTurn(text) => {
                                ctx.agent.restore_checkpoint(ctx.turn_checkpoint.clone());
                                ctx.picture.clear_activity();
                                return OutcomeResolve::TakeTurn {
                                    user_text: text,
                                    interrupted_text: ctx.original_heard.clone(),
                                };
                            }
                            ThinkResolve::Stopped => {
                                ctx.agent.abort();
                                ctx.go_off();
                                return OutcomeResolve::Stopped;
                            }
                        }
                        ctx.picture.activity = Some("thinking…".into());
                        ctx.picture.set_phase(Phase::Thinking);
                        continue;
                    }
                }
            }
        };

        ctx.picture.set_phase(Phase::Reading);
        let heard = match ctx.stt.transcribe(&clip) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "confirm STT failed");
                String::new()
            }
        };
        // Residency-aware: `LowMemory` evicts here, otherwise the model stays
        // warm across the re-prompt / re-ask rounds below.
        maybe_unload_stt(ctx.stt.as_mut(), ctx.turn, ctx.residency);
        ctx.picture.heard = Some(heard.clone());
        ctx.picture.publish();

        let approved = match interpret_yes_no(&heard) {
            Some(v) => v,
            None => {
                tracing::info!(turn = %ctx.turn, heard = %heard, "confirm answer ambiguous — re-ask once");
                let reask = "Was that a yes or a no?";
                ctx.picture.said = Some(reask.into());
                ctx.picture.activity = Some("confirm · yes or no".into());
                ctx.picture.publish();
                if let Ok(pcm) = ctx.tts.synthesize(reask) {
                    drain_output_events(ctx.output_events);
                    if let Err(e) = ctx.audio.play(pcm) {
                        tracing::warn!(error = %e, "confirm reask play failed");
                    }
                    // Barge watch included: a wake hit stops the re-ask and
                    // falls through to capture (returns are intentionally
                    // ignored — any outcome proceeds to listening).
                    let mut watch = ctx.barge_in.then(|| BargeWatch::new(ctx.mic, ctx.wake));
                    let _ = wait_playback_started(
                        ctx.output_events,
                        ctx.cmd_rx,
                        ctx.running,
                        ctx.audio,
                        ctx.picture,
                        watch.as_mut(),
                    );
                    wait_playback_or_stop(
                        ctx.output_events,
                        ctx.cmd_rx,
                        ctx.running,
                        ctx.audio,
                        ctx.picture,
                        watch.as_mut(),
                    );
                }
                if !*ctx.running {
                    ctx.agent.abort();
                    ctx.go_off();
                    return OutcomeResolve::Stopped;
                }
                ctx.picture.set_phase(Phase::AwaitingConfirm);
                if let Err(e) = hear::settle_after_confirm(ctx.mic, ctx.cmd_rx, ctx.running) {
                    tracing::warn!(error = ?e, turn = %ctx.turn, "confirm re-ask settle failed");
                }
                if let Err(e) = ctx.stt.load() {
                    tracing::warn!(error = %e, turn = %ctx.turn, "confirm re-ask stt load failed");
                }
                ctx.picture.set_phase(Phase::Hearing);
                ctx.picture.activity = Some("confirm · listening".into());
                ctx.picture.publish();
                // Deny-by-default on failure, but log the cause instead of
                // silently swallowing device + STT errors into "".
                let second = match hear::capture_utterance(
                    ctx.mic,
                    ctx.vad,
                    ctx.cmd_rx,
                    ctx.running,
                    CaptureKind::AwaitConfirm,
                ) {
                    Ok(c) => match ctx.stt.transcribe(&c) {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::warn!(error = %e, turn = %ctx.turn, "confirm re-ask STT failed — deny");
                            String::new()
                        }
                    },
                    Err(e) => {
                        tracing::warn!(error = ?e, turn = %ctx.turn, "confirm re-ask capture failed — deny");
                        String::new()
                    }
                };
                maybe_unload_stt(ctx.stt.as_mut(), ctx.turn, ctx.residency);
                ctx.picture.heard = Some(second.clone());
                ctx.picture.publish();
                interpret_yes_no(&second).unwrap_or(false)
            }
        };

        tracing::info!(turn = %ctx.turn, approved, heard = %heard, "confirm decision");
        ctx.picture.activity = Some("thinking…".into());
        ctx.picture.set_phase(Phase::Thinking);
        ctx.picture.detail = None;
        outcome = match resume_after_confirm(ctx, &pending.id, approved) {
            Ok(o) => o,
            Err(res) => return res,
        };
    }

    tracing::warn!(turn = %ctx.turn, "confirm loop cap — cancelling pending");
    ctx.agent.abort();
    ctx.picture.detail = Some("too many confirmations".into());
    ctx.picture.set_phase(Phase::Armed);
    OutcomeResolve::ReArm
}

fn collect_typed_input(
    ctx: &mut ConfirmCtx<'_>,
    prompt: String,
    pending: PendingToolCall,
) -> Result<AgentOutcome, OutcomeResolve> {
    let input = pending.input.clone().unwrap_or(boris_agent::PendingInput {
        kind: boris_agent::InputKind::Exact,
        label: "Exact text".into(),
        max_chars: 512,
    });
    tracing::info!(
        turn = %ctx.turn,
        id = %pending.id,
        kind = input.kind.as_str(),
        "agent needs typed input"
    );
    ctx.picture.detail = None;
    ctx.picture.activity = Some(format!("input · {}", input.label));
    ctx.picture.said = Some(prompt.clone());
    ctx.picture.input = Some(InputPeek {
        id: pending.id.clone(),
        kind: input.kind.as_str().into(),
        label: input.label.clone(),
        spoken: prompt.clone(),
        multiline: input.kind.multiline(),
        max_chars: input.max_chars,
    });
    // Flip the phase before TTS so the overlay reads "Your turn" and the
    // HWND parks on-screen. Waiting for playback first left the field up
    // while the snapshot was still Thinking.
    ctx.picture.set_phase(Phase::AwaitingInput);

    if ctx.tts.load().is_ok() {
        if let Ok(pcm) = ctx.tts.synthesize(&prompt) {
            drain_output_events(ctx.output_events);
            if let Err(e) = ctx.audio.play(pcm) {
                tracing::error!(error = %e, "typed-input prompt play failed");
            }
        }
    }
    if !*ctx.running {
        ctx.agent.abort();
        ctx.go_off();
        return Err(OutcomeResolve::Stopped);
    }

    let submitted = wait_typed_input(ctx, &pending.id)?;
    ctx.audio.stop();
    drain_output_events(ctx.output_events);
    ctx.picture.input = None;
    ctx.picture.activity = Some("thinking…".into());
    ctx.picture.set_phase(Phase::Thinking);

    let pending_id = pending.id.clone();
    match run_thinking(ThinkCtx {
        agent: ctx.agent,
        agent_rt: ctx.agent_rt,
        mic: ctx.mic,
        wake: ctx.wake,
        vad: ctx.vad,
        stt: ctx.stt,
        liveness: ctx.liveness,
        barge_in: ctx.barge_in,
        audio: ctx.audio,
        output_events: ctx.output_events,
        picture: ctx.picture,
        activity_events_enabled: ctx.activity_events_enabled.clone(),
        cmd_rx: ctx.cmd_rx,
        running: ctx.running,
        work: AgentWork::ResumeInput {
            pending_id: &pending_id,
            value: submitted,
        },
        turn: ctx.turn,
    }) {
        ThinkResolve::Finished(Ok((outcome, report))) => {
            ctx.picture.update_context(
                report.context_used_tokens,
                report.context_limit_tokens,
                report.context_estimated,
            );
            ctx.last_resume_report = Some(report);
            Ok(outcome)
        }
        ThinkResolve::Finished(Err(e)) if e.kind() == AgentErrorKind::Cancelled => {
            ctx.agent.abort();
            ctx.picture.clear_activity();
            ctx.picture.set_phase(Phase::Armed);
            Err(OutcomeResolve::ReArm)
        }
        ThinkResolve::Finished(Err(e)) => {
            tracing::error!(error = %e, "resume input failed");
            ctx.agent.abort();
            ctx.picture.detail = Some(format!("agent: {e}"));
            ctx.picture.set_phase(Phase::Armed);
            Err(OutcomeResolve::ReArm)
        }
        ThinkResolve::StopTurn => {
            ctx.agent.restore_checkpoint(ctx.turn_checkpoint.clone());
            ctx.picture.clear_activity();
            ctx.picture.set_phase(Phase::Armed);
            Err(OutcomeResolve::ReArm)
        }
        ThinkResolve::TakeTurn(text) => {
            ctx.agent.restore_checkpoint(ctx.turn_checkpoint.clone());
            ctx.picture.clear_activity();
            Err(OutcomeResolve::TakeTurn {
                user_text: text,
                interrupted_text: ctx.original_heard.clone(),
            })
        }
        ThinkResolve::Stopped => {
            ctx.agent.abort();
            ctx.go_off();
            Err(OutcomeResolve::Stopped)
        }
    }
}

fn wait_typed_input(
    ctx: &mut ConfirmCtx<'_>,
    pending_id: &str,
) -> Result<Option<String>, OutcomeResolve> {
    loop {
        if !*ctx.running {
            ctx.agent.abort();
            ctx.go_off();
            return Err(OutcomeResolve::Stopped);
        }
        match ctx
            .cmd_rx
            .recv_timeout(std::time::Duration::from_millis(40))
        {
            Ok(EngineCommand::SubmitInput { id, value }) if id == pending_id => {
                return Ok(Some(value));
            }
            Ok(EngineCommand::CancelInput { id }) if id == pending_id => {
                return Ok(None);
            }
            // Stale submit/cancel for a previous turn's pending id: drop with
            // a log, never apply to this turn's pending call.
            Ok(EngineCommand::SubmitInput { id, .. } | EngineCommand::CancelInput { id }) => {
                tracing::debug!(got = %id, want = %pending_id, "typed input for stale pending id — dropped");
            }
            Ok(EngineCommand::Stop) | Ok(EngineCommand::Shutdown) => {
                *ctx.running = false;
                ctx.agent.abort();
                ctx.go_off();
                return Err(OutcomeResolve::Stopped);
            }
            Ok(EngineCommand::Start) => *ctx.running = true,
            Ok(EngineCommand::SwitchInput { device_id }) => {
                apply_input_switch_from_outcome(ctx, &device_id);
            }
            Ok(EngineCommand::SwitchOutput { device_id }) => {
                super::device_switch::apply_output_switch(
                    ctx.audio,
                    ctx.output_events,
                    ctx.picture,
                    &device_id,
                );
            }
            Ok(EngineCommand::StartWakeEnroll { .. } | EngineCommand::ClearWakeProfile) => {
                tracing::debug!("wake enroll command ignored while waiting for typed input");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                *ctx.running = false;
                ctx.agent.abort();
                ctx.picture.mark_devices_dead();
                ctx.go_off();
                return Err(OutcomeResolve::Stopped);
            }
        }
    }
}

fn apply_input_switch_from_outcome(ctx: &mut ConfirmCtx<'_>, device_id: &str) {
    super::device_switch::apply_input_switch(ctx.audio, ctx.picture, device_id);
}

fn resume_after_confirm(
    ctx: &mut ConfirmCtx<'_>,
    pending_id: &str,
    approved: bool,
) -> Result<AgentOutcome, OutcomeResolve> {
    match run_thinking(ThinkCtx {
        agent: ctx.agent,
        agent_rt: ctx.agent_rt,
        mic: ctx.mic,
        wake: ctx.wake,
        vad: ctx.vad,
        stt: ctx.stt,
        liveness: ctx.liveness,
        barge_in: ctx.barge_in,
        audio: ctx.audio,
        output_events: ctx.output_events,
        picture: ctx.picture,
        activity_events_enabled: ctx.activity_events_enabled.clone(),
        cmd_rx: ctx.cmd_rx,
        running: ctx.running,
        work: AgentWork::Resume {
            pending_id,
            approved,
        },
        turn: ctx.turn,
    }) {
        ThinkResolve::Finished(Ok((outcome, report))) => {
            ctx.picture.update_context(
                report.context_used_tokens,
                report.context_limit_tokens,
                report.context_estimated,
            );
            ctx.last_resume_report = Some(report);
            Ok(outcome)
        }
        ThinkResolve::Finished(Err(e)) if e.kind() == AgentErrorKind::Cancelled => {
            ctx.agent.abort();
            ctx.picture.clear_activity();
            ctx.picture.set_phase(Phase::Armed);
            Err(OutcomeResolve::ReArm)
        }
        ThinkResolve::Finished(Err(e)) => {
            tracing::error!(error = %e, "resume confirmation failed");
            ctx.agent.abort();
            ctx.picture.detail = Some(format!("agent: {e}"));
            ctx.picture.set_phase(Phase::Armed);
            Err(OutcomeResolve::ReArm)
        }
        ThinkResolve::StopTurn => {
            ctx.agent.restore_checkpoint(ctx.turn_checkpoint.clone());
            ctx.picture.clear_activity();
            ctx.picture.set_phase(Phase::Armed);
            Err(OutcomeResolve::ReArm)
        }
        ThinkResolve::TakeTurn(text) => {
            ctx.agent.restore_checkpoint(ctx.turn_checkpoint.clone());
            ctx.picture.clear_activity();
            Err(OutcomeResolve::TakeTurn {
                user_text: text,
                interrupted_text: ctx.original_heard.clone(),
            })
        }
        ThinkResolve::Stopped => {
            ctx.agent.abort();
            ctx.go_off();
            Err(OutcomeResolve::Stopped)
        }
    }
}

fn confirm_activity(name: &str, args_summary: &str) -> String {
    let phrase = boris_agent::describe_tool(name, args_summary, boris_agent::Tense::Present);
    format!("confirm · {phrase}")
}
