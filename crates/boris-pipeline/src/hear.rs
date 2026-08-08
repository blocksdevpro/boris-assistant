//! Mic-side helpers used **inline** by the engine turn loop.
//!
//! These are plain functions over a mic channel — not background workers and
//! not policy state machines.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use boris_audio::buffer::{RecordingBuffer, SlidingBuffer};
use boris_audio::AUDIO_TARGET_RATE;
use boris_core::types::ArcAudioBuffer;
use boris_core::AudioBuffer;
use boris_sense::{
    duration_to_samples, vad_initial_timeout_samples, vad_silence_samples, Vad, WakeWord,
    VAD_PROCESSING_INTERVAL, VAD_WINDOW_SIZE, WAKEWORD_PROCESSING_INTERVAL, WAKEWORD_THRESHOLD,
    WAKEWORD_WINDOW_SIZE,
};

use crate::engine::EngineCommand;

/// How long the user has to *start* speaking when Boris is awaiting a freeform reply.
const AWAIT_REPLY_START_TIMEOUT: Duration = Duration::from_secs(12);

/// Yes/no confirm: slightly shorter start window (answers are short).
const AWAIT_CONFIRM_START_TIMEOUT: Duration = Duration::from_secs(10);

/// Discard residual playback / room echo before opening VAD after TTS.
/// Too short → Boris's own voice (or room echo) gets transcribed as the user.
const POST_TTS_SETTLE: Duration = Duration::from_millis(550);

/// Longer settle after a confirm prompt so TTS tail / room echo does not trip VAD.
const POST_CONFIRM_SETTLE: Duration = Duration::from_millis(1000);

/// Why a hear step returned early.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HearBreak {
    /// Host asked to stop / shut down.
    Stopped,
    /// Host command channel closed.
    Disconnected,
    /// Host wants a different microphone (engine applies then re-enters hear).
    SwitchInput { device_id: String },
    /// Host wants a different speaker.
    SwitchOutput { device_id: String },
}

/// Capture mode for utterance recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    /// After wake word — short initial silence budget.
    AfterWake,
    /// Freeform follow-up (name, choice, full sentence) — longer time to start.
    AwaitReply,
    /// Yes/no after tool confirmation — short answers, careful VAD settle.
    AwaitConfirm,
}

/// Poll host commands while blocking on audio.
fn still_running(cmd_rx: &Receiver<EngineCommand>, running: &mut bool) -> Result<(), HearBreak> {
    loop {
        match cmd_rx.try_recv() {
            Ok(EngineCommand::Stop) | Ok(EngineCommand::Shutdown) => {
                *running = false;
                return Err(HearBreak::Stopped);
            }
            Ok(EngineCommand::Start) => {
                *running = true;
            }
            Ok(EngineCommand::SwitchInput { device_id }) => {
                return Err(HearBreak::SwitchInput { device_id });
            }
            Ok(EngineCommand::SwitchOutput { device_id }) => {
                return Err(HearBreak::SwitchOutput { device_id });
            }
            Err(mpsc::TryRecvError::Empty) => return Ok(()),
            Err(mpsc::TryRecvError::Disconnected) => return Err(HearBreak::Disconnected),
        }
    }
}

fn next_frame(
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
) -> Result<ArcAudioBuffer, HearBreak> {
    loop {
        still_running(cmd_rx, running)?;
        match mic.recv_timeout(std::time::Duration::from_millis(40)) {
            Ok(frame) => return Ok(frame),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(HearBreak::Disconnected);
            }
        }
    }
}

/// Block until the wake model crosses threshold, or the host stops / switches devices.
pub fn wait_for_wake(
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    wake: &mut impl WakeWord,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
) -> Result<(), HearBreak> {
    tracing::info!(
        threshold = WAKEWORD_THRESHOLD,
        window = WAKEWORD_WINDOW_SIZE,
        "wait_for_wake: listening (heartbeat every ~5s with max score)"
    );
    let mut window = SlidingBuffer::new(WAKEWORD_WINDOW_SIZE);
    let score_every = duration_to_samples(WAKEWORD_PROCESSING_INTERVAL, AUDIO_TARGET_RATE);
    let mut samples_since_score: usize = 0;
    let mut frames: u64 = 0;
    let mut scores: u64 = 0;
    let mut max_score: f32 = 0.0;
    let mut last_heartbeat = std::time::Instant::now();
    let mut mic_samples: u64 = 0;

    loop {
        let frame = next_frame(mic, cmd_rx, running)?;
        frames = frames.saturating_add(1);
        mic_samples = mic_samples.saturating_add(frame.len() as u64);
        window.push(&frame);
        samples_since_score = samples_since_score.saturating_add(frame.len());

        if samples_since_score < score_every || !window.ready() {
            // Still useful: prove mic is delivering audio even before first score.
            if last_heartbeat.elapsed() >= std::time::Duration::from_secs(5) {
                tracing::info!(
                    frames,
                    mic_samples,
                    scores,
                    max_score,
                    window_ready = window.ready(),
                    "wake wait heartbeat (no hit yet)"
                );
                last_heartbeat = std::time::Instant::now();
                max_score = 0.0;
            }
            continue;
        }
        samples_since_score = 0;

        match wake.predict(&window.read()) {
            Ok(score) if score >= WAKEWORD_THRESHOLD => {
                tracing::info!(score, frames, scores, "wake hit");
                return Ok(());
            }
            Ok(score) => {
                scores = scores.saturating_add(1);
                if score > max_score {
                    max_score = score;
                }
            }
            Err(e) => tracing::error!(error = %e, "wake predict failed"),
        }

        if last_heartbeat.elapsed() >= std::time::Duration::from_secs(5) {
            tracing::info!(
                frames,
                mic_samples,
                scores,
                max_score,
                threshold = WAKEWORD_THRESHOLD,
                "wake wait heartbeat (no hit yet)"
            );
            last_heartbeat = std::time::Instant::now();
            max_score = 0.0;
        }
    }
}

/// Drain mic briefly after TTS so Boris's own voice is less likely to trigger VAD.
pub fn settle_after_playback(
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
) -> Result<(), HearBreak> {
    settle_after_playback_for(mic, cmd_rx, running, POST_TTS_SETTLE)
}

/// Longer settle used after HITL confirm prompts (echo-sensitive).
pub fn settle_after_confirm(
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
) -> Result<(), HearBreak> {
    settle_after_playback_for(mic, cmd_rx, running, POST_CONFIRM_SETTLE)
}

fn settle_after_playback_for(
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    settle: Duration,
) -> Result<(), HearBreak> {
    let deadline = std::time::Instant::now() + settle;
    while std::time::Instant::now() < deadline {
        still_running(cmd_rx, running)?;
        match mic.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(_) => {} // drop
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(HearBreak::Disconnected);
            }
        }
    }
    Ok(())
}

/// Record from pre-roll through VAD endpoint (or max length). Returns PCM @ 16 kHz.
///
/// Freeform answers (names, choices, full sentences) use [`CaptureKind::AwaitReply`].
pub fn capture_utterance(
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    vad: &mut impl Vad,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    kind: CaptureKind,
) -> Result<AudioBuffer, HearBreak> {
    let max_secs: u32 = match kind {
        CaptureKind::AwaitConfirm => 12, // short yes/no — do not hold the mic forever
        _ => 30,
    };
    tracing::info!(?kind, max_secs, "capture_utterance begin");
    let mut record = RecordingBuffer::new(
        AUDIO_TARGET_RATE as usize * 2,
        AUDIO_TARGET_RATE as usize * max_secs as usize,
    );
    record.set_recording(true);

    let mut has_spoken = false;
    let mut samples_since_speech: usize = 0;
    let mut frame_buf: Vec<f32> = Vec::new();
    let score_every = duration_to_samples(VAD_PROCESSING_INTERVAL, AUDIO_TARGET_RATE);
    let mut samples_since_score: usize = 0;
    // Confirm: require a bit more trailing silence so "yeah…" isn't cut mid-word.
    let silence_after = match kind {
        CaptureKind::AwaitConfirm => {
            vad_silence_samples().saturating_mul(3).saturating_div(2).max(vad_silence_samples())
        }
        _ => vad_silence_samples(),
    };
    let silence_before = match kind {
        CaptureKind::AfterWake => vad_initial_timeout_samples(),
        CaptureKind::AwaitReply => {
            duration_to_samples(AWAIT_REPLY_START_TIMEOUT, AUDIO_TARGET_RATE)
        }
        CaptureKind::AwaitConfirm => {
            duration_to_samples(AWAIT_CONFIRM_START_TIMEOUT, AUDIO_TARGET_RATE)
        }
    };

    loop {
        let frame = next_frame(mic, cmd_rx, running)?;
        record.push(&frame);
        if record.exceeded_max() {
            let clip = {
                record.set_recording(false);
                record.take_audio()
            };
            tracing::warn!(
                samples = clip.len(),
                has_spoken,
                "utterance hit max length — cutting clip"
            );
            return Ok(clip);
        }

        frame_buf.extend_from_slice(&frame);
        while frame_buf.len() >= VAD_WINDOW_SIZE {
            let chunk: Vec<f32> = frame_buf.drain(..VAD_WINDOW_SIZE).collect();
            samples_since_score = samples_since_score.saturating_add(chunk.len());
            if samples_since_score < score_every {
                continue;
            }
            samples_since_score = 0;

            match vad.predict(&chunk) {
                Ok(true) => {
                    if !has_spoken {
                        tracing::debug!("vad: speech started");
                    }
                    has_spoken = true;
                    samples_since_speech = 0;
                }
                Ok(false) => {
                    samples_since_speech = samples_since_speech.saturating_add(score_every);
                    let limit = if has_spoken {
                        silence_after
                    } else {
                        silence_before
                    };
                    if samples_since_speech >= limit {
                        let clip = {
                            record.set_recording(false);
                            record.take_audio()
                        };
                        tracing::info!(
                            samples = clip.len(),
                            ms = (clip.len() as u64 * 1000) / AUDIO_TARGET_RATE as u64,
                            has_spoken,
                            ?kind,
                            "capture_utterance end (silence endpoint)"
                        );
                        return Ok(clip);
                    }
                }
                Err(e) => tracing::error!(error = %e, "vad predict failed"),
            }
        }
    }
}
