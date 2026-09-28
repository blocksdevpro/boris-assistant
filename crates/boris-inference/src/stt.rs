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
/// # Load policy
///
/// Adapters may choose either:
/// - **Explicit load** — host calls [`SpeechToText::load`] before
///   `transcribe`; `transcribe` returns a clear error if unloaded.
/// - **Lazy load** — first `transcribe` (or `load`) opens weights; subsequent
///   calls reuse them until `unload`.
///
/// Prefer explicit preload on the product engine (overlap with capture) and
/// still tolerate lazy load so mocks and simple hosts stay short. Default
/// `load` / `unload` are no-ops.
///
/// # Empty audio
///
/// Empty or all-silent input should return `Ok(String::new())` (or equivalent
/// empty transcript), not panic. Adapters may also return empty when the
/// model produces no tokens.
///
/// # Error mapping
///
/// - Missing / invalid paths or settings → [`boris_core::Error::Config`].
/// - Runtime / inference failures → [`boris_core::Error::Other`] (or
///   [`boris_core::Error::Audio`] if the failure is clearly capture-related).
pub trait SpeechToText: Send {
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

    /// Stable backend identifier for logs / diagnostics (e.g. `"parakeet"`).
    fn backend_id(&self) -> &str {
        "unknown"
    }

    /// Transcribe mono PCM samples at [`boris_core::AUDIO_TARGET_RATE`].
    ///
    /// `audio` is interleaved mono `f32` in roughly `[-1.0, 1.0]`.
    ///
    /// Empty slices must return `Ok("")` (or empty string) rather than panic.
    fn transcribe(&mut self, audio: &[AudioSample]) -> Result<String>;

    /// Whether the backend can usefully score a growing (prefix) buffer while
    /// the user is still speaking.
    ///
    /// Offline models (e.g. Parakeet TDT) return `true` here when a full
    /// re-decode of the prefix is cheap enough to run as a *partial*: the
    /// result is advisory only and the host must still run a final
    /// [`SpeechToText::transcribe`] at endpoint. Backends with true chunked
    /// state should also return `true` and override
    /// [`SpeechToText::transcribe_partial`] with the incremental path.
    ///
    /// Default is `false` so existing adapters opt out until measured.
    fn supports_partials(&self) -> bool {
        false
    }

    /// Transcribe a prefix snapshot for live UI / early-endpoint hints.
    ///
    /// Default re-decodes via [`SpeechToText::transcribe`]. Hosts must treat
    /// the output as unstable (may change as more audio arrives) and must not
    /// skip the final `transcribe`.
    fn transcribe_partial(&mut self, audio: &[AudioSample]) -> Result<String> {
        self.transcribe(audio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boris_core::Error;

    struct EchoStt {
        loaded: bool,
    }

    impl SpeechToText for EchoStt {
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
            "echo"
        }

        fn transcribe(&mut self, audio: &[AudioSample]) -> Result<String> {
            if audio.is_empty() {
                return Ok(String::new());
            }
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
        let mut stt = EchoStt { loaded: false };
        assert!(!stt.is_loaded());
        assert_eq!(stt.backend_id(), "echo");
        stt.load().unwrap();
        assert!(stt.is_loaded());
        let text = stt.transcribe(&[0.0, 0.1, -0.1]).unwrap();
        assert_eq!(text, "samples=3");
        stt.unload().unwrap();
        assert!(!stt.is_loaded());
    }

    #[test]
    fn empty_audio_returns_empty_string() {
        let mut stt = EchoStt { loaded: true };
        assert_eq!(stt.transcribe(&[]).unwrap(), "");
    }

    #[test]
    fn default_is_loaded_is_false() {
        struct Bare;
        impl SpeechToText for Bare {
            fn transcribe(&mut self, _: &[AudioSample]) -> Result<String> {
                Ok(String::new())
            }
        }
        let bare = Bare;
        assert!(!bare.is_loaded());
        assert_eq!(bare.backend_id(), "unknown");
    }

    #[test]
    fn dyn_object_safe() {
        let mut boxed: Box<dyn SpeechToText> = Box::new(FailingStt);
        let err = boxed.transcribe(&[]).unwrap_err();
        assert_eq!(err.to_string(), "stt down");
        assert!(!boxed.is_loaded());
        assert_eq!(boxed.backend_id(), "unknown");
    }

    #[test]
    fn partials_opt_out_by_default() {
        struct Bare;
        impl SpeechToText for Bare {
            fn transcribe(&mut self, _: &[AudioSample]) -> Result<String> {
                Ok("final".into())
            }
        }
        let mut bare = Bare;
        assert!(!bare.supports_partials());
        // Default partial path re-decodes via transcribe.
        assert_eq!(bare.transcribe_partial(&[0.0]).unwrap(), "final");
    }
}
