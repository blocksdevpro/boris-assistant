//! Text-to-speech capability port.

use boris_core::{AudioBuffer, Result};

/// Converts text into mono PCM for playback.
///
/// # Lifecycle
///
/// Same pattern as STT: construct → [`TextToSpeech::load`] →
/// [`TextToSpeech::synthesize`] → [`TextToSpeech::unload`].
///
/// # Load policy
///
/// Adapters may choose either:
/// - **Explicit load** — host calls [`TextToSpeech::load`] before
///   `synthesize`; `synthesize` returns a clear error if unloaded.
/// - **Lazy load** — first `synthesize` (or `load`) opens weights; subsequent
///   calls reuse them until `unload`.
///
/// Prefer explicit preload on the product engine (overlap with agent thinking)
/// and still tolerate lazy load so simple hosts stay short. Default
/// `load` / `unload` are no-ops.
///
/// # Output format
///
/// Returned PCM is **mono `f32`** in roughly `[-1.0, 1.0]`. The engine / audio
/// stack is responsible for resampling from the adapter's native rate
/// ([`TextToSpeech::sample_rate`]) to the output device when needed.
///
/// # Empty text
///
/// Empty or whitespace-only input should return an empty buffer (`Ok(vec![])`)
/// so the engine can skip playback. Prefer that over an error for benign empty
/// input.
///
/// # Error mapping
///
/// - Missing / invalid paths, voices, or settings → [`boris_core::Error::Config`].
/// - Runtime / inference failures → [`boris_core::Error::Other`].
pub trait TextToSpeech: Send {
    /// Load model weights / runtime. Idempotent preferred.
    fn load(&mut self) -> Result<()> {
        Ok(())
    }

    /// Release model resources. Safe to call when already unloaded.
    fn unload(&mut self) -> Result<()> {
        Ok(())
    }

    /// Whether weights / runtime are currently loaded.
    ///
    /// Default is `false` so mocks that never load stay honest. Production
    /// adapters should override to reflect real load state.
    fn is_loaded(&self) -> bool {
        false
    }

    /// Stable backend identifier for logs / diagnostics (e.g. `"supertone"`).
    fn backend_id(&self) -> &str {
        "unknown"
    }

    /// Native sample rate of synthesized mono PCM (Hz).
    ///
    /// Used by hosts to configure playback resampling. Adapters should return
    /// a stable documented rate even before `load` when the rate is fixed by
    /// the model (e.g. 24_000 for Kokoro, 44_100 for Supertone).
    ///
    /// Default is `0` meaning "unknown / not applicable" for mocks.
    fn sample_rate(&self) -> u32 {
        0
    }

    /// Number of native-rate silent samples the host should place between
    /// separately synthesized speakable units.
    ///
    /// Most adapters do not split replies and therefore return zero. Adapters
    /// that own sentence pacing (such as Supertone) override this so a host can
    /// stream units without losing the same pause that a one-shot synthesis
    /// would have inserted.
    fn inter_unit_silence_samples(&self) -> usize {
        0
    }

    /// Synthesize `text` to a mono PCM buffer.
    ///
    /// Empty or whitespace-only input should return an empty buffer.
    fn synthesize(&mut self, text: &str) -> Result<AudioBuffer>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use boris_core::Error;

    struct SilenceTts {
        loaded: bool,
    }

    impl TextToSpeech for SilenceTts {
        fn load(&mut self) -> Result<()> {
            self.loaded = true;
            Ok(())
        }

        fn unload(&mut self) -> Result<()> {
            self.loaded = false;
            Ok(())
        }

        fn is_loaded(&self) -> bool {
            self.loaded
        }

        fn backend_id(&self) -> &str {
            "silence"
        }

        fn sample_rate(&self) -> u32 {
            16_000
        }

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
        let mut tts = SilenceTts { loaded: false };
        assert!(tts.synthesize("   ").unwrap().is_empty());
        assert_eq!(tts.synthesize("hi").unwrap().len(), 16);
    }

    #[test]
    fn lifecycle_load_unload_is_loaded() {
        let mut tts = SilenceTts { loaded: false };
        assert!(!tts.is_loaded());
        assert_eq!(tts.backend_id(), "silence");
        assert_eq!(tts.sample_rate(), 16_000);
        tts.load().unwrap();
        assert!(tts.is_loaded());
        tts.unload().unwrap();
        assert!(!tts.is_loaded());
    }

    #[test]
    fn default_is_loaded_and_sample_rate() {
        struct Bare;
        impl TextToSpeech for Bare {
            fn synthesize(&mut self, _: &str) -> Result<AudioBuffer> {
                Ok(Vec::new())
            }
        }
        let bare = Bare;
        assert!(!bare.is_loaded());
        assert_eq!(bare.backend_id(), "unknown");
        assert_eq!(bare.sample_rate(), 0);
        assert_eq!(bare.inter_unit_silence_samples(), 0);
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
        assert!(!boxed.is_loaded());
        assert_eq!(boxed.backend_id(), "unknown");
        assert_eq!(boxed.sample_rate(), 0);
    }
}
