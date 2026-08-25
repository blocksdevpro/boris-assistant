//! Barge-in while the agent is still working.
//!
//! The turn runs on a scoped thread so this engine thread can score wake and
//! liveness. Work keeps running until STT decides. Silence, a rejected
//! speaker, or STT failure is a no-op.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use boris_agent::{Agent, AgentError, AgentOutcome, TurnReport};
use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;
use boris_core::{ArcAudioBuffer, TurnId};
use boris_sense::{SileroVad, WakeWord};

use crate::hear::{self, CaptureKind, HearBreak};
use crate::liveness::{WakeLiveness, WakeOrigin};
use crate::status::Phase;

use super::barge::{decide_thinking_barge_listen, BargeDecision, BargeWatch};
use super::device_switch::{apply_input_switch, apply_output_switch};
use super::models::SttBox;
use super::picture::Picture;
use super::playback::poll_running;
use super::EngineCommand;

const THINK_POLL: Duration = Duration::from_millis(15);
const REJECT_DRAIN_MS: u64 = 150;

enum AgentJob {
    Prompt(String),
    Resume {
        pending_id: String,
        approved: bool,
    },
    ResumeInput {
        pending_id: String,
        value: Option<String>,
    },
}

pub(super) enum ThinkResolve {
    Finished(Result<(AgentOutcome, TurnReport), AgentError>),
    StopTurn,
    TakeTurn(String),
    Stopped,
}

/// What the scoped agent thread should run while the engine watches the mic.
pub(super) enum AgentWork<'a> {
    Prompt(&'a str),
    Resume {
        pending_id: &'a str,
        approved: bool,
    },
    ResumeInput {
        pending_id: &'a str,
        value: Option<String>,
    },
}

pub(super) struct ThinkCtx<'a> {
    pub agent: &'a mut Agent,
    pub agent_rt: &'a tokio::runtime::Runtime,
    pub mic: &'a crossbeam_channel::Receiver<ArcAudioBuffer>,
    pub wake: &'a mut dyn WakeWord,
    pub vad: &'a mut SileroVad,
    pub stt: &'a mut SttBox,
    pub liveness: &'a mut WakeLiveness,
    pub barge_in: bool,
    pub audio: &'a mut AudioService,
    pub output_events: &'a mut crossbeam_channel::Receiver<OutputEvent>,
    pub picture: &'a mut Picture,
    pub cmd_rx: &'a Receiver<EngineCommand>,
    pub running: &'a mut bool,
    pub work: AgentWork<'a>,
    pub turn: TurnId,
}

pub(super) fn run_thinking(ctx: ThinkCtx<'_>) -> ThinkResolve {
    let ThinkCtx {
        agent,
        agent_rt,
        mic,
        wake,
        vad,
        stt,
        liveness,
        barge_in,
        audio,
        output_events,
        picture,
        cmd_rx,
        running,
        work,
        turn,
    } = ctx;

    let handle = agent_rt.handle().clone();
    let cancel = agent.arm_cancel();
    let job = match work {
        AgentWork::Prompt(text) => AgentJob::Prompt(text.to_string()),
        AgentWork::Resume {
            pending_id,
            approved,
        } => AgentJob::Resume {
            pending_id: pending_id.to_string(),
            approved,
        },
        AgentWork::ResumeInput { pending_id, value } => AgentJob::ResumeInput {
            pending_id: pending_id.to_string(),
            value,
        },
    };
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let agent_done = std::sync::Arc::new(AtomicBool::new(false));

    thread::scope(|s| {
        let agent_done_w = agent_done.clone();
        s.spawn(move || {
            let result = handle.block_on(async move {
                match job {
                    AgentJob::Prompt(text) => agent.prompt_with_report(&text).await,
                    AgentJob::Resume {
                        pending_id,
                        approved,
                    } => {
                        agent
                            .resume_confirmation_with_report(&pending_id, approved)
                            .await
                    }
                    AgentJob::ResumeInput { pending_id, value } => {
                        agent.resume_input_with_report(&pending_id, value).await
                    }
                }
            });
            agent_done_w.store(true, Ordering::Release);
            let _ = done_tx.send(result);
        });

        let mut watch = if barge_in {
            Some(BargeWatch::new(mic, wake))
        } else {
            None
        };

        loop {
            let poll = poll_running(cmd_rx, running, audio, output_events, picture);
            if !poll.running {
                tracing::info!(%turn, "stop during thinking — cancelling turn");
                cancel.cancel();
                let _ = recv_done(&done_rx);
                return ThinkResolve::Stopped;
            }

            match done_rx.try_recv() {
                Ok(result) => return ThinkResolve::Finished(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return ThinkResolve::Finished(Err(AgentError::cancelled("agent thread gone")));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }

            if let Some(watch) = watch.as_mut() {
                if let Some(window) = watch.poll_wake_only() {
                    watch.reset();
                    match on_thinking_wake(
                        &window,
                        &agent_done,
                        mic,
                        vad,
                        stt,
                        liveness,
                        cmd_rx,
                        running,
                        picture,
                        turn,
                    ) {
                        Ok(BargeDecision::Resume) => {}
                        Ok(BargeDecision::StopTalking) => {
                            tracing::info!(%turn, "thinking barge-in stop");
                            cancel.cancel();
                            let _ = recv_done(&done_rx);
                            return ThinkResolve::StopTurn;
                        }
                        Ok(BargeDecision::TakeTurn(next)) => {
                            tracing::info!(%turn, text = %next, "thinking barge-in new turn");
                            cancel.cancel();
                            let _ = recv_done(&done_rx);
                            return ThinkResolve::TakeTurn(next);
                        }
                        Err(HearBreak::Stopped) if !*running => {
                            cancel.cancel();
                            let _ = recv_done(&done_rx);
                            return ThinkResolve::Stopped;
                        }
                        Err(HearBreak::Stopped) => {
                            cancel.cancel();
                            let _ = recv_done(&done_rx);
                            return ThinkResolve::StopTurn;
                        }
                        Err(HearBreak::Disconnected) => {
                            cancel.cancel();
                            let _ = recv_done(&done_rx);
                            return ThinkResolve::Stopped;
                        }
                        Err(HearBreak::SwitchInput { device_id }) => {
                            apply_input_switch(audio, picture, &device_id);
                        }
                        Err(HearBreak::SwitchOutput { device_id }) => {
                            apply_output_switch(audio, output_events, picture, &device_id);
                        }
                        Err(HearBreak::StartWakeEnroll { .. } | HearBreak::ClearWakeProfile) => {}
                    }
                }
            }

            thread::sleep(THINK_POLL);
        }
    })
}

fn recv_done(
    done_rx: &mpsc::Receiver<Result<(AgentOutcome, TurnReport), AgentError>>,
) -> Result<(AgentOutcome, TurnReport), AgentError> {
    done_rx
        .recv()
        .unwrap_or_else(|_| Err(AgentError::cancelled("agent thread gone")))
}

fn on_thinking_wake(
    window: &[f32],
    agent_done: &AtomicBool,
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    vad: &mut SileroVad,
    stt: &mut SttBox,
    liveness: &mut WakeLiveness,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    picture: &mut Picture,
    turn: TurnId,
) -> Result<BargeDecision, HearBreak> {
    let crop = hear::crop_speech(vad, window);
    let origin = liveness.classify(&crop.pcm, crop.speech_hops);
    match origin {
        WakeOrigin::Live | WakeOrigin::Unknown => {}
        other => {
            tracing::info!(%turn, ?other, "thinking barge-in rejected by liveness");
            hear::drain_ms(mic, cmd_rx, running, REJECT_DRAIN_MS)?;
            return Ok(BargeDecision::Resume);
        }
    }

    let prev_activity = picture.activity.clone();
    picture.activity = Some("barge-in · listening".into());
    picture.publish();

    if let Err(e) = stt.load() {
        tracing::warn!(error = %e, %turn, "thinking barge-in stt load failed — resume");
        return Ok(BargeDecision::Resume);
    }

    let mut pcm = if crop.pcm.is_empty() {
        window.to_vec()
    } else {
        crop.pcm.clone()
    };
    let mut hops = crop.speech_hops;

    if !agent_done.load(Ordering::Acquire) {
        let clip = hear::capture_utterance_until(
            mic,
            vad,
            cmd_rx,
            running,
            CaptureKind::AfterWake,
            || agent_done.load(Ordering::Acquire),
        )?;
        let follow = hear::crop_speech(vad, &clip);
        if follow.speech_hops > 0 {
            pcm.extend_from_slice(&follow.pcm);
            hops = hops.saturating_add(follow.speech_hops);
        }
    }

    let text = transcribe_or_empty(stt, &pcm, turn);

    if picture
        .activity
        .as_deref()
        .is_some_and(|a| a.starts_with("barge-in"))
    {
        picture.activity = prev_activity
            .filter(|a| !a.starts_with("barge-in"))
            .or_else(|| {
                if picture.phase == Phase::Thinking {
                    Some("thinking…".into())
                } else {
                    None
                }
            });
        picture.publish();
    }

    let decision = decide_thinking_barge_listen(hops, &text);
    tracing::info!(%turn, %text, ?decision, "thinking barge-in heard");
    Ok(decision)
}

fn transcribe_or_empty(stt: &mut SttBox, pcm: &[f32], turn: TurnId) -> String {
    if pcm.is_empty() {
        return String::new();
    }
    match stt.transcribe(pcm) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, %turn, "thinking barge-in stt failed — resume");
            String::new()
        }
    }
}
