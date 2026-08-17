//! Mic-side helpers used **inline** by the engine turn loop.
//!
//! These are plain functions over a mic channel — not background workers and
//! not policy state machines.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use boris_audio::buffer::{RecordingBuffer, SlidingBuffer};
use boris_audio::AUDIO_TARGET_RATE;
use boris_core::{ArcAudioBuffer, AudioBuffer};
use boris_sense::{
    duration_to_samples, vad_initial_timeout_samples, vad_silence_samples, Vad, WakeWord,
    VAD_WINDOW_SIZE, WAKEWORD_PROCESSING_INTERVAL, WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE,
};

use crate::engine::EngineCommand;

/// How long the user has to *start* speaking when Boris is awaiting a freeform reply.
const AWAIT_REPLY_START_TIMEOUT: Duration = Duration::from_secs(12);

/// Yes/no confirm: short start window — answers are "yes"/"no", not essays.
const AWAIT_CONFIRM_START_TIMEOUT: Duration = Duration::from_secs(6);

/// Discard residual playback / room echo before opening VAD after TTS.
/// Too short → Boris's own voice (or room echo) gets transcribed as the user.
const POST_TTS_SETTLE: Duration = Duration::from_millis(550);

/// Settle after a confirm prompt. Must drain speaker tail / room echo so VAD
/// does not hear Boris as the user — but keep it tight: every ms here is dead
/// air before the user knows they can answer.
const POST_CONFIRM_SETTLE: Duration = Duration::from_millis(380);

/// After a barge-in pause the speaker is already silent. Only wait out the
/// room tail so VAD does not grab the last syllable of leftover playback.
const POST_BARGE_SETTLE: Duration = Duration::from_millis(220);

/// Trailing silence for yes/no confirms (short utterances endpoint fast).
///
/// 250 ms is one official Silero hop-window of patience after speech stops —
/// enough to absorb a breath, not enough to make HITL feel frozen. Freeform
/// keeps the longer shared [`vad_silence_samples`] (LiveKit 550 ms) window.
const CONFIRM_SILENCE_AFTER: Duration = Duration::from_millis(250);

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
    /// Next wake hits are enroll takes, not turns.
    StartWakeEnroll { takes: u32 },
    /// Drop the stored live-mic profile.
    ClearWakeProfile,
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
            Ok(EngineCommand::StartWakeEnroll { takes }) => {
                return Err(HearBreak::StartWakeEnroll { takes });
            }
            Ok(EngineCommand::ClearWakeProfile) => {
                return Err(HearBreak::ClearWakeProfile);
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
///
/// Returns the 2 s window that fired so the engine can run liveness / enroll
/// on the same samples.
pub fn wait_for_wake(
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    wake: &mut impl WakeWord,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
) -> Result<AudioBuffer, HearBreak> {
    tracing::info!(
        threshold = WAKEWORD_THRESHOLD,
        window = WAKEWORD_WINDOW_SIZE,
        "wait_for_wake: listening (heartbeat every ~5s with max score)"
    );
    let wait_started = std::time::Instant::now();
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

        let pcm = window.read();
        match wake.predict(&pcm) {
            Ok(score) if score >= WAKEWORD_THRESHOLD => {
                tracing::info!(
                    score,
                    frames,
                    scores,
                    ms = wait_started.elapsed().as_millis() as u64,
                    "wake hit"
                );
                return Ok(pcm);
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

/// Brief settle after pausing speech for a barge-in listen.
pub fn settle_after_barge(
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
) -> Result<(), HearBreak> {
    settle_after_playback_for(mic, cmd_rx, running, POST_BARGE_SETTLE)
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
        CaptureKind::AwaitConfirm => 8, // short yes/no — do not hold the mic forever
        _ => 30,
    };
    tracing::info!(?kind, max_secs, "capture_utterance begin");
    let wall = std::time::Instant::now();
    let mut record = RecordingBuffer::new(
        AUDIO_TARGET_RATE as usize * 2,
        AUDIO_TARGET_RATE as usize * max_secs as usize,
    );
    record.set_recording(true);
    vad.reset();

    let mut has_spoken = false;
    let mut samples_since_speech: usize = 0;
    // Incremental hop assembler. Input callback frames can be any size; consume
    // every sample without a fixed-capacity staging ring, allocation, drain, or
    // memmove. Silero still receives exact 512-sample hops in stream order.
    let mut hop = [0.0f32; VAD_WINDOW_SIZE];
    let mut hop_len = 0usize;
    let mut n_predicts: u32 = 0;
    let mut n_speech_hops: u32 = 0;
    // Confirm: *shorter* trailing silence than freeform — "yes"/"no" end cleanly
    // and 1.3s+ of post-speech wait made HITL feel frozen after the answer.
    let silence_after = match kind {
        CaptureKind::AwaitConfirm => {
            duration_to_samples(CONFIRM_SILENCE_AFTER, AUDIO_TARGET_RATE).max(VAD_WINDOW_SIZE)
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
            // take_audio first — never stop/trim before draining the full clip.
            let clip = record.take_audio();
            record.set_recording(false);
            tracing::warn!(
                samples = clip.len(),
                has_spoken,
                ms = wall.elapsed().as_millis() as u64,
                "utterance hit max length — cutting clip"
            );
            return Ok(clip);
        }

        for &sample in frame.iter() {
            hop[hop_len] = sample;
            hop_len += 1;
            if hop_len < VAD_WINDOW_SIZE {
                continue;
            }
            hop_len = 0;
            n_predicts = n_predicts.saturating_add(1);

            match vad.predict(&hop) {
                Ok(true) => {
                    if !has_spoken {
                        tracing::debug!("vad: speech started");
                    }
                    has_spoken = true;
                    n_speech_hops = n_speech_hops.saturating_add(1);
                    samples_since_speech = 0;
                }
                Ok(false) => {
                    samples_since_speech = samples_since_speech.saturating_add(hop.len());
                    let limit = if has_spoken {
                        silence_after
                    } else {
                        silence_before
                    };
                    if samples_since_speech >= limit {
                        // Drain the full utterance before leaving recording mode.
                        // (Stopping first used to trim to pre-roll and drop speech.)
                        let clip = record.take_audio();
                        record.set_recording(false);
                        tracing::info!(
                            samples = clip.len(),
                            clip_ms = (clip.len() as u64 * 1000) / AUDIO_TARGET_RATE as u64,
                            ms = wall.elapsed().as_millis() as u64,
                            has_spoken,
                            n_predicts,
                            n_speech_hops,
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

/// Speech region inside a wake window. Empty when Silero heard no speech —
/// do **not** fall back to the raw 2 s buffer (that enrolled room noise).
#[derive(Debug, Clone)]
pub struct SpeechCrop {
    pub pcm: AudioBuffer,
    pub speech_hops: u32,
}

/// Keep Silero-speech hops plus one hop of context. Used for liveness / enroll.
pub fn crop_speech(vad: &mut (impl Vad + ?Sized), pcm: &[f32]) -> SpeechCrop {
    vad.reset();
    let hop = VAD_WINDOW_SIZE;
    let mut first = None;
    let mut last = None;
    let mut speech_hops = 0u32;
    let mut i = 0;
    while i + hop <= pcm.len() {
        match vad.predict(&pcm[i..i + hop]) {
            Ok(true) => {
                speech_hops = speech_hops.saturating_add(1);
                if first.is_none() {
                    first = Some(i);
                }
                last = Some(i);
            }
            Ok(false) => {}
            Err(e) => tracing::debug!(error = %e, "vad crop"),
        }
        i += hop;
    }
    match (first, last) {
        (Some(a), Some(b)) => {
            let start = a.saturating_sub(hop);
            let end = (b + hop * 2).min(pcm.len());
            SpeechCrop {
                pcm: pcm[start..end].to_vec(),
                speech_hops,
            }
        }
        _ => SpeechCrop {
            pcm: Vec::new(),
            speech_hops: 0,
        },
    }
}

/// Drop incoming mic for `ms` so the same wake does not re-fire immediately.
pub fn drain_ms(
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    ms: u64,
) -> Result<(), HearBreak> {
    let deadline = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        still_running(cmd_rx, running)?;
        match mic.recv_timeout(Duration::from_millis(20)) {
            Ok(_) | Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(HearBreak::Disconnected);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::Arc;

    use boris_core::ArcAudioBuffer;
    use boris_sense::Vad;

    use super::*;

    struct ScriptedVad {
        answers: Vec<bool>,
        idx: usize,
        reset_count: u32,
    }

    impl Vad for ScriptedVad {
        fn reset(&mut self) {
            self.reset_count = self.reset_count.saturating_add(1);
        }

        fn predict(&mut self, audio: &[f32]) -> boris_core::Result<bool> {
            assert_eq!(audio.len(), VAD_WINDOW_SIZE);
            let i = self.idx.min(self.answers.len().saturating_sub(1));
            self.idx = self.idx.saturating_add(1);
            Ok(self.answers.get(i).copied().unwrap_or(false))
        }
    }

    fn run_capture(
        kind: CaptureKind,
        answers: Vec<bool>,
        hops: usize,
    ) -> (boris_core::AudioBuffer, u32) {
        let (mic_tx, mic_rx) = crossbeam_channel::unbounded::<ArcAudioBuffer>();
        let (_cmd_tx, cmd_rx) = mpsc::channel();
        for _ in 0..hops {
            mic_tx
                .send(Arc::from(vec![0.0f32; VAD_WINDOW_SIZE]))
                .expect("send hop");
        }
        let mut vad = ScriptedVad {
            answers,
            idx: 0,
            reset_count: 0,
        };
        let mut running = true;
        let clip = capture_utterance(&mic_rx, &mut vad, &cmd_rx, &mut running, kind)
            .expect("capture should endpoint");
        (clip, vad.reset_count)
    }

    #[test]
    fn all_false_after_wake_hits_start_timeout() {
        // Start timeout is hop-aligned by construction (duration / 32 ms).
        let start_hops = vad_initial_timeout_samples() / VAD_WINDOW_SIZE;
        let (clip, resets) = run_capture(CaptureKind::AfterWake, vec![false], start_hops);
        assert_eq!(resets, 1);
        assert_eq!(clip.len(), start_hops * VAD_WINDOW_SIZE);
    }

    #[test]
    fn speech_then_silence_waits_trailing_window() {
        // 1 speech hop + trailing silence hops.
        let trailing = vad_silence_samples().div_ceil(VAD_WINDOW_SIZE);
        let mut answers = vec![false; trailing + 1];
        answers[0] = true;
        let (clip, resets) = run_capture(CaptureKind::AfterWake, answers, trailing + 1);
        assert_eq!(resets, 1);
        assert_eq!(clip.len(), (trailing + 1) * VAD_WINDOW_SIZE);
    }

    #[test]
    fn confirm_endpoints_sooner_than_freeform() {
        let confirm_trailing =
            duration_to_samples(CONFIRM_SILENCE_AFTER, AUDIO_TARGET_RATE).div_ceil(VAD_WINDOW_SIZE);
        let freeform_trailing = vad_silence_samples().div_ceil(VAD_WINDOW_SIZE);
        assert!(confirm_trailing < freeform_trailing);

        let hops = freeform_trailing + 8;
        let mut answers = vec![false; hops];
        answers[0] = true;

        let (confirm, _) = run_capture(CaptureKind::AwaitConfirm, answers.clone(), hops);
        let (reply, _) = run_capture(CaptureKind::AwaitReply, answers, hops);
        assert_eq!(confirm.len(), (confirm_trailing + 1) * VAD_WINDOW_SIZE);
        assert_eq!(reply.len(), (freeform_trailing + 1) * VAD_WINDOW_SIZE);
        assert!(confirm.len() < reply.len());
    }

    #[test]
    fn oversized_callback_frame_processes_every_hop_without_drops() {
        let trailing = vad_silence_samples().div_ceil(VAD_WINDOW_SIZE);
        let hops = trailing + 1;
        let (mic_tx, mic_rx) = crossbeam_channel::unbounded::<ArcAudioBuffer>();
        let (_cmd_tx, cmd_rx) = mpsc::channel();
        // One callback frame is deliberately much larger than the former
        // four-hop pseudo-ring. Speech + all endpoint silence live in it.
        mic_tx
            .send(Arc::from(vec![0.0f32; hops * VAD_WINDOW_SIZE]))
            .expect("send oversized frame");
        let mut answers = vec![false; hops];
        answers[0] = true;
        let mut vad = ScriptedVad {
            answers,
            idx: 0,
            reset_count: 0,
        };
        let mut running = true;
        let clip = capture_utterance(
            &mic_rx,
            &mut vad,
            &cmd_rx,
            &mut running,
            CaptureKind::AfterWake,
        )
        .expect("oversized frame should contain a complete utterance");
        assert_eq!(vad.idx, hops, "every complete hop must reach Silero");
        assert_eq!(clip.len(), hops * VAD_WINDOW_SIZE);
    }
}
