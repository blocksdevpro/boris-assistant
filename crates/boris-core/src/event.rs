use crate::AudioSampleBuffer;

pub enum BorisEvent {
    WakeWordDetected,
    SpeechEnded,
    RecordingFinished(AudioSampleBuffer),
}

pub enum InferenceEvent {}
