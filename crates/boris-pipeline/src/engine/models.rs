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

/// Background model load that never panics the engine thread on spawn failure.
///
/// Call sites treat the joined `Result` as a fault-friendly load error (same as
/// a failed `load()`), so spawn OS errors surface cleanly instead of aborting.
pub(super) enum ModelLoadJob<T> {
    Thread(JoinHandle<(T, Result<(), String>)>),
    /// Spawn failed (or sync recovery); `T` is still owned for the engine.
    Ready(T, Result<(), String>),
}

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
pub(super) fn spawn_stt_load(stt: SttBox) -> ModelLoadJob<SttBox> {
    // Deliver the model after a successful spawn so a failed spawn keeps ownership
    // (moving into the closure would drop STT when the OS rejects the thread).
    let (tx, rx) = std::sync::mpsc::sync_channel::<SttBox>(1);
    match thread::Builder::new()
        .name("boris-stt-load".into())
        .spawn(move || {
            let mut stt = match rx.recv() {
                Ok(s) => s,
                Err(_) => {
                    return (
                        Box::new(LostStt) as SttBox,
                        Err("stt load channel closed".into()),
                    );
                }
            };
            let t = std::time::Instant::now();
            let r = stt.load().map_err(|e| e.to_string());
            if r.is_ok() {
                tracing::info!(ms = t.elapsed().as_millis() as u64, "stt preload ready");
            }
            (stt, r)
        }) {
        Ok(h) => match tx.send(stt) {
            Ok(()) => ModelLoadJob::Thread(h),
            Err(std::sync::mpsc::SendError(stt)) => {
                let _ = h.join();
                ModelLoadJob::Ready(
                    stt,
                    Err("stt load thread exited before receiving model".into()),
                )
            }
        },
        Err(e) => {
            tracing::error!(error = %e, "failed to spawn stt load thread");
            ModelLoadJob::Ready(stt, Err(format!("spawn stt load thread: {e}")))
        }
    }
}

pub(super) fn join_stt_load(job: ModelLoadJob<SttBox>) -> (SttBox, Result<(), String>) {
    match job {
        ModelLoadJob::Thread(h) => h.join().unwrap_or_else(|_| {
            tracing::error!("stt load thread panicked");
            // Recover with a no-op stub so the engine can still stop cleanly.
            (Box::new(LostStt), Err("stt load thread panicked".into()))
        }),
        ModelLoadJob::Ready(stt, r) => (stt, r),
    }
}

/// Reclaim STT after an optional follow-up preload job (or the idle slot).
pub(super) fn reclaim_stt_slot(
    slot: &mut Option<SttBox>,
    job: &mut Option<ModelLoadJob<SttBox>>,
) -> SttBox {
    if let Some(j) = job.take() {
        let (stt, load_r) = join_stt_load(j);
        if let Err(e) = load_r {
            tracing::warn!(error = %e, "stt follow-up preload failed (will retry on next turn)");
        }
        return stt;
    }
    if let Some(stt) = slot.take() {
        return stt;
    }
    // Invariant broken (should never happen). Recover so the engine loop continues.
    tracing::error!("stt slot empty (invariant broken); recovering with placeholder STT");
    Box::new(LostStt)
}

/// Load TTS on a helper thread (overlaps with agent thinking).
pub(super) fn spawn_tts_load(tts: TtsBox) -> ModelLoadJob<TtsBox> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<TtsBox>(1);
    match thread::Builder::new()
        .name("boris-tts-load".into())
        .spawn(move || {
            let mut tts = match rx.recv() {
                Ok(s) => s,
                Err(_) => {
                    return (
                        Box::new(LostTts) as TtsBox,
                        Err("tts load channel closed".into()),
                    );
                }
            };
            let t = std::time::Instant::now();
            let r = tts.load().map_err(|e| e.to_string());
            if r.is_ok() {
                tracing::info!(ms = t.elapsed().as_millis() as u64, "tts preload ready");
            }
            (tts, r)
        }) {
        Ok(h) => match tx.send(tts) {
            Ok(()) => ModelLoadJob::Thread(h),
            Err(std::sync::mpsc::SendError(tts)) => {
                let _ = h.join();
                ModelLoadJob::Ready(
                    tts,
                    Err("tts load thread exited before receiving model".into()),
                )
            }
        },
        Err(e) => {
            tracing::error!(error = %e, "failed to spawn tts load thread");
            ModelLoadJob::Ready(tts, Err(format!("spawn tts load thread: {e}")))
        }
    }
}

pub(super) fn join_tts_load(job: ModelLoadJob<TtsBox>) -> (TtsBox, Result<(), String>) {
    match job {
        ModelLoadJob::Thread(h) => h.join().unwrap_or_else(|_| {
            tracing::error!("tts load thread panicked");
            (Box::new(LostTts), Err("tts load thread panicked".into()))
        }),
        ModelLoadJob::Ready(tts, r) => (tts, r),
    }
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

/// Build a placeholder STT so a caller can recover from a broken invariant
/// (e.g. an `Option<SttBox>` slot unexpectedly empty) without panicking the
/// engine thread. Every call transcribes to an error; the next real load
/// replaces it as usual.
pub(super) fn lost_stt() -> SttBox {
    Box::new(LostStt)
}

/// Placeholder if the STT handle was lost (load-thread panic or empty slot recovery).
struct LostStt;
impl SpeechToText for LostStt {
    fn transcribe(&mut self, _: &[boris_core::AudioSample]) -> boris_core::Result<String> {
        Err(boris_core::Error::other(
            "STT model unavailable (load-thread panic or slot recovery)",
        ))
    }
}

struct LostTts;
impl TextToSpeech for LostTts {
    fn backend_id(&self) -> &str {
        "lost"
    }

    fn synthesize(&mut self, _: &str) -> boris_core::Result<boris_core::AudioBuffer> {
        Err(boris_core::Error::other(
            "TTS model unavailable (load-thread panic)",
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
    fn backend_id(&self) -> &str {
        "null"
    }

    fn synthesize(&mut self, _: &str) -> boris_core::Result<boris_core::AudioBuffer> {
        Err(boris_core::Error::other("tts-supertone feature disabled"))
    }
}
