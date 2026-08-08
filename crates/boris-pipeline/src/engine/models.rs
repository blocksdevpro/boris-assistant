//! STT / TTS load helpers for the sequential turn loop.
//!
//! Models load one step ahead on short helper threads (STT while capturing,
//! TTS while the agent thinks) so the UI never shows "loading model" chrome —
//! only real phases (Hearing / Reading / Thinking / Talking).

use std::thread::{self, JoinHandle};

use boris_core::TurnId;
use boris_inference::{SpeechToText, TextToSpeech};

pub(super) type SttBox = Box<dyn SpeechToText>;
pub(super) type TtsBox = Box<dyn TextToSpeech>;

/// Drop STT + TTS weights from RAM. Safe if already unloaded.
pub(super) fn release_voice_models(
    stt: &mut dyn SpeechToText,
    tts: &mut dyn TextToSpeech,
    reason: &str,
) {
    if let Err(e) = stt.unload() {
        tracing::warn!(error = %e, %reason, "stt unload failed");
    }
    if let Err(e) = tts.unload() {
        tracing::warn!(error = %e, %reason, "tts unload failed");
    }
    tracing::info!(%reason, "STT/TTS released (idle RAM)");
}

/// Load STT on a helper thread (overlaps with mic capture / playback).
pub(super) fn spawn_stt_load(mut stt: SttBox) -> JoinHandle<(SttBox, Result<(), String>)> {
    thread::Builder::new()
        .name("boris-stt-load".into())
        .spawn(move || {
            let t = std::time::Instant::now();
            let r = stt.load().map_err(|e| e.to_string());
            if r.is_ok() {
                tracing::info!(ms = t.elapsed().as_millis() as u64, "stt preload ready");
            }
            (stt, r)
        })
        .expect("spawn stt load thread")
}

pub(super) fn join_stt_load(
    job: JoinHandle<(SttBox, Result<(), String>)>,
) -> (SttBox, Result<(), String>) {
    job.join().unwrap_or_else(|_| {
        tracing::error!("stt load thread panicked");
        // Recover with a no-op stub so the engine can still stop cleanly.
        // Real STT is lost only if the load thread panicked mid-flight.
        (
            Box::new(PanicLostStt),
            Err("stt load thread panicked".into()),
        )
    })
}

/// Reclaim STT after an optional follow-up preload job (or the idle slot).
pub(super) fn reclaim_stt_slot(
    slot: &mut Option<SttBox>,
    job: &mut Option<JoinHandle<(SttBox, Result<(), String>)>>,
) -> SttBox {
    if let Some(j) = job.take() {
        let (stt, load_r) = join_stt_load(j);
        if let Err(e) = load_r {
            tracing::warn!(error = %e, "stt follow-up preload failed (will retry on next turn)");
        }
        return stt;
    }
    slot.take().expect("stt slot empty")
}

/// Load TTS on a helper thread (overlaps with agent thinking).
pub(super) fn spawn_tts_load(mut tts: TtsBox) -> JoinHandle<(TtsBox, Result<(), String>)> {
    thread::Builder::new()
        .name("boris-tts-load".into())
        .spawn(move || {
            let t = std::time::Instant::now();
            let r = tts.load().map_err(|e| e.to_string());
            if r.is_ok() {
                tracing::info!(ms = t.elapsed().as_millis() as u64, "tts preload ready");
            }
            (tts, r)
        })
        .expect("spawn tts load thread")
}

pub(super) fn join_tts_load(
    job: JoinHandle<(TtsBox, Result<(), String>)>,
) -> (TtsBox, Result<(), String>) {
    job.join().unwrap_or_else(|_| {
        tracing::error!("tts load thread panicked");
        (
            Box::new(PanicLostTts),
            Err("tts load thread panicked".into()),
        )
    })
}

pub(super) fn unload_stt(stt: &mut dyn SpeechToText, turn: TurnId) {
    if let Err(e) = stt.unload() {
        tracing::warn!(error = %e, %turn, "stt unload failed");
    } else {
        tracing::debug!(%turn, "stt unloaded");
    }
}

pub(super) fn unload_tts(tts: &mut dyn TextToSpeech, turn: TurnId) {
    if let Err(e) = tts.unload() {
        tracing::warn!(error = %e, %turn, "tts unload failed");
    } else {
        tracing::debug!(%turn, "tts unloaded");
    }
}

/// Build the product STT backend (or a null stub when the feature is off).
pub(super) fn create_stt(model_dir: std::path::PathBuf) -> SttBox {
    #[cfg(feature = "stt-parakeet")]
    {
        Box::new(boris_stt_parakeet::ParakeetStt::with_model_dir(model_dir))
    }
    #[cfg(not(feature = "stt-parakeet"))]
    {
        let _ = model_dir;
        Box::new(NullStt)
    }
}

/// Build the product TTS backend (or a null stub when the feature is off).
pub(super) fn create_tts(
    model_dir: std::path::PathBuf,
    voice_dir: std::path::PathBuf,
    voice_id: &str,
) -> TtsBox {
    #[cfg(feature = "tts-supertone")]
    {
        Box::new(boris_tts_supertone::SupertoneTts::with_paths(
            model_dir, voice_dir, voice_id,
        ))
    }
    #[cfg(not(feature = "tts-supertone"))]
    {
        let _ = (model_dir, voice_dir, voice_id);
        Box::new(NullTts)
    }
}

/// Placeholder if the STT load thread panics (should never happen in practice).
struct PanicLostStt;
impl SpeechToText for PanicLostStt {
    fn transcribe(&mut self, _: &[boris_core::AudioSample]) -> boris_core::Result<String> {
        Err(boris_core::Error::other(
            "STT model lost after load-thread panic",
        ))
    }
}

struct PanicLostTts;
impl TextToSpeech for PanicLostTts {
    fn synthesize(&mut self, _: &str) -> boris_core::Result<boris_core::AudioBuffer> {
        Err(boris_core::Error::other(
            "TTS model lost after load-thread panic",
        ))
    }
}

// ── Optional null backends when features are off ─────────────────────────────

#[cfg(not(feature = "stt-parakeet"))]
struct NullStt;

#[cfg(not(feature = "stt-parakeet"))]
impl SpeechToText for NullStt {
    fn transcribe(&mut self, _: &[boris_core::AudioSample]) -> boris_core::Result<String> {
        Err(boris_core::Error::other("stt-parakeet feature disabled"))
    }
}

#[cfg(not(feature = "tts-supertone"))]
struct NullTts;

#[cfg(not(feature = "tts-supertone"))]
impl TextToSpeech for NullTts {
    fn synthesize(&mut self, _: &str) -> boris_core::Result<boris_core::AudioBuffer> {
        Err(boris_core::Error::other("tts-supertone feature disabled"))
    }
}
