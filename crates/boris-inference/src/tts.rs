//! Text-to-speech capability port.

use boris_core::{AudioBuffer, Result};

/// Converts text into mono PCM for playback.
///
/// # Lifecycle
///
/// Same pattern as STT: construct → [`TextToSpeech::load`] →
/// [`TextToSpeech::synthesize`] → [`TextToSpeech::unload`].
///
/// Returned PCM is mono `f32`. The engine / audio stack is responsible for
/// resampling from the adapter's native rate to the output device when needed.
pub trait TextToSpeech: Send {
    /// Load model weights / runtime. Idempotent preferred.
    fn load(&mut self) -> Result<()> {
        Ok(())
    }

    /// Release model resources. Safe to call when already unloaded.
    fn unload(&mut self) -> Result<()> {
        Ok(())
    }

    /// Synthesize `text` to a mono PCM buffer.
    ///
    /// Empty or whitespace-only input may return an empty buffer or an error;
    /// adapters should document their choice. Prefer an empty buffer for
    /// benign empty input so the engine can skip playback.
    fn synthesize(&mut self, text: &str) -> Result<AudioBuffer>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use boris_core::Error;

    struct SilenceTts;

    impl TextToSpeech for SilenceTts {
        fn synthesize(&mut self, text: &str) -> Result<AudioBuffer> {
            if text.trim().is_empty() {
                return Ok(Vec::new());
            }
            // Tiny non-empty buffer so callers can detect "had speech".
            Ok(vec![0.0; 16])
        }
    }

    #[test]
    fn synthesize_empty_and_nonempty() {
        let mut tts = SilenceTts;
        assert!(tts.synthesize("   ").unwrap().is_empty());
        assert_eq!(tts.synthesize("hi").unwrap().len(), 16);
    }

    #[test]
    fn dyn_object_safe() {
        struct Boom;
        impl TextToSpeech for Boom {
            fn synthesize(&mut self, _: &str) -> Result<AudioBuffer> {
                Err(Error::other("tts down"))
            }
        }
        let mut boxed: Box<dyn TextToSpeech> = Box::new(Boom);
        assert!(boxed.synthesize("x").is_err());
    }
}
