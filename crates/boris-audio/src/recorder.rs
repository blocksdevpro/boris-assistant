use std::{
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
};

use boris_core::{AudioSampleBuffer, event::BorisEvent};

pub struct AudioRecorder {
    _handle: JoinHandle<()>,
}
pub enum RecordCommand {
    StartRecording,
    StopRecording,
}

impl AudioRecorder {
    pub fn spawn(
        audio_rx: Receiver<AudioSampleBuffer>,
        control_rx: Receiver<RecordCommand>,
        event_tx: Sender<BorisEvent>,
    ) -> Self {
        let handle = std::thread::spawn(move || {
            let mut buffer: Vec<f32> = Vec::new();
            let mut is_recording = false; // Added state variable

            loop {
                while let Ok(command) = control_rx.try_recv() {
                    match command {
                        RecordCommand::StartRecording => {
                            buffer.clear();
                            is_recording = true;
                        }
                        RecordCommand::StopRecording => {
                            is_recording = false;
                            println!("buffer_len: {}", buffer.len());
                            if !buffer.is_empty() {
                                let audio = std::mem::take(&mut buffer);
                                event_tx.send(BorisEvent::RecordingFinished(audio)).ok();
                            }
                        }
                    }
                }

                if let Ok(audio) = audio_rx.recv() {
                    if is_recording {
                        buffer.extend(audio.iter());
                    }
                }
            }
        });

        Self { _handle: handle }
    }
}
