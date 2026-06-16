use crate::AudioBuffer;

pub enum Event {
    WakeWordDetected,
    SpeechEnded,
    RecordingFinished(AudioBuffer),
}

pub enum InferenceEvent {}
