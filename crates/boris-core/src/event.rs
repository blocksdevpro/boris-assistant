use crate::AudioBuffer;

pub enum BorisEvent {
    WakeWordDetected,
    SpeechEnded,
    RecordingFinished(AudioBuffer),
}

pub enum InferenceEvent {}
