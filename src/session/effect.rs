use boris_core::{AudioBuffer, TurnId};

/// Side effects the runtime applies after [`super::Session::handle`].
///
/// Session is pure policy: it never touches channels. `apply_effects` in `main`
/// is the only place that maps these onto worker commands.
#[derive(Debug)]
pub enum Effect {
    /// Enable wakeword scoring.
    ArmWakeword,
    /// Disable wakeword scoring (listening / speaking).
    DisarmWakeword,
    /// Start VAD + utterance capture for `turn`.
    StartListen { turn: TurnId },
    /// Stop VAD + utterance capture (clip should follow).
    StopListen,
    /// Preload STT model.
    WarmStt,
    /// Send PCM to STT for `turn`.
    Transcribe { turn: TurnId, audio: AudioBuffer },
    /// Preload TTS model.
    WarmTts,
    /// Send user text to the agent for `turn`.
    Chat { turn: TurnId, text: String },
    /// Send agent reply text to TTS for `turn`.
    Synthesize { turn: TurnId, text: String },
    /// Queue PCM on the playback sink for `turn`.
    Play { turn: TurnId, pcm: AudioBuffer },
    /// Immediately silence speakers and drop any in-flight play job.
    /// Used on recovery so wake re-arm does not hear TTS echo.
    StopPlayback,
}
