//! CAM++ / WeSpeaker ONNX: fbank frames → L2-normalized embedding.

use ort::session::Session;
use ort::value::Tensor;

use boris_core::{Error, Result};

use super::fbank::{log_mel_fbank, n_frames, MEL_BINS};
use super::voiceprint::l2_normalize;

/// WeSpeaker CAM++ (and ResNet) ONNX wrapper. One session, engine thread only.
pub struct SpeakerEmbedder {
    session: Session,
    input_name: String,
    output_name: String,
}

impl SpeakerEmbedder {
    pub fn try_new(model_bytes: &[u8]) -> Result<Self> {
        if model_bytes.is_empty() {
            return Err(Error::other("speaker embed: empty ONNX buffer"));
        }
        let session = Session::builder()
            .map_err(|e| Error::other(format!("speaker embed: session builder: {e}")))?
            .commit_from_memory(model_bytes)
            .map_err(|e| {
                Error::other(format!(
                    "speaker embed: load ONNX (bytes={}): {e}",
                    model_bytes.len()
                ))
            })?;

        let inputs: Vec<String> = session
            .inputs()
            .iter()
            .map(|o| o.name().to_string())
            .collect();
        let outputs: Vec<String> = session
            .outputs()
            .iter()
            .map(|o| o.name().to_string())
            .collect();
        let input_name = pick_name(&inputs, &["feats", "feat", "input"])
            .ok_or_else(|| Error::other(format!("speaker embed: no feats input, have {inputs:?}")))?
            .to_string();
        let output_name = pick_name(&outputs, &["embs", "emb", "embedding", "output"])
            .ok_or_else(|| {
                Error::other(format!("speaker embed: no embs output, have {outputs:?}"))
            })?
            .to_string();

        tracing::info!(
            bytes = model_bytes.len(),
            input = %input_name,
            output = %output_name,
            "SpeakerEmbedder ready"
        );
        Ok(Self {
            session,
            input_name,
            output_name,
        })
    }

    /// L2-normalized embedding, or `None` if the crop is too short.
    pub fn embed(&mut self, pcm: &[f32]) -> Result<Option<Vec<f32>>> {
        let Some(feat) = log_mel_fbank(pcm) else {
            return Ok(None);
        };
        let t = n_frames(pcm.len());
        if t == 0 || feat.len() != t * MEL_BINS {
            return Ok(None);
        }

        let input = Tensor::from_array(([1usize, t, MEL_BINS], feat))
            .map_err(|e| Error::other(format!("speaker embed: feats tensor: {e}")))?;

        let outputs = self
            .session
            .run(ort::inputs![self.input_name.as_str() => input])
            .map_err(|e| Error::other(format!("speaker embed: run: {e}")))?;

        let (_, data) = outputs[self.output_name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(|e| Error::other(format!("speaker embed: output: {e}")))?;
        if data.is_empty() || data.iter().any(|x| !x.is_finite()) {
            return Err(Error::other("speaker embed: empty or non-finite embedding"));
        }
        let mut emb = data.to_vec();
        l2_normalize(&mut emb);
        Ok(Some(emb))
    }
}

fn pick_name<'a>(have: &'a [String], prefer: &[&str]) -> Option<&'a str> {
    for want in prefer {
        if let Some(n) = have.iter().find(|n| n.eq_ignore_ascii_case(want)) {
            return Some(n.as_str());
        }
    }
    have.first().map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bytes_fail() {
        let err = match SpeakerEmbedder::try_new(&[]) {
            Ok(_) => panic!("empty ONNX must fail"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("empty"));
    }
}
