//! Thin Silero VAD wrapper (official streaming ONNX).
//!
//! Keep this dumb: the network emits a speech probability; hangover /
//! endpointing stay in the pipeline. Extra energy gates clip soft speech.

use std::fmt;

use ort::session::Session;
use ort::value::{Tensor, TensorRef};

use boris_core::AUDIO_TARGET_RATE;
use boris_core::{AudioSample, Error, Result};

use crate::vad::Vad;

/// Official Silero hop at 16 kHz (32 ms). Pipeline [`crate::vad::VAD_WINDOW_SIZE`] equals this.
pub const SILERO_VAD_FRAME_SAMPLES_16K: usize = 512;
/// Official wrapper context prepended to each hop.
pub const SILERO_VAD_CONTEXT_SAMPLES_16K: usize = 64;
/// Effective `input` width: context + hop.
pub const SILERO_VAD_INPUT_SAMPLES_16K: usize =
    SILERO_VAD_CONTEXT_SAMPLES_16K + SILERO_VAD_FRAME_SAMPLES_16K;
/// LSTM hidden + cell, row-major `[2, 1, 128]`.
pub const SILERO_VAD_STATE_SHAPE: [usize; 3] = [2, 1, 128];
/// Official default; overridable via [`SileroVad::try_new_with_threshold`].
pub const SILERO_SPEECH_THRESHOLD: f32 = 0.5;

const STATE_LEN: usize =
    SILERO_VAD_STATE_SHAPE[0] * SILERO_VAD_STATE_SHAPE[1] * SILERO_VAD_STATE_SHAPE[2];

/// Pipeline rate is fixed at 16 kHz; Silero hop math depends on it.
const _: () = assert!(AUDIO_TARGET_RATE == 16_000);

/// Silero ONNX adapter for mono hops at [`AUDIO_TARGET_RATE`].
///
/// Construct with [`Self::try_new`] after [`crate::init_onnx_runtime`].
pub struct SileroVad {
    session: Session,
    /// LSTM hidden+cell, length 256, row-major `[2, 1, 128]`.
    state: Vec<f32>,
    /// Last 64 samples of the previous hop (zeros at reset).
    context: Vec<f32>,
    /// Reused `[1, 576]` input scratch.
    input_scratch: Vec<f32>,
    threshold: f32,
}

impl SileroVad {
    /// Load from ONNX bytes (desktop embeds the official graph).
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the buffer is empty, ORT cannot build a session,
    /// or the graph is missing the expected named tensors.
    pub fn try_new(model_bytes: &[u8]) -> Result<Self> {
        Self::try_new_with_threshold(model_bytes, SILERO_SPEECH_THRESHOLD)
    }

    /// Same as [`Self::try_new`] with an explicit speech-probability threshold.
    ///
    /// `threshold` must be in `(0, 1]`.
    pub fn try_new_with_threshold(model_bytes: &[u8], threshold: f32) -> Result<Self> {
        if model_bytes.is_empty() {
            return Err(Error::other(
                "silero vad: failed to load ONNX (bytes=0): empty buffer",
            ));
        }
        if !threshold_ok(threshold) {
            return Err(Error::other(format!(
                "silero vad: threshold must be in (0, 1], got {threshold}"
            )));
        }

        tracing::info!(
            bytes = model_bytes.len(),
            threshold,
            hop = SILERO_VAD_FRAME_SAMPLES_16K,
            "SileroVad::try_new — loading ORT session from bytes"
        );

        let session = Session::builder()
            .map_err(|e| Error::other(format!("silero vad: session builder: {e}")))?
            .commit_from_memory(model_bytes)
            .map_err(|e| {
                Error::other(format!(
                    "silero vad: failed to load ONNX (bytes={}): {e}",
                    model_bytes.len()
                ))
            })?;

        require_named_io(&session)?;

        tracing::info!(
            bytes = model_bytes.len(),
            threshold,
            hop = SILERO_VAD_FRAME_SAMPLES_16K,
            "SileroVad ready"
        );

        Ok(Self {
            session,
            state: vec![0.0; STATE_LEN],
            context: vec![0.0; SILERO_VAD_CONTEXT_SAMPLES_16K],
            input_scratch: vec![0.0; SILERO_VAD_INPUT_SAMPLES_16K],
            threshold,
        })
    }

    /// Speech-probability threshold used by [`Vad::predict`].
    pub fn threshold(&self) -> f32 {
        self.threshold
    }
}

impl fmt::Debug for SileroVad {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SileroVad")
            .field("sample_rate_hz", &AUDIO_TARGET_RATE)
            .field("hop_samples", &SILERO_VAD_FRAME_SAMPLES_16K)
            .field("threshold", &self.threshold)
            .finish_non_exhaustive()
    }
}

impl Vad for SileroVad {
    fn reset(&mut self) {
        zero_state(&mut self.state, &mut self.context);
    }

    fn predict(&mut self, audio: &[AudioSample]) -> Result<bool> {
        validate_frame_len(audio.len())?;

        self.input_scratch[..SILERO_VAD_CONTEXT_SAMPLES_16K].copy_from_slice(&self.context);
        self.input_scratch[SILERO_VAD_CONTEXT_SAMPLES_16K..].copy_from_slice(audio);

        let input = TensorRef::from_array_view((
            [1usize, SILERO_VAD_INPUT_SAMPLES_16K],
            self.input_scratch.as_slice(),
        ))
        .map_err(|e| Error::other(format!("silero vad: input tensor: {e}")))?;
        let state = TensorRef::from_array_view((SILERO_VAD_STATE_SHAPE, self.state.as_slice()))
            .map_err(|e| Error::other(format!("silero vad: state tensor: {e}")))?;
        let sr = Tensor::from_array(([1usize], vec![i64::from(AUDIO_TARGET_RATE)]))
            .map_err(|e| Error::other(format!("silero vad: sr tensor: {e}")))?;

        let outputs = self
            .session
            .run(ort::inputs! {
                "input" => input,
                "state" => state,
                "sr" => sr,
            })
            .map_err(|e| Error::other(format!("silero vad prediction failed: {e}")))?;

        let (_, probs) = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(|e| Error::other(format!("silero vad: output: {e}")))?;
        let p = probs.first().copied().unwrap_or(f32::NAN);

        let (_, state_out) = outputs["stateN"]
            .try_extract_tensor::<f32>()
            .map_err(|e| Error::other(format!("silero vad: stateN: {e}")))?;
        if state_out.len() != self.state.len() {
            return Err(Error::other(format!(
                "silero vad: stateN length {} != {}",
                state_out.len(),
                self.state.len()
            )));
        }
        self.state.copy_from_slice(state_out);
        self.context
            .copy_from_slice(&audio[audio.len() - SILERO_VAD_CONTEXT_SAMPLES_16K..]);

        Ok(is_speech(p, self.threshold))
    }
}

/// `true` when `prob` is a finite value at or above `threshold`.
pub(crate) fn is_speech(prob: f32, threshold: f32) -> bool {
    prob.is_finite() && prob >= threshold
}

pub(crate) fn zero_state(state: &mut [f32], context: &mut [f32]) {
    state.fill(0.0);
    context.fill(0.0);
}

fn threshold_ok(threshold: f32) -> bool {
    threshold.is_finite() && threshold > 0.0 && threshold <= 1.0
}

fn require_named_io(session: &Session) -> Result<()> {
    let inputs: Vec<&str> = session.inputs().iter().map(|o| o.name()).collect();
    let outputs: Vec<&str> = session.outputs().iter().map(|o| o.name()).collect();
    for expected in ["input", "state", "sr"] {
        if !inputs.contains(&expected) {
            return Err(Error::other(format!(
                "silero vad: missing input '{expected}', have {inputs:?}"
            )));
        }
    }
    for expected in ["output", "stateN"] {
        if !outputs.contains(&expected) {
            return Err(Error::other(format!(
                "silero vad: missing output '{expected}', have {outputs:?}"
            )));
        }
    }
    Ok(())
}

fn validate_frame_len(len: usize) -> Result<()> {
    if len == SILERO_VAD_FRAME_SAMPLES_16K {
        Ok(())
    } else {
        Err(Error::other(format!(
            "silero vad invalid frame length: got {len} samples, expected {} \
             (32 ms at {} Hz)",
            SILERO_VAD_FRAME_SAMPLES_16K, AUDIO_TARGET_RATE
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::vad::Vad;

    // The desktop embeds this same graph. Making it a compile-time fixture
    // prevents a missing asset or broken ORT/model combination from turning
    // the inference tests into silent no-ops.
    static SILERO_TEST_MODEL: &[u8] =
        include_bytes!("../../../../assets/models/silero/silero_vad.onnx");

    #[test]
    fn is_speech_threshold_edges() {
        assert!(!is_speech(0.49, 0.5));
        assert!(is_speech(0.5, 0.5));
        assert!(is_speech(0.9, 0.5));
        assert!(!is_speech(f32::NAN, 0.5));
        assert!(!is_speech(f32::NEG_INFINITY, 0.5));
        assert!(!is_speech(f32::INFINITY, 0.5));
    }

    #[test]
    fn zero_state_clears() {
        let mut state = vec![1.0; STATE_LEN];
        let mut context = vec![1.0; SILERO_VAD_CONTEXT_SAMPLES_16K];
        zero_state(&mut state, &mut context);
        assert!(state.iter().all(|&x| x == 0.0));
        assert!(context.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn validate_frame_len_messages() {
        let err = validate_frame_len(160).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("160"), "{msg}");
        assert!(msg.contains("512"), "{msg}");
    }

    #[test]
    fn rejects_empty_bytes() {
        let _ort = crate::ort::lock_ort_for_test();
        let err = SileroVad::try_new(&[]).unwrap_err();
        assert!(err.to_string().contains("bytes=0"), "{err}");
    }

    #[test]
    fn rejects_bad_threshold() {
        let _ort = crate::ort::lock_ort_for_test();
        let err = SileroVad::try_new_with_threshold(&[1, 2, 3], 0.0).unwrap_err();
        assert!(err.to_string().contains("threshold"), "{err}");
    }

    fn load_silero() -> SileroVad {
        crate::init_onnx_runtime().expect("desktop-supported CI must initialize ONNX Runtime");
        SileroVad::try_new(SILERO_TEST_MODEL)
            .expect("the embedded desktop Silero graph must load and expose the expected I/O")
    }

    #[test]
    fn silence_512_is_not_speech() {
        let _ort = crate::ort::lock_ort_for_test();
        let mut vad = load_silero();
        let frame = vec![0.0f32; SILERO_VAD_FRAME_SAMPLES_16K];
        for hop in 0..20 {
            let speech = vad.predict(&frame).expect("512-sample frame is valid");
            assert!(!speech, "digital silence classified as speech at hop {hop}");
        }
    }

    #[test]
    fn rejects_160_sample_frame() {
        let _ort = crate::ort::lock_ort_for_test();
        let mut vad = load_silero();
        let err = vad.predict(&vec![0.0f32; 160]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("160"), "{msg}");
        assert!(msg.contains("512"), "{msg}");
    }

    #[test]
    fn reset_clears_context() {
        let _ort = crate::ort::lock_ort_for_test();
        let mut vad = load_silero();
        let loud = vec![0.9f32; SILERO_VAD_FRAME_SAMPLES_16K];
        let _ = vad.predict(&loud);
        vad.reset();
        let silence = vec![0.0f32; SILERO_VAD_FRAME_SAMPLES_16K];
        let speech = vad.predict(&silence).expect("silence after reset");
        assert!(!speech);
    }

    #[test]
    fn reset_replays_a_deterministic_sequence() {
        let _ort = crate::ort::lock_ort_for_test();
        let mut vad = load_silero();
        let frame: Vec<f32> = (0..SILERO_VAD_FRAME_SAMPLES_16K)
            .map(|i| ((i as f32 * 0.037).sin()) * 0.08)
            .collect();
        let first: Vec<bool> = (0..6)
            .map(|_| vad.predict(&frame).expect("first sequence"))
            .collect();
        vad.reset();
        let replay: Vec<bool> = (0..6)
            .map(|_| vad.predict(&frame).expect("replayed sequence"))
            .collect();
        assert_eq!(
            first, replay,
            "reset must clear recurrent state and context"
        );
    }
}
