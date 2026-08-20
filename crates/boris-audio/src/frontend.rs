//! Capture front-end: high-pass, AGC, and AEC on the 16 kHz mono tap.
//!
//! Lives on the input worker, after resample, before subscriber fan-out.
//! Far-end TTS is queued from the output worker and consumed in lockstep with
//! capture frames so AEC3 sees the same clock as the mic.

use boris_core::{AudioBuffer, AudioSample, AUDIO_TARGET_RATE};
use sonora::config::{AdaptiveDigital, EchoCanceller, GainController2, HighPassFilter};
use sonora::{AudioProcessing, Config, StreamConfig};

/// 10 ms at [`AUDIO_TARGET_RATE`]. WebRTC APM frame size.
pub const FRAME_SAMPLES: usize = (AUDIO_TARGET_RATE as usize) / 100;

/// Drop render that races more than this ahead of capture (~3 s of TTS).
const MAX_RENDER_PENDING: usize = AUDIO_TARGET_RATE as usize * 3;

/// Starting AEC delay. WASAPI shared-mode buffering plus resample; AEC3
/// delay estimation refines from here. Must be set when AEC is on.
const STREAM_DELAY_MS: i32 = 80;

/// Playback samples (or control) for the echo canceller.
#[derive(Debug)]
pub enum FarEnd {
    /// Mono PCM already at [`AUDIO_TARGET_RATE`].
    Samples(AudioBuffer),
    /// Device is writing silence; keep queued TTS until [`FarEnd::Resume`].
    Pause,
    Resume,
    /// Job flushed / replaced. Drop queued TTS.
    Clear,
}

/// High-pass + AGC2 + AEC3 on 16 kHz mono capture.
pub struct CaptureFrontEnd {
    apm: AudioProcessing,
    capture_pending: AudioBuffer,
    render_pending: AudioBuffer,
    render_scratch: [AudioSample; FRAME_SAMPLES],
    capture_scratch: [AudioSample; FRAME_SAMPLES],
    paused: bool,
    enabled: bool,
}

impl CaptureFrontEnd {
    pub fn new() -> Self {
        Self::with_delay_ms(STREAM_DELAY_MS)
    }

    fn with_delay_ms(delay_ms: i32) -> Self {
        let enabled = frontend_enabled();
        let stream = StreamConfig::new(AUDIO_TARGET_RATE, 1);
        let agc = GainController2 {
            // We do not drive the OS mixer.
            input_volume_controller: false,
            adaptive_digital: Some(AdaptiveDigital::default()),
            ..Default::default()
        };

        let config = Config {
            high_pass_filter: Some(HighPassFilter::default()),
            echo_canceller: Some(EchoCanceller::default()),
            gain_controller2: Some(agc),
            // Neural / aggressive NS hurts modern ASR (Parakeet included).
            noise_suppression: None,
            ..Default::default()
        };

        let mut apm = AudioProcessing::builder()
            .config(config)
            .capture_config(stream)
            .render_config(stream)
            .build();

        if let Err(e) = apm.set_stream_delay_ms(delay_ms) {
            tracing::debug!(error = %e, delay_ms, "AEC stream delay clamped");
        }

        tracing::info!(
            enabled,
            frame = FRAME_SAMPLES,
            delay_ms,
            "capture front-end ready"
        );

        Self {
            apm,
            capture_pending: Vec::with_capacity(FRAME_SAMPLES * 2),
            render_pending: Vec::with_capacity(FRAME_SAMPLES * 8),
            render_scratch: [0.0; FRAME_SAMPLES],
            capture_scratch: [0.0; FRAME_SAMPLES],
            paused: false,
            enabled,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Apply a far-end control or queue TTS for lockstep AEC.
    pub fn apply_far_end(&mut self, msg: FarEnd) {
        if !self.enabled {
            return;
        }
        match msg {
            FarEnd::Samples(pcm) => {
                if pcm.is_empty() {
                    return;
                }
                self.render_pending.extend_from_slice(&pcm);
                if self.render_pending.len() > MAX_RENDER_PENDING {
                    let overflow = self.render_pending.len() - MAX_RENDER_PENDING;
                    self.render_pending.drain(..overflow);
                    tracing::warn!(
                        overflow,
                        "capture front-end: far-end queue overflow — dropped oldest"
                    );
                }
            }
            FarEnd::Pause => self.paused = true,
            FarEnd::Resume => self.paused = false,
            FarEnd::Clear => {
                self.render_pending.clear();
                self.paused = false;
            }
        }
    }

    pub fn drain_far_end(&mut self, rx: &crossbeam_channel::Receiver<FarEnd>) {
        while let Ok(msg) = rx.try_recv() {
            self.apply_far_end(msg);
        }
    }

    /// Run HPF + AGC + AEC on a 16 kHz mono chunk. May return fewer samples
    /// than `input` (holds a partial 10 ms frame). Empty input still consumes
    /// nothing; leftover is emitted on later calls.
    pub fn process_capture(&mut self, input: &[AudioSample]) -> AudioBuffer {
        if !self.enabled {
            return input.to_vec();
        }
        if input.is_empty() && self.capture_pending.is_empty() {
            return Vec::new();
        }

        self.capture_pending.extend_from_slice(input);
        let frames = self.capture_pending.len() / FRAME_SAMPLES;
        if frames == 0 {
            return Vec::new();
        }

        let mut out = Vec::with_capacity(frames * FRAME_SAMPLES);
        for _ in 0..frames {
            self.capture_scratch
                .copy_from_slice(&self.capture_pending[..FRAME_SAMPLES]);
            self.capture_pending.drain(..FRAME_SAMPLES);
            if let Err(e) = self.process_one_frame() {
                tracing::error!(error = %e, "capture front-end frame failed — passing raw");
            }
            out.extend_from_slice(&self.capture_scratch);
        }
        out
    }

    fn process_one_frame(&mut self) -> Result<(), sonora::Error> {
        self.fill_render_frame();
        let mut render_out = [0.0f32; FRAME_SAMPLES];
        self.apm
            .process_render_f32(&[&self.render_scratch], &mut [&mut render_out])?;
        let mut capture_out = [0.0f32; FRAME_SAMPLES];
        self.apm
            .process_capture_f32(&[&self.capture_scratch], &mut [&mut capture_out])?;
        self.capture_scratch.copy_from_slice(&capture_out);
        Ok(())
    }

    fn fill_render_frame(&mut self) {
        self.render_scratch.fill(0.0);
        if self.paused {
            return;
        }
        let n = FRAME_SAMPLES.min(self.render_pending.len());
        if n == 0 {
            return;
        }
        self.render_scratch[..n].copy_from_slice(&self.render_pending[..n]);
        self.render_pending.drain(..n);
    }
}

impl Default for CaptureFrontEnd {
    fn default() -> Self {
        Self::new()
    }
}

fn frontend_enabled() -> bool {
    match std::env::var("BORIS_AUDIO_FRONTEND") {
        Ok(v) => {
            let v = v.trim();
            !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off"))
        }
        Err(_) => true,
    }
}

/// Push a far-end message without blocking the output worker.
pub fn try_send_far_end(tx: &crossbeam_channel::Sender<FarEnd>, msg: FarEnd) {
    match tx.try_send(msg) {
        Ok(()) => {}
        Err(crossbeam_channel::TrySendError::Full(_)) => {
            tracing::warn!("far-end queue full — dropping AEC reference chunk");
        }
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, amp: f32, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| {
                (2.0 * std::f32::consts::PI * freq * i as f32 / AUDIO_TARGET_RATE as f32).sin()
                    * amp
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        if x.is_empty() {
            return 0.0;
        }
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn frame_size_is_10ms() {
        assert_eq!(FRAME_SAMPLES, 160);
        assert_eq!(StreamConfig::new(AUDIO_TARGET_RATE, 1).num_frames(), 160);
    }

    #[test]
    fn empty_input_stays_empty() {
        let mut fe = CaptureFrontEnd::new();
        assert!(fe.process_capture(&[]).is_empty());
    }

    #[test]
    fn partial_frame_is_held() {
        let mut fe = CaptureFrontEnd::new();
        let out = fe.process_capture(&[0.1; 80]);
        assert!(out.is_empty(), "partial 10 ms frame must not emit");
        let out = fe.process_capture(&[0.1; 80]);
        assert_eq!(out.len(), FRAME_SAMPLES);
    }

    #[test]
    fn quiet_tone_is_gained_up() {
        let mut fe = CaptureFrontEnd::new();
        let input = tone(700.0, 0.02, AUDIO_TARGET_RATE as usize);
        let in_rms = rms(&input);
        let out = fe.process_capture(&input);
        assert!(out.len() >= AUDIO_TARGET_RATE as usize - FRAME_SAMPLES);
        // AGC2 needs a bit of speech-like energy before it moves. Use the
        // second half, after the controller has seen the tone.
        let tail = &out[out.len() / 2..];
        let out_rms = rms(tail);
        assert!(
            out_rms > in_rms * 1.5,
            "AGC should lift a quiet close-talk-distance tone: in={in_rms} out={out_rms}"
        );
    }

    #[test]
    fn highpass_kills_sub_bass() {
        let mut fe = CaptureFrontEnd::new();
        // 40 Hz rumble + a mid tone so AGC has something to track.
        let n = AUDIO_TARGET_RATE as usize;
        let rumble = tone(40.0, 0.4, n);
        let mid = tone(1200.0, 0.05, n);
        let input: Vec<f32> = rumble.iter().zip(mid.iter()).map(|(a, b)| a + b).collect();
        let out = fe.process_capture(&input);
        let tail_in = &input[n / 2..];
        let tail_out = &out[out.len() / 2..];
        // Crude spectral proxy: zero-crossing rate rises when rumble dies.
        let zcr = |x: &[f32]| {
            x.windows(2)
                .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
                .count() as f32
                / x.len() as f32
        };
        assert!(
            zcr(tail_out) > zcr(tail_in) * 1.3,
            "HPF should remove 40 Hz; zcr in={} out={}",
            zcr(tail_in),
            zcr(tail_out)
        );
    }

    #[test]
    fn aec_reduces_matched_echo() {
        // Lockstep render/capture: delay is already 0 in this harness.
        let mut fe = CaptureFrontEnd::with_delay_ms(0);
        let n = AUDIO_TARGET_RATE as usize; // 1 s
        let echo = tone(440.0, 0.3, n);
        fe.apply_far_end(FarEnd::Samples(echo.clone()));
        let out = fe.process_capture(&echo);
        // First ~200 ms is convergence; compare the last half-second.
        let start = out.len().saturating_sub(n / 2);
        let residual = rms(&out[start..]);
        let original = rms(&echo[echo.len() / 2..]);
        assert!(
            residual < original * 0.5,
            "AEC should cut a known echo: original={original} residual={residual}"
        );
    }

    #[test]
    fn pause_holds_queued_render() {
        let mut fe = CaptureFrontEnd::new();
        fe.apply_far_end(FarEnd::Samples(vec![0.2; FRAME_SAMPLES * 4]));
        fe.apply_far_end(FarEnd::Pause);
        let _ = fe.process_capture(&[0.0; FRAME_SAMPLES * 2]);
        assert_eq!(fe.render_pending.len(), FRAME_SAMPLES * 4);
        fe.apply_far_end(FarEnd::Resume);
        let _ = fe.process_capture(&[0.0; FRAME_SAMPLES * 2]);
        assert_eq!(fe.render_pending.len(), FRAME_SAMPLES * 2);
    }

    #[test]
    fn clear_drops_queued_render() {
        let mut fe = CaptureFrontEnd::new();
        fe.apply_far_end(FarEnd::Samples(vec![0.2; FRAME_SAMPLES * 3]));
        fe.apply_far_end(FarEnd::Clear);
        assert!(fe.render_pending.is_empty());
    }
}
