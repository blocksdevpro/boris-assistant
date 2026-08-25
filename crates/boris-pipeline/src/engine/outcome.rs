//! HITL confirmation path: speak prompt → freeform yes/no → resume agent.
//!
//! Called from the main turn loop after `agent.prompt_with_report` returns
//! [`AgentOutcome::NeedsConfirmation`]. Nested confirms are capped (also in agent policy).

use std::sync::mpsc::Receiver;

use boris_agent::session::store::SessionStore;
use boris_agent::session::types::SessionId;
use boris_agent::{Agent, AgentErrorKind, AgentOutcome, PendingToolCall};
use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;
use boris_core::{ArcAudioBuffer, TurnId};
use boris_sense::{SileroVad, WakeWord};

use crate::hear::{self, CaptureKind, HearBreak};
use crate::liveness::WakeLiveness;
use crate::status::{InputPeek, Phase};

use super::barge::thinking_takeover_text;
use super::confirm::interpret_yes_no;
use super::models::{release_voice_models, SttBox, TtsBox};
use super::picture::Picture;
use super::playback::{wait_playback_or_stop, wait_playback_started, PlaybackWait};
use super::session::{end_session, go_off};
use super::think::{run_thinking, AgentWork, ThinkCtx, ThinkResolve};
use super::EngineCommand;

pub(super) enum OutcomeResolve {
    Done(AgentOutcome),
    ReArm,
    Stopped,
    TakeTurn(String),
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
    pub store: &'a SessionStore,
    pub active_session: &'a mut Option<SessionId>,
    pub transcript_len: &'a mut usize,
    pub original_heard: Option<String>,
    pub turn: TurnId,
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
    for _ in 0..8 {
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
            return OutcomeResolve::Done(outcome);
        };

        tracing::info!(
            turn = %ctx.turn,
            tool = %pending.name,
            pending_id = %pending.id,
            "agent needs confirmation"
        );
        // Never put confirm text in `detail` (overlay treats detail as error).
        // Drop prior-turn `heard` so the UI does not show the last STT line as
        // “You” while waiting for yes/no (fresh answer is written after STT).
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
                let _ = ctx.tts.unload();
                ctx.picture.detail = Some(format!("tts: {e}"));
                ctx.picture.set_phase(Phase::Armed);
                return OutcomeResolve::ReArm;
            }
        };
        while ctx.output_events.try_recv().is_ok() {}
        // UI: show confirm context while Boris is speaking so the user knows
        // a yes/no is coming (not a freeform reply).
        ctx.picture.set_phase(Phase::AwaitingConfirm);
        if let Err(e) = ctx.audio.play(pcm) {
            tracing::error!(error = %e, "confirm prompt play failed");
        }
        match wait_playback_started(
            ctx.output_events,
            ctx.cmd_rx,
            ctx.running,
            ctx.audio,
            ctx.picture,
        ) {
            PlaybackWait::Stopped => {
                ctx.agent.abort();
                ctx.go_off();
                return OutcomeResolve::Stopped;
            }
            PlaybackWait::Aborted | PlaybackWait::BargedIn => {}
            PlaybackWait::Finished => {
                ctx.picture.set_phase(Phase::Talking);
                // Keep activity as confirm so overlay still reads as yes/no.
                ctx.picture.activity = Some(confirm_activity(&pending.name, &pending.args_summary));
                match wait_playback_or_stop(
                    ctx.output_events,
                    ctx.cmd_rx,
                    ctx.running,
                    ctx.audio,
                    ctx.picture,
                ) {
                    PlaybackWait::Finished => {}
                    PlaybackWait::Stopped => {
                        ctx.agent.abort();
                        ctx.go_off();
                        return OutcomeResolve::Stopped;
                    }
                    PlaybackWait::Aborted | PlaybackWait::BargedIn => {
                        ctx.agent.abort();
                        ctx.picture.detail = Some("confirmation playback interrupted".into());
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
                    ctx.picture.set_phase(Phase::Off);
                    OutcomeResolve::Stopped
                }
                _ => {
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
            Err(_) => {
                // Silence: re-prompt once instead of silently rejecting.
                tracing::info!(turn = %ctx.turn, "confirm capture empty — re-prompt");
                let reask = "I need a yes or no on that.";
                ctx.picture.said = Some(reask.into());
                ctx.picture.activity = Some("confirm · say yes or no".into());
                ctx.picture.publish();
                if let Ok(pcm) = ctx.tts.synthesize(reask) {
                    while ctx.output_events.try_recv().is_ok() {}
                    if let Err(e) = ctx.audio.play(pcm) {
                        tracing::warn!(error = %e, "confirm reask play failed");
                    }
                    let _ = wait_playback_started(
                        ctx.output_events,
                        ctx.cmd_rx,
                        ctx.running,
                        ctx.audio,
                        ctx.picture,
                    );
                    wait_playback_or_stop(
                        ctx.output_events,
                        ctx.cmd_rx,
                        ctx.running,
                        ctx.audio,
                        ctx.picture,
                    );
                }
                if !*ctx.running {
                    ctx.agent.abort();
                    ctx.go_off();
                    return OutcomeResolve::Stopped;
                }
                let _ = hear::settle_after_confirm(ctx.mic, ctx.cmd_rx, ctx.running);
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
                        outcome = match ctx
                            .agent_rt
                            .block_on(ctx.agent.resume_confirmation(&pending.id, false))
                        {
                            Ok(o) => o,
                            Err(e) => {
                                tracing::error!(error = %e, "resume reject failed");
                                ctx.agent.abort();
                                ctx.picture.detail = Some(format!("agent: {e}"));
                                ctx.picture.clear_activity();
                                ctx.picture.set_phase(Phase::Armed);
                                return OutcomeResolve::ReArm;
                            }
                        };
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
        let _ = ctx.stt.unload();
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
                    while ctx.output_events.try_recv().is_ok() {}
                    if let Err(e) = ctx.audio.play(pcm) {
                        tracing::warn!(error = %e, "confirm reask play failed");
                    }
                    let _ = wait_playback_started(
                        ctx.output_events,
                        ctx.cmd_rx,
                        ctx.running,
                        ctx.audio,
                        ctx.picture,
                    );
                    wait_playback_or_stop(
                        ctx.output_events,
                        ctx.cmd_rx,
                        ctx.running,
                        ctx.audio,
                        ctx.picture,
                    );
                }
                if !*ctx.running {
                    ctx.agent.abort();
                    ctx.go_off();
                    return OutcomeResolve::Stopped;
                }
                ctx.picture.set_phase(Phase::AwaitingConfirm);
                let _ = hear::settle_after_confirm(ctx.mic, ctx.cmd_rx, ctx.running);
                let _ = ctx.stt.load();
                ctx.picture.set_phase(Phase::Hearing);
                ctx.picture.activity = Some("confirm · listening".into());
                ctx.picture.publish();
                let second = hear::capture_utterance(
                    ctx.mic,
                    ctx.vad,
                    ctx.cmd_rx,
                    ctx.running,
                    CaptureKind::AwaitConfirm,
                )
                .ok()
                .and_then(|c| ctx.stt.transcribe(&c).ok())
                .unwrap_or_default();
                let _ = ctx.stt.unload();
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
            while ctx.output_events.try_recv().is_ok() {}
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
    while ctx.output_events.try_recv().is_ok() {}
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
        cmd_rx: ctx.cmd_rx,
        running: ctx.running,
        work: AgentWork::ResumeInput {
            pending_id: &pending_id,
            value: submitted,
        },
        turn: ctx.turn,
    }) {
        ThinkResolve::Finished(Ok((outcome, _))) => Ok(outcome),
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
            ctx.agent.abort();
            ctx.picture.clear_activity();
            ctx.picture.set_phase(Phase::Armed);
            Err(OutcomeResolve::ReArm)
        }
        ThinkResolve::TakeTurn(text) => {
            ctx.agent.abort();
            ctx.picture.clear_activity();
            Err(OutcomeResolve::TakeTurn(thinking_takeover_text(
                ctx.original_heard.as_deref(),
                &text,
            )))
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
            Ok(EngineCommand::SubmitInput { .. } | EngineCommand::CancelInput { .. }) => {}
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
            Ok(EngineCommand::StartWakeEnroll { .. } | EngineCommand::ClearWakeProfile) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                *ctx.running = false;
                ctx.agent.abort();
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
        cmd_rx: ctx.cmd_rx,
        running: ctx.running,
        work: AgentWork::Resume {
            pending_id,
            approved,
        },
        turn: ctx.turn,
    }) {
        ThinkResolve::Finished(Ok((outcome, _))) => Ok(outcome),
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
            ctx.agent.abort();
            ctx.picture.clear_activity();
            ctx.picture.set_phase(Phase::Armed);
            Err(OutcomeResolve::ReArm)
        }
        ThinkResolve::TakeTurn(text) => {
            ctx.agent.abort();
            ctx.picture.clear_activity();
            Err(OutcomeResolve::TakeTurn(thinking_takeover_text(
                ctx.original_heard.as_deref(),
                &text,
            )))
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
