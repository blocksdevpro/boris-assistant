use crate::AudioBuffer;

pub enum Event {
    // ── Audio pipeline ────────────────────────────────────────────────────────
    WakeWordDetected,
    SpeechEnded,
    RecordingResult(AudioBuffer),

    // ── Inference ─────────────────────────────────────────────────────────────
    SpeechToTextResult(String),

    // ── Agent ─────────────────────────────────────────────────────────────────
    /// The agent has produced a text reply to be spoken aloud (→ TTS).
    AgentResponse(String),
}

pub enum InferenceEvent {}
