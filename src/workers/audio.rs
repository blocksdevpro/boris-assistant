use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use boris_audio::{buffer::RecordingBuffer, AUDIO_TARGET_RATE};
use boris_core::{event::Event, types::ArcAudioBuffer, TurnId};

// ── Recorder control ──────────────────────────────────────────────────────────

/// Start capture for a specific turn so the resulting clip is tagged correctly.
#[derive(Debug)]
pub enum RecorderCtl {
    Start { turn: TurnId },
    Stop,
}

// ── Utterance capture ─────────────────────────────────────────────────────────

/// Job that records one user utterance for a [`TurnId`].
///
/// Continuously keeps a short pre-roll while idle. [`RecorderCtl::Start`] freezes
/// that window and appends live audio; [`RecorderCtl::Stop`] emits
/// [`Event::RecordingResult`] tagged with the active turn.
pub struct UtteranceCapture {
    _handle: JoinHandle<()>,
}

impl UtteranceCapture {
    pub fn spawn(
        audio_rx: crossbeam_channel::Receiver<ArcAudioBuffer>,
        control_rx: mpsc::Receiver<RecorderCtl>,
        event_tx: mpsc::Sender<Event>,
    ) -> Self {
        let handle = thread::spawn(move || {
            // 2-second pre-roll so speech that started just before Start is kept.
            // Hard-cap active recording at 30s so a stuck VAD cannot grow forever.
            const MAX_UTTERANCE_SECS: u32 = 30;
            let mut buffer = RecordingBuffer::new(
                AUDIO_TARGET_RATE as usize * 2,
                AUDIO_TARGET_RATE as usize * MAX_UTTERANCE_SECS as usize,
            );
            let mut active_turn: Option<TurnId> = None;

            loop {
                while let Ok(cmd) = control_rx.try_recv() {
                    match cmd {
                        RecorderCtl::Start { turn } => {
                            active_turn = Some(turn);
                            buffer.set_recording(true);
                        }
                        RecorderCtl::Stop => {
                            buffer.set_recording(false);
                            let audio = buffer.take_audio();
                            if let Some(turn) = active_turn.take() {
                                event_tx.send(Event::RecordingResult { turn, audio }).ok();
                            } else {
                                tracing::warn!(
                                    "UtteranceCapture: Stop with no active turn — dropping clip"
                                );
                            }
                        }
                    }
                }

                match audio_rx.recv() {
                    Ok(audio) => {
                        buffer.push(&audio);
                        // Force-end the utterance when the hard cap is hit so Session
                        // can progress (STT) instead of growing RAM indefinitely.
                        if buffer.exceeded_max() {
                            if let Some(turn) = active_turn.take() {
                                buffer.set_recording(false);
                                let clip = buffer.take_audio();
                                tracing::warn!(
                                    %turn,
                                    samples = clip.len(),
                                    "UtteranceCapture: max utterance length reached — forcing clip"
                                );
                                event_tx
                                    .send(Event::RecordingResult { turn, audio: clip })
                                    .ok();
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Self { _handle: handle }
    }
}
