//! Barge-in while the agent is still working.
//!
//! The turn runs on a scoped thread so this engine thread can score wake and
//! liveness. Work keeps running until STT decides. Silence, a rejected
//! speaker, or STT failure is a no-op.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use boris_agent::{Agent, AgentError, AgentOutcome, TurnReport};
use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;
use boris_core::{ArcAudioBuffer, TurnId};
use boris_sense::{SileroVad, WakeWord};

use crate::hear::{self, CaptureKind, HearBreak};
use crate::liveness::{WakeLiveness, WakeOrigin};
use crate::status::Phase;

use super::barge::{
    decide_thinking_barge_listen, drain_mic, listen_after_pause, BargeDecision, BargeWatch,
};
use super::device_switch::{apply_input_switch, apply_output_switch};
use super::early_speech::SpeechFeed;
use super::models::{lost_tts, SttBox, TtsBox, TtsSlot};
use super::picture::Picture;
use super::playback::{poll_running, PlaybackWait};
use super::speech::{stream_reply, SpeechUnits, StreamedSpeech};
use super::util::speakable_reply_units;
use super::EngineCommand;

const THINK_POLL: Duration = Duration::from_millis(15);
const REJECT_DRAIN_MS: u64 = 150;

enum AgentJob {
    Prompt(String),
    Replacement {
        user_text: String,
        interrupted_text: Option<String>,
    },
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
    Replacement {
        user_text: &'a str,
        interrupted_text: Option<&'a str>,
    },
    Resume {
        pending_id: &'a str,
        approved: bool,
    },
    ResumeInput {
        pending_id: &'a str,
        value: Option<String>,
    },
}

/// Speech that may start before the agent turn returns (first Thinking pass
/// of a turn only; confirm/input resumes pass `None`).
pub(super) struct ThinkSpeech<'a> {
    pub feed: &'a mut SpeechFeed,
    pub tts: &'a mut TtsSlot,
    pub progress_line: bool,
    pub report: &'a mut EarlySpeech,
}

/// What early speech did during Thinking.
#[derive(Default)]
pub(super) struct EarlySpeech {
    /// The committed final answer played to the end. Its `tts` is a
    /// placeholder; the model went back to the slot.
    pub delivered: Option<StreamedSpeech>,
    /// When the delivered job started, measured from the start of Thinking.
    pub delivered_after_ms: u64,
    pub progress_spoken: bool,
    /// Anything (progress line or streamed answer) reached the speaker.
    pub audible: bool,
    /// When the progress line became audible, from the start of Thinking.
    pub progress_audio_ms: Option<u64>,
    /// When a streamed answer that was later cancelled became audible.
    pub cancelled_audio_ms: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SpeakKind {
    Answer,
    Progress,
}

enum SpeakStep {
    Continue,
    Resolve(ThinkResolve),
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
    /// Agent events publish frozen Thinking snapshots directly. Disable them
    /// while interruption capture owns the foreground UI.
    pub activity_events_enabled: Arc<AtomicBool>,
    pub cmd_rx: &'a Receiver<EngineCommand>,
    pub running: &'a mut bool,
    pub work: AgentWork<'a>,
    pub turn: TurnId,
    pub speech: Option<ThinkSpeech<'a>>,
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
        activity_events_enabled,
        cmd_rx,
        running,
        work,
        turn,
        mut speech,
    } = ctx;
    let think_started = Instant::now();

    let handle = agent_rt.handle().clone();
    let cancel = agent.arm_cancel();
    let job = match work {
        AgentWork::Prompt(text) => AgentJob::Prompt(text.to_string()),
        AgentWork::Replacement {
            user_text,
            interrupted_text,
        } => AgentJob::Replacement {
            user_text: user_text.to_string(),
            interrupted_text: interrupted_text.map(ToOwned::to_owned),
        },
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
                    AgentJob::Replacement {
                        user_text,
                        interrupted_text,
                    } => {
                        agent
                            .prompt_replacement_with_report(&user_text, interrupted_text.as_deref())
                            .await
                    }
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

            // Streamed answer first (even if the agent just finished, so a
            // committed answer is still played from here), then the one-time
            // progress line on long tool work.
            if let Some(sp) = speech.as_mut() {
                let agent_finished = agent_done.load(Ordering::Acquire);
                sp.feed.drain();
                let kind = if sp.feed.has_units() && sp.report.delivered.is_none() {
                    Some(SpeakKind::Answer)
                } else if sp.progress_line
                    && !agent_finished
                    && !sp.report.progress_spoken
                    && !sp.report.audible
                    && sp.feed.progress_due(Instant::now())
                {
                    Some(SpeakKind::Progress)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    if let Some(tts) = sp.tts.take_loaded() {
                        let step = speak_while_thinking(
                            kind,
                            tts,
                            sp,
                            &agent_done,
                            think_started,
                            SpeakEnv {
                                audio: &mut *audio,
                                output_events: &mut *output_events,
                                cmd_rx,
                                running: &mut *running,
                                picture: &mut *picture,
                                mic,
                                vad: &mut *vad,
                                stt: &mut *stt,
                                watch: watch.as_mut(),
                                turn,
                            },
                        );
                        match step {
                            SpeakStep::Continue => {}
                            SpeakStep::Resolve(resolve) => {
                                cancel.cancel();
                                let _ = recv_done(&done_rx);
                                return resolve;
                            }
                        }
                    }
                }
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
                    // From this point until classification, the microphone is
                    // the foreground interaction. Frozen tool/thought events
                    // from the old agent turn must not repaint Thinking over it.
                    activity_events_enabled.store(false, Ordering::Release);
                    let heard = on_thinking_wake(
                        &window, mic, vad, stt, liveness, cmd_rx, running, picture, turn,
                    );
                    match heard {
                        Ok(BargeDecision::Resume) => {
                            activity_events_enabled.store(true, Ordering::Release);
                        }
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
                            activity_events_enabled.store(true, Ordering::Release);
                        }
                        Err(HearBreak::SwitchOutput { device_id }) => {
                            apply_output_switch(audio, output_events, picture, &device_id);
                            activity_events_enabled.store(true, Ordering::Release);
                        }
                        Err(
                            HearBreak::StartWakeEnroll { .. }
                            | HearBreak::ClearWakeProfile
                            | HearBreak::RunSelfCheck,
                        ) => {
                            activity_events_enabled.store(true, Ordering::Release);
                        }
                    }
                }
            }

            thread::sleep(THINK_POLL);
        }
    })
}

struct SpeakEnv<'e, 'w> {
    audio: &'e mut AudioService,
    output_events: &'e mut crossbeam_channel::Receiver<OutputEvent>,
    cmd_rx: &'e Receiver<EngineCommand>,
    running: &'e mut bool,
    picture: &'e mut Picture,
    mic: &'e crossbeam_channel::Receiver<ArcAudioBuffer>,
    vad: &'e mut SileroVad,
    stt: &'e mut SttBox,
    watch: Option<&'e mut BargeWatch<'w>>,
    turn: TurnId,
}

/// Play the streamed answer or the progress line while the agent keeps
/// working. The job uses the Talking barge watch (lower wake threshold plus
/// close-talk energy, AEC on the reference) exactly like normal replies, so
/// Boris's own playback is treated the same way and a barge-in still pauses
/// playback at once.
fn speak_while_thinking(
    kind: SpeakKind,
    tts: TtsBox,
    sp: &mut ThinkSpeech<'_>,
    agent_done: &AtomicBool,
    think_started: Instant,
    env: SpeakEnv<'_, '_>,
) -> SpeakStep {
    let SpeakEnv {
        audio,
        output_events,
        cmd_rx,
        running,
        picture,
        mic,
        vad,
        stt,
        mut watch,
        turn,
    } = env;
    let gap_samples = tts.inter_unit_silence_samples();
    let started_after_ms = think_started.elapsed().as_millis() as u64;
    let mut progress_units = match kind {
        SpeakKind::Answer => Vec::new(),
        SpeakKind::Progress => {
            sp.report.progress_spoken = true;
            let line = sp.feed.progress_text();
            tracing::info!(%turn, %line, "speaking progress line during tool work");
            speakable_reply_units(&line)
        }
    };
    if let Some(w) = watch.as_deref_mut() {
        w.reset();
    }
    let prev_activity = picture.activity.clone();
    let mut tts = tts;
    let mut already_audible = false;
    let step = loop {
        let units = match kind {
            SpeakKind::Answer => SpeechUnits::Feed {
                feed: &mut *sp.feed,
                agent_done,
            },
            SpeakKind::Progress => SpeechUnits::Fixed(std::mem::take(&mut progress_units)),
        };
        let mut speech = stream_reply(
            tts,
            units,
            gap_samples,
            turn,
            already_audible,
            audio,
            output_events,
            cmd_rx,
            running,
            picture,
            mic,
            watch.as_deref_mut(),
        );
        tts = std::mem::replace(&mut speech.tts, lost_tts());
        sp.report.audible |= speech.played;
        if let Some(started) = speech.audio_started_ms {
            let at = started_after_ms.saturating_add(started);
            match kind {
                SpeakKind::Progress => {
                    sp.report.progress_audio_ms.get_or_insert(at);
                }
                SpeakKind::Answer if speech.feed_cancelled => {
                    sp.report.cancelled_audio_ms.get_or_insert(at);
                }
                SpeakKind::Answer => {}
            }
        }
        match speech.wait {
            PlaybackWait::Finished => {
                if kind == SpeakKind::Answer {
                    if sp.feed.committed() {
                        tracing::info!(%turn, "streamed answer delivered during thinking");
                        sp.report.delivered = Some(speech);
                        sp.report.delivered_after_ms = started_after_ms;
                    }
                    sp.feed.disable();
                }
                break SpeakStep::Continue;
            }
            PlaybackWait::Aborted => {
                if kind == SpeakKind::Answer && !speech.feed_cancelled {
                    // Device loss or a synthesis error: the normal path
                    // speaks the full reply once the agent returns.
                    tracing::warn!(%turn, error = ?speech.error, "streamed answer aborted");
                    sp.feed.disable();
                }
                break SpeakStep::Continue;
            }
            PlaybackWait::Stopped => break SpeakStep::Resolve(ThinkResolve::Stopped),
            PlaybackWait::BargedIn => {
                tracing::info!(%turn, "barge-in during early speech");
                match listen_after_pause(mic, vad, stt, picture, cmd_rx, running, false) {
                    Ok(BargeDecision::Resume) if kind == SpeakKind::Answer => {
                        if audio.resume().is_err() {
                            audio.stop();
                            sp.feed.disable();
                            break SpeakStep::Continue;
                        }
                        already_audible = true;
                        picture.set_phase(Phase::Talking);
                        continue;
                    }
                    Ok(BargeDecision::Resume) => {
                        // The progress line is filler: drop its rest.
                        audio.stop();
                        break SpeakStep::Continue;
                    }
                    Ok(BargeDecision::StopTalking) => {
                        audio.stop();
                        break SpeakStep::Resolve(ThinkResolve::StopTurn);
                    }
                    Ok(BargeDecision::TakeTurn(next)) => {
                        audio.stop();
                        break SpeakStep::Resolve(ThinkResolve::TakeTurn(next));
                    }
                    Err(error) => {
                        audio.stop();
                        sp.feed.disable();
                        match error {
                            HearBreak::Stopped if !*running => {
                                break SpeakStep::Resolve(ThinkResolve::Stopped)
                            }
                            HearBreak::Stopped => break SpeakStep::Resolve(ThinkResolve::StopTurn),
                            HearBreak::Disconnected => {
                                break SpeakStep::Resolve(ThinkResolve::Stopped)
                            }
                            HearBreak::SwitchInput { device_id } => {
                                apply_input_switch(audio, picture, &device_id);
                            }
                            HearBreak::SwitchOutput { device_id } => {
                                apply_output_switch(audio, output_events, picture, &device_id);
                            }
                            HearBreak::StartWakeEnroll { .. }
                            | HearBreak::ClearWakeProfile
                            | HearBreak::RunSelfCheck => {}
                        }
                        break SpeakStep::Continue;
                    }
                }
            }
        }
    };
    sp.tts.give_back(tts);
    // Our own playback tail must not be scored as a wake once Thinking resumes.
    drain_mic(mic);
    if let Some(w) = watch.as_deref_mut() {
        w.reset();
    }
    if matches!(step, SpeakStep::Continue)
        && sp.report.delivered.is_none()
        && !agent_done.load(Ordering::Acquire)
    {
        picture.activity = prev_activity
            .filter(|activity| !activity.starts_with("barge-in"))
            .or_else(|| Some("thinking…".into()));
        picture.set_phase(Phase::Thinking);
    }
    step
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
    let prev_thinking = picture.thinking.clone();
    picture.activity = Some("barge-in · listening".into());
    picture.set_phase(Phase::Hearing);

    // Match talking barge-in semantics: the detection window is only for wake
    // and liveness. Drain its tail, then record a fresh clip so "Hey Boris"
    // never becomes part of the replacement request sent to STT.
    if let Err(error) = hear::settle_after_barge(mic, cmd_rx, running) {
        restore_thinking_picture(picture, prev_activity, prev_thinking);
        return Err(error);
    }

    // Once the wake is accepted, finish listening even if the old agent job
    // completes. Its result is held by run_thinking until this utterance says
    // resume, stop, or replace; cutting capture here loses the user's redirect.
    let clip = match hear::capture_utterance(mic, vad, cmd_rx, running, CaptureKind::BargeIn) {
        Ok(clip) => clip,
        Err(error) => {
            restore_thinking_picture(picture, prev_activity, prev_thinking);
            return Err(error);
        }
    };
    let follow = hear::crop_speech(vad, &clip);

    if let Err(e) = stt.load() {
        tracing::warn!(error = %e, %turn, "thinking barge-in stt load failed — resume");
        restore_thinking_picture(picture, prev_activity, prev_thinking);
        return Ok(BargeDecision::Resume);
    }

    picture.activity = Some("barge-in · transcribing".into());
    picture.set_phase(Phase::Reading);
    let text = transcribe_or_empty(stt, &follow.pcm, turn);
    let decision = decide_thinking_barge_listen(follow.speech_hops, &text);
    match &decision {
        BargeDecision::Resume => {
            restore_thinking_picture(picture, prev_activity, prev_thinking);
        }
        BargeDecision::StopTalking => {
            picture.activity = Some("barge-in · stopping".into());
            picture.set_phase(Phase::Thinking);
        }
        BargeDecision::TakeTurn(_) => {
            picture.activity = Some("barge-in · switching".into());
            picture.set_phase(Phase::Thinking);
        }
    }
    tracing::info!(%turn, %text, ?decision, "thinking barge-in heard");
    Ok(decision)
}

fn restore_thinking_picture(
    picture: &mut Picture,
    previous_activity: Option<String>,
    previous_thinking: Option<String>,
) {
    picture.activity = previous_activity
        .filter(|activity| !activity.starts_with("barge-in"))
        .or_else(|| Some("thinking…".into()));
    picture.thinking = previous_thinking;
    picture.set_phase(Phase::Thinking);
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
