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

/// Drive NeedsConfirmation → speak → freeform yes/no → resume until Speak/Silent.
pub(super) fn resolve_agent_outcome(
    mut outcome: AgentOutcome,
    agent: &mut Agent,
    agent_rt: &tokio::runtime::Runtime,
    tts: &mut TtsBox,
    stt: &mut SttBox,
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    vad: &mut impl boris_sense::Vad,
    audio: &mut AudioService,
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    picture: &mut Picture,
    store: &SessionStore,
    active_session: &mut Option<SessionId>,
    transcript_len: &mut usize,
    turn: TurnId,
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
            %turn,
            tool = %pending.name,
            pending_id = %pending.id,
            "agent needs confirmation"
        );
        // Never put confirm text in `detail` (overlay treats detail as error).
        picture.detail = None;
        picture.activity = Some(format!("confirm · {}", pending.name));
        picture.said = Some(prompt.clone());
        picture.publish();

        // Speak the confirm prompt.
        if let Err(e) = tts.load() {
            tracing::error!(error = %e, "tts load failed for confirm");
            agent.cancel_pending();
            picture.detail = Some(format!("tts: {e}"));
            picture.set_phase(Phase::Armed);
            return OutcomeResolve::ReArm;
        }
        let pcm = match tts.synthesize(&prompt) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "tts synth failed for confirm");
                agent.cancel_pending();
                let _ = tts.unload();
                picture.detail = Some(format!("tts: {e}"));
                picture.set_phase(Phase::Armed);
                return OutcomeResolve::ReArm;
            }
        };
        while output_events.try_recv().is_ok() {}
        audio.play(pcm);
        match wait_playback_started(output_events, cmd_rx, running, audio, picture) {
            PlaybackWait::Stopped => {
                agent.cancel_pending();
                go_off(
                    picture,
                    audio,
                    store,
                    active_session,
                    transcript_len,
                    stt.as_mut(),
                    tts.as_mut(),
                );
                return OutcomeResolve::Stopped;
            }
            PlaybackWait::Aborted => {}
            PlaybackWait::Finished => {
                picture.set_phase(Phase::Talking);
                wait_playback_or_stop(output_events, cmd_rx, running, audio, picture);
            }
        }
        if !*running {
            agent.cancel_pending();
            go_off(
                picture,
                audio,
                store,
                active_session,
                transcript_len,
                stt.as_mut(),
                tts.as_mut(),
            );
            return OutcomeResolve::Stopped;
        }

        // Listen for yes/no with longer post-TTS settle + confirm-specific VAD.
        picture.set_phase(Phase::AwaitingConfirm);
        if let Err(e) = hear::settle_after_confirm(mic, cmd_rx, running) {
            agent.cancel_pending();
            picture.clear_activity();
            return match e {
                HearBreak::Stopped if !*running => {
                    go_off(
                        picture,
                        audio,
                        store,
                        active_session,
                        transcript_len,
                        stt.as_mut(),
                        tts.as_mut(),
                    );
                    OutcomeResolve::Stopped
                }
                HearBreak::Disconnected => {
                    end_session(store, active_session, transcript_len);
                    release_voice_models(stt.as_mut(), tts.as_mut(), "disconnected");
                    picture.set_phase(Phase::Off);
                    OutcomeResolve::Stopped
                }
                _ => {
                    picture.set_phase(Phase::Armed);
                    OutcomeResolve::ReArm
                }
            };
        }

        if let Err(e) = stt.load() {
            tracing::error!(error = %e, "stt load failed for confirm");
            agent.cancel_pending();
            picture.detail = Some(format!("stt: {e}"));
            picture.clear_activity();
            picture.set_phase(Phase::Armed);
            return OutcomeResolve::ReArm;
        }

        picture.set_phase(Phase::Hearing);
        let clip =
            match hear::capture_utterance(mic, vad, cmd_rx, running, CaptureKind::AwaitConfirm) {
                Ok(c) => c,
                Err(HearBreak::Stopped) if !*running => {
                    agent.cancel_pending();
                    go_off(
                        picture,
                        audio,
                        store,
                        active_session,
                        transcript_len,
                        stt.as_mut(),
                        tts.as_mut(),
                    );
                    return OutcomeResolve::Stopped;
                }
                Err(_) => {
                    // Silence: re-prompt once instead of silently rejecting.
                    tracing::info!(%turn, "confirm capture empty — re-prompt");
                    let reask = "I need a yes or no on that.";
                    picture.said = Some(reask.into());
                    picture.activity = Some("confirm · say yes or no".into());
                    picture.publish();
                    if let Ok(pcm) = tts.synthesize(reask) {
                        while output_events.try_recv().is_ok() {}
                        audio.play(pcm);
                        let _ =
                            wait_playback_started(output_events, cmd_rx, running, audio, picture);
                        wait_playback_or_stop(output_events, cmd_rx, running, audio, picture);
                    }
                    if !*running {
                        agent.cancel_pending();
                        go_off(
                            picture,
                            audio,
                            store,
                            active_session,
                            transcript_len,
                            stt.as_mut(),
                            tts.as_mut(),
                        );
                        return OutcomeResolve::Stopped;
                    }
                    let _ = hear::settle_after_confirm(mic, cmd_rx, running);
                    picture.set_phase(Phase::Hearing);
                    match hear::capture_utterance(
                        mic,
                        vad,
                        cmd_rx,
                        running,
                        CaptureKind::AwaitConfirm,
                    ) {
                        Ok(c) => c,
                        Err(_) => {
                            tracing::info!(%turn, "confirm second capture failed — reject");
                            outcome = match agent_rt
                                .block_on(agent.resume_confirmation(&pending.id, false))
                            {
                                Ok(o) => o,
                                Err(e) => {
                                    tracing::error!(error = %e, "resume reject failed");
                                    agent.cancel_pending();
                                    picture.detail = Some(format!("agent: {e}"));
                                    picture.clear_activity();
                                    picture.set_phase(Phase::Armed);
                                    return OutcomeResolve::ReArm;
                                }
                            };
                            picture.activity = Some("thinking…".into());
                            picture.set_phase(Phase::Thinking);
                            continue;
                        }
                    }
                }
            };

        picture.set_phase(Phase::Reading);
        let heard = match stt.transcribe(&clip) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "confirm STT failed");
                String::new()
            }
        };
        let _ = stt.unload();
        picture.heard = Some(heard.clone());
        picture.publish();

        let approved = match interpret_yes_no(&heard) {
            Some(v) => v,
            None => {
                tracing::info!(%turn, heard = %heard, "confirm answer ambiguous — re-ask once");
                let reask = "Was that a yes or a no?";
                picture.said = Some(reask.into());
                picture.activity = Some("confirm · yes or no".into());
                picture.publish();
                if let Ok(pcm) = tts.synthesize(reask) {
                    while output_events.try_recv().is_ok() {}
                    audio.play(pcm);
                    let _ = wait_playback_started(output_events, cmd_rx, running, audio, picture);
                    wait_playback_or_stop(output_events, cmd_rx, running, audio, picture);
                }
                if !*running {
                    agent.cancel_pending();
                    go_off(
                        picture,
                        audio,
                        store,
                        active_session,
                        transcript_len,
                        stt.as_mut(),
                        tts.as_mut(),
                    );
                    return OutcomeResolve::Stopped;
                }
                picture.set_phase(Phase::AwaitingConfirm);
                let _ = hear::settle_after_confirm(mic, cmd_rx, running);
                let _ = stt.load();
                picture.set_phase(Phase::Hearing);
                let second =
                    hear::capture_utterance(mic, vad, cmd_rx, running, CaptureKind::AwaitConfirm)
                        .ok()
                        .and_then(|c| stt.transcribe(&c).ok())
                        .unwrap_or_default();
                let _ = stt.unload();
                picture.heard = Some(second.clone());
                picture.publish();
                interpret_yes_no(&second).unwrap_or(false)
            }
        };

        tracing::info!(%turn, approved, heard = %heard, "confirm decision");
        picture.activity = Some("thinking…".into());
        picture.set_phase(Phase::Thinking);
        picture.detail = None;
        outcome = match agent_rt.block_on(agent.resume_confirmation(&pending.id, approved)) {
            Ok(o) => o,
            Err(e) => {
                tracing::error!(error = %e, "resume confirmation failed");
                agent.cancel_pending();
                picture.detail = Some(format!("agent: {e}"));
                picture.set_phase(Phase::Armed);
                return OutcomeResolve::ReArm;
            }
        };
    }

    tracing::warn!(%turn, "confirm loop cap — cancelling pending");
    agent.cancel_pending();
    picture.detail = Some("too many confirmations".into());
    picture.set_phase(Phase::Armed);
    OutcomeResolve::ReArm
}
