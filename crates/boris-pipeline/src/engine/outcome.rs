//! HITL confirmation path: speak prompt → freeform yes/no → resume agent.
//!
//! Called from the main turn loop after `agent.prompt_with_report` returns
//! [`AgentOutcome::NeedsConfirmation`]. Nested confirms are capped (also in agent policy).

use std::sync::mpsc::Receiver;

use boris_agent::session::store::SessionStore;
use boris_agent::session::types::SessionId;
use boris_agent::{Agent, AgentOutcome};
use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;
use boris_core::{ArcAudioBuffer, TurnId};
use boris_sense::SileroVad;

use crate::hear::{self, CaptureKind, HearBreak};
use crate::status::Phase;

use super::confirm::interpret_yes_no;
use super::models::{release_voice_models, SttBox, TtsBox};
use super::picture::Picture;
use super::playback::{wait_playback_or_stop, wait_playback_started, PlaybackWait};
use super::session::{end_session, go_off};
use super::EngineCommand;

pub(super) enum OutcomeResolve {
    Done(AgentOutcome),
    ReArm,
    Stopped,
}

/// Mutable + shared context for the confirm resolution loop (avoids 18-param functions).
pub(super) struct ConfirmCtx<'a> {
    pub agent: &'a mut Agent,
    pub agent_rt: &'a tokio::runtime::Runtime,
    pub tts: &'a mut TtsBox,
    pub stt: &'a mut SttBox,
    pub mic: &'a crossbeam_channel::Receiver<ArcAudioBuffer>,
    pub vad: &'a mut SileroVad,
    pub audio: &'a mut AudioService,
    pub output_events: &'a mut crossbeam_channel::Receiver<OutputEvent>,
    pub cmd_rx: &'a Receiver<EngineCommand>,
    pub running: &'a mut bool,
    pub picture: &'a mut Picture,
    pub store: &'a SessionStore,
    pub active_session: &'a mut Option<SessionId>,
    pub transcript_len: &'a mut usize,
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

/// Drive NeedsConfirmation → speak → freeform yes/no → resume until Speak/Silent.
pub(super) fn resolve_agent_outcome(
    mut outcome: AgentOutcome,
    ctx: &mut ConfirmCtx<'_>,
) -> OutcomeResolve {
    // Cap nested confirms (also enforced in agent policy).
    for _ in 0..4 {
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
        ctx.picture.activity = Some(format!("confirm · {}", pending.name));
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
                ctx.picture.activity = Some(format!("confirm · {}", pending.name));
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
        outcome = match ctx
            .agent_rt
            .block_on(ctx.agent.resume_confirmation(&pending.id, approved))
        {
            Ok(o) => o,
            Err(e) => {
                tracing::error!(error = %e, "resume confirmation failed");
                ctx.agent.abort();
                ctx.picture.detail = Some(format!("agent: {e}"));
                ctx.picture.set_phase(Phase::Armed);
                return OutcomeResolve::ReArm;
            }
        };
    }

    tracing::warn!(turn = %ctx.turn, "confirm loop cap — cancelling pending");
    ctx.agent.abort();
    ctx.picture.detail = Some("too many confirmations".into());
    ctx.picture.set_phase(Phase::Armed);
    OutcomeResolve::ReArm
}
