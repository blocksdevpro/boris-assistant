//! On-device self-check: proves the voice chain works on this PC without
//! anyone speaking.
//!
//! Boris synthesizes "Hey Boris, what time is it?" in memory (never played)
//! and runs it through the same wake model, VAD, and speech-to-text the engine
//! uses for real turns. That covers the ONNX runtime DLLs, every downloaded
//! model, and the CPU path. A short live-mic listen and the speaker stream
//! state cover the devices. Runs on the engine thread while idle, reusing the
//! engine's own model instances, then releases models per residency policy.

use std::time::{Duration, Instant};

use boris_audio::resampler::Resampler;
use boris_audio::AUDIO_TARGET_RATE;
use boris_sense::{
    duration_to_samples, WakeWord, VAD_WINDOW_SIZE, WAKEWORD_PROCESSING_INTERVAL,
    WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE,
};

use super::models::release_voice_models;
use super::setup::EngineRuntime;
use crate::hear;
use crate::status::{SelfCheckReport, SelfCheckStep, SelfCheckStepId as Id};

/// Spoken (in memory only) through the whole chain.
pub(super) const PHRASE: &str = "Hey Boris, what time is it?";
/// Live-mic listen. Long enough to see a working mic's noise floor.
const MIC_LISTEN: Duration = Duration::from_millis(1500);
/// Silence before the phrase so the 2 s wake window is full, as in real use.
const LEAD_IN: Duration = Duration::from_millis(2500);
/// Silence after the phrase.
const TAIL: Duration = Duration::from_millis(800);
/// Speech the VAD must find in the phrase (~320 ms, the liveness minimum).
const MIN_SPEECH_HOPS: u32 = 10;

fn step(id: Id, started: Instant, ok: bool, detail: impl Into<String>) -> SelfCheckStep {
    SelfCheckStep {
        id,
        ok,
        detail: detail.into(),
        ms: millis(started.elapsed()),
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Audio length of `hops` VAD windows.
fn vad_hops_ms(hops: u32) -> u64 {
    u64::from(hops) * VAD_WINDOW_SIZE as u64 * 1000 / u64::from(AUDIO_TARGET_RATE)
}

fn record(rt: &mut EngineRuntime, report: &mut SelfCheckReport, s: SelfCheckStep) {
    if s.ok {
        tracing::info!(step = %s.id, ms = s.ms, detail = %s.detail, "self-check step ok");
    } else {
        tracing::warn!(step = %s.id, ms = s.ms, detail = %s.detail, "self-check step FAILED");
    }
    report.steps.push(s);
    rt.picture.self_check = Some(report.clone());
    rt.picture.publish();
}

/// Run every step, publishing progress after each one.
pub(super) fn run_self_check(rt: &mut EngineRuntime) {
    tracing::info!("self-check: start");
    let mut report = SelfCheckReport {
        running: true,
        ok: false,
        steps: Vec::new(),
    };
    rt.picture.self_check = Some(report.clone());
    rt.picture.publish();

    let s = check_microphone(rt);
    record(rt, &mut report, s);
    let s = check_speaker(rt);
    record(rt, &mut report, s);

    let (s, phrase) = check_voice(rt);
    record(rt, &mut report, s);
    match phrase {
        Some(pcm) => {
            let s = check_wake(rt, &pcm);
            record(rt, &mut report, s);
            let s = check_speech_detect(rt, &pcm);
            record(rt, &mut report, s);
            let s = check_transcribe(rt, &pcm);
            record(rt, &mut report, s);
        }
        None => {
            for id in [Id::Wake, Id::SpeechDetect, Id::Transcribe] {
                let s = step(
                    id,
                    Instant::now(),
                    false,
                    "Skipped: Boris's voice could not be made",
                );
                record(rt, &mut report, s);
            }
        }
    }

    if rt.residency.should_evict_idle() {
        release_voice_models(rt.stt.as_mut(), rt.tts.as_mut(), "self-check done");
    }
    report.running = false;
    report.ok = report.steps.iter().all(|s| s.ok);
    tracing::info!(ok = report.ok, "self-check: done");
    rt.picture.self_check = Some(report);
    rt.picture.publish();
}

fn check_microphone(rt: &mut EngineRuntime) -> SelfCheckStep {
    let started = Instant::now();
    while rt.mic.try_recv().is_ok() {}
    let deadline = started + MIC_LISTEN;
    let (mut samples, mut peak) = (0usize, 0.0f32);
    while Instant::now() < deadline {
        if let Ok(frame) = rt.mic.recv_timeout(Duration::from_millis(50)) {
            samples += frame.len();
            peak = frame.iter().fold(peak, |m, s| m.max(s.abs()));
        }
    }
    let label = &rt.picture.mic.label;
    let expected = duration_to_samples(MIC_LISTEN, AUDIO_TARGET_RATE);
    if samples < expected / 3 {
        return step(
            Id::Microphone,
            started,
            false,
            format!("No audio is arriving from {label}"),
        );
    }
    if peak <= hear::MIC_SILENT_PEAK {
        return step(
            Id::Microphone,
            started,
            false,
            format!(
                "Only silence from {label}: blocked by Windows privacy, muted, or the wrong mic"
            ),
        );
    }
    step(Id::Microphone, started, true, format!("Hearing {label}"))
}

fn check_speaker(rt: &mut EngineRuntime) -> SelfCheckStep {
    let started = Instant::now();
    let output = rt.audio.health().output();
    let label = &rt.picture.speaker.label;
    if output.failed || output.device_id.is_empty() {
        return step(
            Id::Speaker,
            started,
            false,
            format!("{label} is not working"),
        );
    }
    step(Id::Speaker, started, true, format!("Ready on {label}"))
}

/// Synthesize the phrase and convert it to 16 kHz with silence around it.
fn check_voice(rt: &mut EngineRuntime) -> (SelfCheckStep, Option<Vec<f32>>) {
    let started = Instant::now();
    if !rt.tts.is_loaded() {
        if let Err(e) = rt.tts.load() {
            return (
                step(
                    Id::Voice,
                    started,
                    false,
                    format!("Voice model failed to load: {e}"),
                ),
                None,
            );
        }
    }
    let pcm = match rt.tts.synthesize(PHRASE) {
        Ok(pcm) if !pcm.is_empty() => pcm,
        Ok(_) => {
            return (
                step(Id::Voice, started, false, "Voice model produced no audio"),
                None,
            )
        }
        Err(e) => {
            return (
                step(
                    Id::Voice,
                    started,
                    false,
                    format!("Voice model failed: {e}"),
                ),
                None,
            )
        }
    };
    let speech =
        match Resampler::new(1, rt.tts.sample_rate(), AUDIO_TARGET_RATE).resample_oneshot(&pcm) {
            Ok(speech) => speech,
            Err(e) => {
                return (
                    step(Id::Voice, started, false, format!("Resample failed: {e}")),
                    None,
                )
            }
        };
    let lead = duration_to_samples(LEAD_IN, AUDIO_TARGET_RATE);
    let tail = duration_to_samples(TAIL, AUDIO_TARGET_RATE);
    let mut stream = vec![0.0f32; lead];
    stream.extend_from_slice(&speech);
    stream.extend(std::iter::repeat_n(0.0f32, tail));
    let secs = speech.len() as f32 / AUDIO_TARGET_RATE as f32;
    (
        step(
            Id::Voice,
            started,
            true,
            format!("Made {secs:.1} s of speech"),
        ),
        Some(stream),
    )
}

/// Score the phrase the way `wait_for_wake` does: 2 s window, 80 ms hop.
fn check_wake(rt: &mut EngineRuntime, stream: &[f32]) -> SelfCheckStep {
    let started = Instant::now();
    let hop = duration_to_samples(WAKEWORD_PROCESSING_INTERVAL, AUDIO_TARGET_RATE).max(1);
    let mut best = 0.0f32;
    let mut end = WAKEWORD_WINDOW_SIZE;
    while end <= stream.len() {
        match rt.wake.predict(&stream[end - WAKEWORD_WINDOW_SIZE..end]) {
            Ok(score) => best = best.max(score),
            Err(e) => {
                return step(Id::Wake, started, false, format!("Wake model failed: {e}"));
            }
        }
        end += hop;
    }
    let ok = best >= WAKEWORD_THRESHOLD;
    let detail = if ok {
        format!("Heard \u{201c}Hey Boris\u{201d} (score {best:.2})")
    } else {
        format!("Wake model did not fire (best score {best:.2})")
    };
    step(Id::Wake, started, ok, detail)
}

fn check_speech_detect(rt: &mut EngineRuntime, stream: &[f32]) -> SelfCheckStep {
    let started = Instant::now();
    let crop = hear::crop_speech(&mut rt.vad, stream);
    let ok = crop.speech_hops >= MIN_SPEECH_HOPS;
    let ms = vad_hops_ms(crop.speech_hops);
    let detail = if ok {
        format!("Found {ms} ms of speech")
    } else {
        format!("Speech detector found only {ms} ms of speech")
    };
    step(Id::SpeechDetect, started, ok, detail)
}

fn check_transcribe(rt: &mut EngineRuntime, stream: &[f32]) -> SelfCheckStep {
    let started = Instant::now();
    if !rt.stt.is_loaded() {
        if let Err(e) = rt.stt.load() {
            return step(
                Id::Transcribe,
                started,
                false,
                format!("Speech-to-text model failed to load: {e}"),
            );
        }
    }
    match rt.stt.transcribe(stream) {
        Ok(text) => {
            let ok = transcript_matches(&text);
            let shown = text.trim();
            let detail = if ok {
                format!("Heard \u{201c}{shown}\u{201d}")
            } else if shown.is_empty() {
                "Speech-to-text returned nothing".to_string()
            } else {
                format!("Expected the test phrase, heard \u{201c}{shown}\u{201d}")
            };
            step(Id::Transcribe, started, ok, detail)
        }
        Err(e) => step(
            Id::Transcribe,
            started,
            false,
            format!("Speech-to-text failed: {e}"),
        ),
    }
}

/// The models work when the plain words come back. "Boris" itself is not
/// required: a name can be misheard without anything being broken.
fn transcript_matches(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("time") && (t.contains("what") || t.contains("is it"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_hops_convert_to_audio_time() {
        // Silero: 512 samples at 16 kHz = 32 ms per hop.
        assert_eq!(vad_hops_ms(1), 32);
        assert_eq!(vad_hops_ms(MIN_SPEECH_HOPS), 320);
    }

    #[test]
    fn step_ids_keep_their_wire_names() {
        let wire = serde_json::to_string(&[Id::SpeechDetect, Id::Microphone]).unwrap();
        assert_eq!(wire, r#"["speech_detect","microphone"]"#);
        assert_eq!(Id::SpeechDetect.to_string(), "speech_detect");
    }

    #[test]
    fn transcript_check_accepts_the_phrase_with_a_misheard_name() {
        assert!(transcript_matches("Hey Boris, what time is it?"));
        assert!(transcript_matches("hey boys what time is it"));
        assert!(!transcript_matches(""));
        assert!(!transcript_matches("hello there"));
    }
}
