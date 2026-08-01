//! Mic-side helpers used **inline** by the engine turn loop.
//!
//! These are plain functions over a mic channel — not background workers and
//! not policy state machines.

use std::sync::mpsc::{self, Receiver};

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

/// Why a hear step returned early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HearBreak {
    /// Host asked to stop / shut down.
    Stopped,
    /// Host command channel closed.
    Disconnected,
}

/// Poll host commands while blocking on audio. Returns `true` if still running.
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
            Ok(EngineCommand::SwitchInput { .. }) | Ok(EngineCommand::SwitchOutput { .. }) => {
                // Device switch is applied by the engine between turns; ignore mid-hear.
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

/// Block until the wake model crosses threshold, or the host stops us.
pub fn wait_for_wake(
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    wake: &mut impl WakeWord,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
) -> Result<(), HearBreak> {
    let mut window = SlidingBuffer::new(WAKEWORD_WINDOW_SIZE);
    let score_every = duration_to_samples(WAKEWORD_PROCESSING_INTERVAL, AUDIO_TARGET_RATE);
    let mut samples_since_score: usize = 0;

    loop {
        let frame = next_frame(mic, cmd_rx, running)?;
        window.push(&frame);
        samples_since_score = samples_since_score.saturating_add(frame.len());

        if samples_since_score < score_every || !window.ready() {
            continue;
        }
        samples_since_score = 0;

        match wake.predict(&window.read()) {
            Ok(score) if score >= WAKEWORD_THRESHOLD => {
                tracing::info!(score, "wake hit");
                return Ok(());
            }
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "wake predict failed"),
        }
    }
}

/// Record from pre-roll through VAD endpoint (or max length). Returns PCM @ 16 kHz.
pub fn capture_utterance(
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    vad: &mut impl Vad,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
) -> Result<AudioBuffer, HearBreak> {
    const MAX_UTTERANCE_SECS: u32 = 30;
    let mut record = RecordingBuffer::new(
        AUDIO_TARGET_RATE as usize * 2,
        AUDIO_TARGET_RATE as usize * MAX_UTTERANCE_SECS as usize,
    );
    record.set_recording(true);

    let mut has_spoken = false;
    let mut samples_since_speech: usize = 0;
    let mut frame_buf: Vec<f32> = Vec::new();
    let score_every = duration_to_samples(VAD_PROCESSING_INTERVAL, AUDIO_TARGET_RATE);
    let mut samples_since_score: usize = 0;
    let silence_after = vad_silence_samples();
    let silence_before = vad_initial_timeout_samples();

    loop {
        let frame = next_frame(mic, cmd_rx, running)?;
        record.push(&frame);
        if record.exceeded_max() {
            tracing::warn!("utterance hit max length — cutting clip");
            record.set_recording(false);
            return Ok(record.take_audio());
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
                        record.set_recording(false);
                        return Ok(record.take_audio());
                    }
                }
                Err(e) => tracing::error!(error = %e, "vad predict failed"),
            }
        }
    }
}
