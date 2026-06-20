use crate::AudioBuffer;

pub enum Event {
    WakeWordDetected,
    SpeechEnded,
    RecordingResult(AudioBuffer),
    SpeechToTextResult(String),
}

pub enum InferenceEvent {}
