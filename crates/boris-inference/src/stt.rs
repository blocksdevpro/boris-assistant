//! Speech-to-text capability port.

use boris_core::{AudioSample, Result};

/// Converts mono PCM (pipeline rate) into text.
///
/// # Lifecycle
///
/// 1. Construct with paths / config (adapter-specific constructors).
/// 2. [`SpeechToText::load`] — open weights / runtime (may be heavy).
/// 3. [`SpeechToText::transcribe`] — one or more utterances.
/// 4. [`SpeechToText::unload`] — free GPU/CPU weights when idle.
///
/// Default `load` / `unload` are no-ops so lightweight mocks stay short.
/// Production adapters should override both and treat a missing model in
/// `transcribe` as an error (or lazy-load once).
pub trait SpeechToText: Send {
    /// Load model weights / runtime. Idempotent preferred.
    fn load(&mut self) -> Result<()> {
        Ok(())
    }

    /// Release model resources. Safe to call when already unloaded.
    fn unload(&mut self) -> Result<()> {
        Ok(())
    }

    /// Transcribe mono PCM samples at [`boris_core::AUDIO_TARGET_RATE`].
    ///
    /// `audio` is interleaved mono `f32` in roughly `[-1.0, 1.0]`.
    fn transcribe(&mut self, audio: &[AudioSample]) -> Result<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use boris_core::Error;

    struct EchoStt;

    impl SpeechToText for EchoStt {
        fn transcribe(&mut self, audio: &[AudioSample]) -> Result<String> {
            Ok(format!("samples={}", audio.len()))
        }
    }

    struct FailingStt;

    impl SpeechToText for FailingStt {
        fn transcribe(&mut self, _: &[AudioSample]) -> Result<String> {
            Err(Error::other("stt down"))
        }
    }

    #[test]
    fn default_load_unload_and_transcribe() {
        let mut stt = EchoStt;
        stt.load().unwrap();
        let text = stt.transcribe(&[0.0, 0.1, -0.1]).unwrap();
        assert_eq!(text, "samples=3");
        stt.unload().unwrap();
    }

    #[test]
    fn dyn_object_safe() {
        let mut boxed: Box<dyn SpeechToText> = Box::new(FailingStt);
        let err = boxed.transcribe(&[]).unwrap_err();
        assert_eq!(err.to_string(), "stt down");
    }
}
