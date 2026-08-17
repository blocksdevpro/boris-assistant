//! Responsive sentence-streamed TTS + playback coordination.
//!
//! Synthesis runs on one helper thread because a model call can take seconds.
//! The sequential engine remains the sole owner of phase transitions, audio
//! commands, device switches, and host-command polling.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;
use boris_core::{ArcAudioBuffer, AudioBuffer, TurnId};

use crate::status::{EngineState, Phase};

use super::barge::{drain_mic, BargeWatch};
use super::models::{lost_tts, TtsBox};
use super::picture::Picture;
use super::playback::{poll_running, PlaybackWait};
use super::EngineCommand;

const EVENT_POLL: Duration = Duration::from_millis(20);
const START_TIMEOUT: Duration = Duration::from_secs(5);
const FINISH_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
/// A cancelled inference is cooperative and may still be inside one native
/// model call. Do not let Stop/device-switch wait forever for that call.
const SYNTH_CANCEL_JOIN_GRACE: Duration = Duration::from_secs(2);
const MIN_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_DRAIN_TIMEOUT: Duration = Duration::from_secs(300);

enum SynthEvent {
    Unit { index: usize, pcm: AudioBuffer },
    Failed { index: usize, message: String },
    Done,
}

enum SpawnedSynth {
    Running {
        join: JoinHandle<Option<TtsBox>>,
        cancel: Arc<AtomicBool>,
        events: crossbeam_channel::Receiver<SynthEvent>,
    },
    Failed {
        tts: TtsBox,
        message: String,
    },
}

/// Result of a streamed speech job. The TTS model is always returned, even
/// after host cancellation or a worker panic (the latter returns a lost-model
/// placeholder so the engine can surface a normal load error next turn).
pub(super) struct StreamedSpeech {
    pub tts: TtsBox,
    pub wait: PlaybackWait,
    pub error: Option<String>,
    pub played: bool,
    pub tts_first_ms: u64,
    /// Observed output-worker `Started`, relative to this speech job.
    pub audio_started_ms: Option<u64>,
    /// Observed output callback `Drained`, relative to this speech job.
    pub audio_drained_ms: Option<u64>,
    pub tts_ms: u64,
    pub play_ms: u64,
    pub speech_ms: u64,
    pub queued_samples: usize,
    /// Units not yet appended when playback was paused for barge-in.
    pub remaining_units: Vec<String>,
}

fn send_synth_event(
    tx: &crossbeam_channel::Sender<SynthEvent>,
    cancel: &AtomicBool,
    mut event: SynthEvent,
) -> bool {
    loop {
        if cancel.load(Ordering::Acquire) {
            return false;
        }
        match tx.send_timeout(event, EVENT_POLL) {
            Ok(()) => return true,
            Err(crossbeam_channel::SendTimeoutError::Timeout(returned)) => event = returned,
            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => return false,
        }
    }
}

fn spawn_synth(tts: TtsBox, units: Vec<String>) -> SpawnedSynth {
    // Hand the model over only after spawn succeeds. If the OS rejects the
    // thread, the engine retains the original model instead of dropping it.
    let (model_tx, model_rx) = mpsc::sync_channel::<TtsBox>(1);
    let (event_tx, event_rx) = crossbeam_channel::bounded::<SynthEvent>(2);
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();

    let join = match thread::Builder::new()
        .name("boris-tts-stream".into())
        .spawn(move || {
            let mut tts = model_rx.recv().ok()?;
            for (index, unit) in units.into_iter().enumerate() {
                if worker_cancel.load(Ordering::Acquire) {
                    break;
                }
                match tts.synthesize(&unit) {
                    Ok(pcm) => {
                        if !send_synth_event(
                            &event_tx,
                            &worker_cancel,
                            SynthEvent::Unit { index, pcm },
                        ) {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = send_synth_event(
                            &event_tx,
                            &worker_cancel,
                            SynthEvent::Failed {
                                index,
                                message: error.to_string(),
                            },
                        );
                        break;
                    }
                }
            }
            let _ = send_synth_event(&event_tx, &worker_cancel, SynthEvent::Done);
            Some(tts)
        }) {
        Ok(join) => join,
        Err(error) => {
            return SpawnedSynth::Failed {
                tts,
                message: format!("spawn TTS stream worker: {error}"),
            };
        }
    };

    match model_tx.send(tts) {
        Ok(()) => SpawnedSynth::Running {
            join,
            cancel,
            events: event_rx,
        },
        Err(mpsc::SendError(tts)) => {
            let _ = join.join();
            SpawnedSynth::Failed {
                tts,
                message: "TTS stream worker exited before receiving model".into(),
            }
        }
    }
}

fn drain_deadline(now: Instant, queued_samples: usize, sample_rate: u32) -> Instant {
    let audio_seconds =
        (queued_samples as f64 / sample_rate.max(1) as f64).min(MAX_DRAIN_TIMEOUT.as_secs_f64());
    let audio = Duration::from_secs_f64(audio_seconds);
    let allowance = (audio + Duration::from_secs(8)).clamp(MIN_DRAIN_TIMEOUT, MAX_DRAIN_TIMEOUT);
    now + allowance
}

fn pace_unit(pcm: AudioBuffer, gap_samples: usize, has_prior_unit: bool) -> AudioBuffer {
    if !has_prior_unit || gap_samples == 0 || pcm.is_empty() {
        return pcm;
    }
    let mut paced = Vec::with_capacity(gap_samples.saturating_add(pcm.len()));
    paced.resize(gap_samples, 0.0);
    paced.extend(pcm);
    paced
}

fn wait_for_synth_exit<T>(
    join: &JoinHandle<T>,
    deadline: Option<Instant>,
    mut on_wait: impl FnMut(),
) -> bool {
    while !join.is_finished() {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return false;
        }
        on_wait();
        thread::sleep(EVENT_POLL);
    }
    true
}

/// Synthesize and play reply units while continuing to service host commands.
#[allow(clippy::too_many_arguments)]
pub(super) fn stream_reply(
    tts: TtsBox,
    units: Vec<String>,
    gap_samples: usize,
    turn: TurnId,
    already_audible: bool,
    audio: &mut AudioService,
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    cmd_rx: &mpsc::Receiver<EngineCommand>,
    running: &mut bool,
    picture: &mut Picture,
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    mut barge: Option<&mut BargeWatch<'_>>,
) -> StreamedSpeech {
    let started_at = Instant::now();
    let source_rate = audio.source_rate();
    let all_units = units;
    if already_audible {
        picture.set_phase(Phase::Talking);
    }

    let (join, cancel, synth_events, mut synthesis_done, mut held_tts) = if all_units.is_empty()
    {
        if !already_audible {
            return StreamedSpeech {
                tts,
                wait: PlaybackWait::Aborted,
                error: Some("TTS produced no playable audio".into()),
                played: already_audible,
                tts_first_ms: 0,
                audio_started_ms: None,
                audio_drained_ms: None,
                tts_ms: started_at.elapsed().as_millis() as u64,
                play_ms: 0,
                speech_ms: started_at.elapsed().as_millis() as u64,
                queued_samples: 0,
                remaining_units: Vec::new(),
            };
        }
        // Resume after barge-in with only leftover PCM — no more sentences.
        (None, None, None, true, Some(tts))
    } else {
        match spawn_synth(tts, all_units.clone()) {
            SpawnedSynth::Running {
                join,
                cancel,
                events,
            } => (Some(join), Some(cancel), Some(events), false, None),
            SpawnedSynth::Failed { tts, message } => {
                return StreamedSpeech {
                    tts,
                    wait: PlaybackWait::Aborted,
                    error: Some(message),
                    played: already_audible,
                    tts_first_ms: 0,
                    audio_started_ms: None,
                    audio_drained_ms: None,
                    tts_ms: started_at.elapsed().as_millis() as u64,
                    play_ms: 0,
                    speech_ms: started_at.elapsed().as_millis() as u64,
                    queued_samples: 0,
                    remaining_units: all_units,
                };
            }
        }
    };

    if !already_audible {
        while output_events.try_recv().is_ok() {}
    }

    let mut synthesis_error = None;
    let mut tts_first_ms = 0u64;
    let mut tts_done_ms = 0u64;
    let mut audio_queued_at = None;
    let mut audio_started_at = None;
    let mut audio_started_ms = None;
    let mut audio_drained_ms = None;
    let mut started_deadline = None;
    let mut drain_by = None;
    let mut finish_retry_started = None;
    let mut finish_ack: Option<crossbeam_channel::Receiver<()>> = None;
    let mut pause_ack: Option<crossbeam_channel::Receiver<()>> = None;
    let mut finish_applied = false;
    let mut drain_observed = false;
    let mut queued_units = 0usize;
    let mut queued_samples = 0usize;
    let mut heard_started = already_audible;
    let mut wait = PlaybackWait::Finished;
    let mut done = false;

    while !done {
        let poll = poll_running(cmd_rx, running, audio, output_events, picture);
        if !poll.running {
            if let Some(cancel) = cancel.as_ref() {
                cancel.store(true, Ordering::Release);
            }
            audio.stop();
            // Make Stop visible immediately even if the current model call
            // needs a moment to return and release its weights.
            picture.engine = EngineState::Off;
            picture.set_phase(Phase::Off);
            wait = PlaybackWait::Stopped;
            break;
        }
        if poll.output_rebuilt {
            if let Some(cancel) = cancel.as_ref() {
                cancel.store(true, Ordering::Release);
            }
            picture.set_phase(Phase::Armed);
            wait = PlaybackWait::Aborted;
            break;
        }

        if pause_ack.is_none() {
            let hit = match barge.as_mut() {
                Some(watch) => watch.poll(),
                None => {
                    drain_mic(mic);
                    None
                }
            };
            if hit.is_some() {
                match audio.request_pause() {
                    Ok(ack) => {
                        tracing::info!(%turn, queued_units, "barge-in — pausing leftover speech");
                        pause_ack = Some(ack);
                        if let Some(cancel) = cancel.as_ref() {
                            cancel.store(true, Ordering::Release);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%turn, error = %error, "barge-in pause enqueue failed");
                    }
                }
            }
        }
        if let Some(ack) = pause_ack.as_ref() {
            match ack.try_recv() {
                Ok(()) => {
                    wait = PlaybackWait::BargedIn;
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    synthesis_error = Some("pause acknowledgement disconnected".into());
                    audio.stop();
                    wait = PlaybackWait::Aborted;
                    break;
                }
            }
        }

        if let Some(synth_events) = synth_events.as_ref() {
            loop {
                match synth_events.try_recv() {
                    Ok(SynthEvent::Unit { index, pcm }) => {
                        if pcm.is_empty() {
                            tracing::warn!(%turn, unit = index, "TTS unit produced no samples");
                            continue;
                        }
                        let pcm =
                            pace_unit(pcm, gap_samples, already_audible || queued_units > 0);
                        let samples = pcm.len();
                        if let Err(error) = audio.append(pcm) {
                            synthesis_error =
                                Some(format!("append TTS unit {}: {error}", index + 1));
                            if let Some(cancel) = cancel.as_ref() {
                                cancel.store(true, Ordering::Release);
                            }
                            audio.stop();
                            wait = PlaybackWait::Aborted;
                            done = true;
                            break;
                        }
                        if queued_units == 0 && !already_audible {
                            tts_first_ms = started_at.elapsed().as_millis() as u64;
                            let now = Instant::now();
                            audio_queued_at = Some(now);
                            started_deadline = Some(now + START_TIMEOUT);
                        }
                        queued_units += 1;
                        queued_samples = queued_samples.saturating_add(samples);
                    }
                    Ok(SynthEvent::Failed { index, message }) => {
                        tracing::warn!(%turn, unit = index, error = %message, "TTS stream unit failed");
                        synthesis_error = Some(format!("TTS unit {}: {message}", index + 1));
                    }
                    Ok(SynthEvent::Done) => {
                        synthesis_done = true;
                        tts_done_ms = started_at.elapsed().as_millis() as u64;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        synthesis_done = true;
                        if tts_done_ms == 0 {
                            tts_done_ms = started_at.elapsed().as_millis() as u64;
                        }
                        break;
                    }
                }
            }
        }

        loop {
            match output_events.try_recv() {
                Ok(OutputEvent::Started) => {
                    if !heard_started {
                        heard_started = true;
                        audio_started_at = Some(Instant::now());
                        audio_started_ms = Some(started_at.elapsed().as_millis() as u64);
                        picture.set_phase(Phase::Talking);
                        tracing::info!(%turn, "playback started — UI Talking");
                    }
                }
                Ok(OutputEvent::Drained) => {
                    // Event and control acknowledgements use separate channels;
                    // either may be observed first even though the worker closes
                    // the job before the callback can drain it.
                    if !heard_started && queued_units > 0 {
                        // A saturated advisory event queue can drop Started.
                        // Drained proves that real samples reached the callback.
                        heard_started = true;
                        audio_started_at = audio_queued_at;
                        audio_started_ms = Some(tts_first_ms);
                    }
                    drain_observed = true;
                    audio_drained_ms = Some(started_at.elapsed().as_millis() as u64);
                    if finish_applied {
                        wait = PlaybackWait::Finished;
                        done = true;
                        break;
                    }
                }
                Ok(OutputEvent::Cleared) => {
                    wait = PlaybackWait::Aborted;
                    done = true;
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    synthesis_error
                        .get_or_insert_with(|| "output event worker disconnected".into());
                    wait = PlaybackWait::Aborted;
                    done = true;
                    break;
                }
            }
        }

        if done {
            break;
        }

        if synthesis_done && queued_units == 0 && !already_audible {
            synthesis_error.get_or_insert_with(|| "TTS produced no playable audio".into());
            wait = PlaybackWait::Aborted;
            break;
        }

        if synthesis_done && finish_ack.is_none() && !finish_applied && pause_ack.is_none() {
            let retry_start = *finish_retry_started.get_or_insert_with(Instant::now);
            match audio.request_finish_job() {
                Ok(ack) => finish_ack = Some(ack),
                Err(error) if retry_start.elapsed() < FINISH_REQUEST_TIMEOUT => {
                    tracing::debug!(%turn, error = %error, "FinishJob queue busy — retrying");
                }
                Err(error) => {
                    synthesis_error = Some(format!("could not close playback job: {error}"));
                    audio.stop();
                    wait = PlaybackWait::Aborted;
                    break;
                }
            }
        }

        if let Some(ack) = finish_ack.as_ref() {
            match ack.try_recv() {
                Ok(()) => {
                    finish_ack = None;
                    finish_applied = true;
                    drain_by = Some(drain_deadline(Instant::now(), queued_samples, source_rate));
                    if drain_observed {
                        wait = PlaybackWait::Finished;
                        done = true;
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    if finish_retry_started
                        .is_some_and(|start| start.elapsed() >= FINISH_REQUEST_TIMEOUT)
                    {
                        synthesis_error = Some("FinishJob acknowledgement timed out".into());
                        audio.stop();
                        wait = PlaybackWait::Aborted;
                        break;
                    }
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    synthesis_error = Some("FinishJob acknowledgement disconnected".into());
                    audio.stop();
                    wait = PlaybackWait::Aborted;
                    break;
                }
            }
        }

        if done {
            break;
        }

        let now = Instant::now();
        if !heard_started && started_deadline.is_some_and(|deadline| now >= deadline) {
            // `Started` is advisory and intentionally nonblocking in the audio
            // worker. If its bounded event queue was saturated, infer start
            // from the accepted PCM and keep the stronger Drained deadline as
            // the stuck-output guard.
            tracing::warn!(%turn, "playback Started event missed; inferring queued start");
            heard_started = true;
            audio_started_at = audio_queued_at;
            audio_started_ms = Some(tts_first_ms);
            started_deadline = None;
            picture.set_phase(Phase::Talking);
        }
        if drain_by.is_some_and(|deadline| now >= deadline) {
            synthesis_error = Some("playback did not drain before deadline".into());
            audio.stop();
            wait = PlaybackWait::Aborted;
            break;
        }

        thread::sleep(EVENT_POLL);
    }

    if wait != PlaybackWait::Finished {
        if let Some(cancel) = cancel.as_ref() {
            cancel.store(true, Ordering::Release);
        }
    }

    // A model inference cannot be preempted safely. While it winds down, keep
    // handling host/device commands instead of blocking in join(). Cancelled
    // jobs get a finite grace; after that the native call is detached and the
    // engine receives a lost-model placeholder rather than hanging forever.
    let tts = if let Some(join) = join {
        let join_deadline =
            (wait != PlaybackWait::Finished).then(|| Instant::now() + SYNTH_CANCEL_JOIN_GRACE);
        let synth_exited = wait_for_synth_exit(&join, join_deadline, || {
            let poll = poll_running(cmd_rx, running, audio, output_events, picture);
            if !poll.running {
                audio.stop();
                picture.engine = EngineState::Off;
                picture.set_phase(Phase::Off);
                wait = PlaybackWait::Stopped;
            } else if poll.output_rebuilt {
                picture.set_phase(Phase::Armed);
                wait = PlaybackWait::Aborted;
            }
        });
        if synth_exited {
            match join.join() {
                Ok(Some(tts)) => tts,
                Ok(None) => {
                    synthesis_error.get_or_insert_with(|| "TTS stream lost model ownership".into());
                    lost_tts()
                }
                Err(_) => {
                    synthesis_error.get_or_insert_with(|| "TTS stream worker panicked".into());
                    lost_tts()
                }
            }
        } else {
            synthesis_error.get_or_insert_with(|| {
                format!(
                    "TTS cancellation exceeded {} ms; detached stuck inference",
                    SYNTH_CANCEL_JOIN_GRACE.as_millis()
                )
            });
            tracing::error!(%turn, grace_ms = SYNTH_CANCEL_JOIN_GRACE.as_millis() as u64, "detaching stuck TTS inference");
            drop(join);
            lost_tts()
        }
    } else {
        held_tts.take().unwrap_or_else(lost_tts)
    };

    let remaining_units = if wait == PlaybackWait::BargedIn {
        all_units.into_iter().skip(queued_units).collect()
    } else {
        Vec::new()
    };

    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    StreamedSpeech {
        tts,
        wait,
        error: synthesis_error,
        played: heard_started,
        tts_first_ms,
        audio_started_ms,
        audio_drained_ms,
        tts_ms: if tts_done_ms == 0 {
            elapsed_ms
        } else {
            tts_done_ms
        },
        play_ms: audio_started_at
            .map(|start| start.elapsed().as_millis() as u64)
            .unwrap_or(0),
        speech_ms: elapsed_ms,
        queued_samples,
        remaining_units,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boris_inference::TextToSpeech;

    struct FakeTts;

    impl TextToSpeech for FakeTts {
        fn synthesize(&mut self, text: &str) -> boris_core::Result<AudioBuffer> {
            Ok(vec![text.len() as f32; 8])
        }
    }

    #[test]
    fn drain_budget_scales_with_audio_and_is_capped() {
        let now = Instant::now();
        let short = drain_deadline(now, 44_100, 44_100).duration_since(now);
        assert_eq!(short, Duration::from_secs(9));

        let huge = drain_deadline(now, usize::MAX, 1).duration_since(now);
        assert_eq!(huge, MAX_DRAIN_TIMEOUT);
    }

    #[test]
    fn separately_synthesized_units_keep_adapter_owned_gap() {
        assert_eq!(pace_unit(vec![0.4, 0.5], 3, false), vec![0.4, 0.5]);
        assert_eq!(
            pace_unit(vec![0.4, 0.5], 3, true),
            vec![0.0, 0.0, 0.0, 0.4, 0.5]
        );
    }

    #[test]
    fn cancelling_producer_returns_model_ownership() {
        let spawned = spawn_synth(
            Box::new(FakeTts),
            vec!["one".into(), "two".into(), "three".into(), "four".into()],
        );
        let SpawnedSynth::Running {
            join,
            cancel,
            events,
        } = spawned
        else {
            panic!("test worker must spawn");
        };
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(1)),
            Ok(SynthEvent::Unit { index: 0, .. })
        ));
        cancel.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !join.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(join.is_finished(), "cancelled producer must wind down");
        assert!(join.join().unwrap().is_some(), "model must be returned");
    }

    #[test]
    fn cancelled_worker_wait_has_a_hard_deadline() {
        let (release_tx, release_rx) = mpsc::sync_channel::<()>(0);
        let join = thread::spawn(move || {
            let _ = release_rx.recv();
        });
        let started = Instant::now();
        let finished = wait_for_synth_exit(
            &join,
            Some(Instant::now() + Duration::from_millis(40)),
            || {},
        );
        assert!(!finished);
        assert!(started.elapsed() < Duration::from_millis(250));

        // The production path detaches here. Release and join in the unit test
        // so no helper thread leaks into neighboring tests.
        release_tx.send(()).unwrap();
        join.join().unwrap();
    }
}
