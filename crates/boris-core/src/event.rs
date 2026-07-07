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

    /// TTS synthesis is complete; the PCM samples are ready for playback.
    PlaybackReady(AudioBuffer),

    // ── Errors ────────────────────────────────────────────────────────────────
    /// A worker encountered a non-fatal error. The main loop logs it and
    /// decides whether to attempt recovery.
    WorkerError {
        worker: &'static str,
        message: String,
    },
}
