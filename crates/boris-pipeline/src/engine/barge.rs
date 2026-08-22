//! Wake-word barge-in while Boris is talking or working.
//!
//! Talking: Armed's live-mic gate is the wrong test because the 2 s window is
//! full of speaker echo, so playback_z / mismatch_z reject a real "Boris" said
//! over leftover TTS. Barge-in therefore uses a lower wake threshold plus a
//! close-talk energy rise. A false pause still resumes leftover speech.
//!
//! Thinking: there is no TTS in the mic, so the Armed live-mic gate applies.
//! Wake-only (no energy fallback). Work keeps running until STT decides.

use std::time::Instant;

use boris_audio::buffer::SlidingBuffer;
use boris_audio::AUDIO_TARGET_RATE;
use boris_core::ArcAudioBuffer;
use boris_sense::{
    duration_to_samples, WakeWord, VAD_WINDOW_SIZE, WAKEWORD_PROCESSING_INTERVAL,
    WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE,
};

/// Shortest crop we treat as "the user said something" after a barge-in pause.
/// Matches the live-mic enroll floor (~320 ms of Silero speech).
const MIN_BARGE_SPEECH_HOPS: u32 = 10;

/// Mixed TTS + close-talk rarely reaches the Armed 0.5 floor. One score at
/// the Armed threshold is still enough; otherwise need a short streak.
const BARGE_WAKE_THRESHOLD: f32 = 0.28;
const BARGE_WAKE_STREAK: u32 = 2;

/// Soft wake that only counts when the mic is also much louder than echo.
const BARGE_WAKE_SOFT: f32 = 0.16;

/// Hops to learn speaker-echo RMS before treating a jump as close-talk.
const ENERGY_WARMUP_HOPS: u32 = 12;
/// Close-talk is typically several times louder than laptop-speaker echo.
const ENERGY_RATIO: f32 = 2.2;
const ENERGY_MIN_RMS: f32 = 0.028;
/// ~250 ms of louder-than-echo speech.
const ENERGY_STREAK: u32 = 8;
/// Absolute close-talk floor (echo at the mic is rarely this hot).
const ENERGY_CLOSE_RMS: f32 = 0.10;
const ENERGY_CLOSE_STREAK: u32 = 10;
/// Loud talk-over with no usable wake score (~450 ms).
const ENERGY_ONLY_STREAK: u32 = 14;

/// Rolling wake + close-talk energy scorer used from the Talking poll loop.
pub(super) struct BargeWatch<'a> {
    mic: &'a crossbeam_channel::Receiver<ArcAudioBuffer>,
    wake: &'a mut dyn WakeWord,
    window: SlidingBuffer,
    samples_since_score: usize,
    hop: [f32; VAD_WINDOW_SIZE],
    hop_len: usize,
    energy_hops: u32,
    baseline_rms: f32,
    loud_hops: u32,
    close_hops: u32,
    wake_streak: u32,
    last_score: f32,
    max_score: f32,
    last_heartbeat: Instant,
}

impl<'a> BargeWatch<'a> {
    pub(super) fn new(
        mic: &'a crossbeam_channel::Receiver<ArcAudioBuffer>,
        wake: &'a mut dyn WakeWord,
    ) -> Self {
        Self {
            mic,
            wake,
            window: SlidingBuffer::new(WAKEWORD_WINDOW_SIZE),
            samples_since_score: 0,
            hop: [0.0; VAD_WINDOW_SIZE],
            hop_len: 0,
            energy_hops: 0,
            baseline_rms: 0.0,
            loud_hops: 0,
            close_hops: 0,
            wake_streak: 0,
            last_score: 0.0,
            max_score: 0.0,
            last_heartbeat: Instant::now(),
        }
    }

    /// Drain available mic frames and return a barge-in hit window.
    pub(super) fn poll(&mut self) -> Option<boris_core::AudioBuffer> {
        self.poll_hit(false)
    }

    /// Thinking barge-in: wake word only. Close-talk energy is a Talking
    /// fallback because leftover TTS masks the wake; during Thinking there is
    /// no TTS in the mic, so energy-only would fire on a roommate or a video.
    pub(super) fn poll_wake_only(&mut self) -> Option<boris_core::AudioBuffer> {
        self.poll_hit(true)
    }

    /// Drop the rolling window so the same wake crop cannot re-fire.
    pub(super) fn reset(&mut self) {
        self.window = SlidingBuffer::new(WAKEWORD_WINDOW_SIZE);
        self.samples_since_score = 0;
        self.hop_len = 0;
        self.energy_hops = 0;
        self.baseline_rms = 0.0;
        self.loud_hops = 0;
        self.close_hops = 0;
        self.wake_streak = 0;
        self.last_score = 0.0;
        self.max_score = 0.0;
    }

    fn poll_hit(&mut self, wake_only: bool) -> Option<boris_core::AudioBuffer> {
        let score_every = duration_to_samples(WAKEWORD_PROCESSING_INTERVAL, AUDIO_TARGET_RATE);
        let mut energy_hit = false;
        loop {
            match self.mic.try_recv() {
                Ok(frame) => {
                    self.window.push(&frame);
                    self.samples_since_score = self.samples_since_score.saturating_add(frame.len());
                    if self.push_energy(&frame) {
                        energy_hit = true;
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            }
        }

        let mut wake_hit = false;
        while self.window.ready() && self.samples_since_score >= score_every {
            self.samples_since_score = self.samples_since_score.saturating_sub(score_every);
            let pcm = self.window.read();
            match self.wake.predict(&pcm) {
                Ok(score) => {
                    self.last_score = score;
                    if score > self.max_score {
                        self.max_score = score;
                    }
                    if score >= BARGE_WAKE_THRESHOLD {
                        self.wake_streak = self.wake_streak.saturating_add(1);
                    } else {
                        self.wake_streak = 0;
                    }
                    if score >= WAKEWORD_THRESHOLD || self.wake_streak >= BARGE_WAKE_STREAK {
                        wake_hit = true;
                    }
                }
                Err(e) => tracing::debug!(error = %e, "barge-in wake predict failed"),
            }
        }

        if self.last_heartbeat.elapsed() >= std::time::Duration::from_secs(2) {
            tracing::info!(
                max_score = self.max_score,
                last_score = self.last_score,
                baseline_rms = self.baseline_rms,
                loud_hops = self.loud_hops,
                "barge-in listen heartbeat"
            );
            self.max_score = 0.0;
            self.last_heartbeat = Instant::now();
        }

        let energy_only =
            self.loud_hops >= ENERGY_ONLY_STREAK || self.close_hops >= ENERGY_CLOSE_STREAK;
        let energy_with_wake = energy_hit && self.last_score >= BARGE_WAKE_SOFT;
        let hit = if wake_only {
            wake_hit
        } else {
            wake_hit || energy_only || energy_with_wake
        };
        if !hit {
            return None;
        }

        let why = if wake_hit && self.last_score >= WAKEWORD_THRESHOLD {
            "wake"
        } else if wake_hit {
            "wake-streak"
        } else if energy_with_wake {
            "energy+soft-wake"
        } else {
            "close-talk-energy"
        };
        tracing::info!(
            score = self.last_score,
            streak = self.wake_streak,
            baseline_rms = self.baseline_rms,
            loud_hops = self.loud_hops,
            wake_only,
            why,
            "barge-in accepted"
        );
        Some(self.window.read())
    }

    fn push_energy(&mut self, frame: &[f32]) -> bool {
        let mut hit = false;
        for &sample in frame {
            self.hop[self.hop_len] = sample;
            self.hop_len += 1;
            if self.hop_len < VAD_WINDOW_SIZE {
                continue;
            }
            self.hop_len = 0;
            if self.on_energy_hop(rms(&self.hop)) {
                hit = true;
            }
        }
        hit
    }

    fn on_energy_hop(&mut self, rms: f32) -> bool {
        self.energy_hops = self.energy_hops.saturating_add(1);
        if self.energy_hops <= ENERGY_WARMUP_HOPS {
            self.baseline_rms = self.baseline_rms.max(rms);
            return false;
        }
        // Slow follow on quieter hops so a pause in TTS does not lock a high floor.
        if rms < self.baseline_rms {
            self.baseline_rms = self.baseline_rms * 0.92 + rms * 0.08;
        }

        let above_echo = rms >= ENERGY_MIN_RMS
            && self.baseline_rms > 0.0
            && rms >= self.baseline_rms * ENERGY_RATIO;
        if above_echo {
            self.loud_hops = self.loud_hops.saturating_add(1);
        } else {
            self.loud_hops = 0;
        }

        if rms >= ENERGY_CLOSE_RMS {
            self.close_hops = self.close_hops.saturating_add(1);
        } else {
            self.close_hops = 0;
        }

        self.loud_hops >= ENERGY_STREAK || self.close_hops >= ENERGY_CLOSE_STREAK
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Drop queued mic frames so Talking does not leave a stale backlog.
pub(super) fn drain_mic(mic: &crossbeam_channel::Receiver<ArcAudioBuffer>) {
    while mic.try_recv().is_ok() {}
}

/// What to do with leftover speech after a barge-in listen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BargeDecision {
    /// False hit, silence, or the user asked him to keep going.
    Resume,
    /// User only wanted him to stop talking.
    StopTalking,
    /// Real new request — discard leftover and start a turn.
    TakeTurn(String),
}

/// Classify the post-pause utterance. `expect_reply` makes short acks
/// ("yes" / "ok") a new turn so a mid-question barge-in can still answer.
pub(super) fn decide_barge_listen(
    speech_hops: u32,
    transcript: &str,
    expect_reply: bool,
) -> BargeDecision {
    if speech_hops < MIN_BARGE_SPEECH_HOPS {
        return BargeDecision::Resume;
    }
    let normalized = normalize_utterance(transcript);
    if normalized.is_empty() {
        return BargeDecision::Resume;
    }
    let rest = strip_wake_prefix(&normalized);
    if rest.is_empty() || is_continue_phrase(rest) {
        return BargeDecision::Resume;
    }
    if is_stop_phrase(rest) {
        return BargeDecision::StopTalking;
    }
    if expect_reply {
        return BargeDecision::TakeTurn(transcript.trim().to_string());
    }
    if is_short_ack(rest) {
        return BargeDecision::Resume;
    }
    BargeDecision::TakeTurn(transcript.trim().to_string())
}

/// Classify a barge-in while the agent is still working.
///
/// Silence / wake-only / continue → [`BargeDecision::Resume`] (work keeps
/// running). A bare stop → [`BargeDecision::StopTalking`] (cancel the turn).
/// A stop plus a new instruction, or any other request, takes a new turn.
pub(super) fn decide_thinking_barge_listen(speech_hops: u32, transcript: &str) -> BargeDecision {
    if speech_hops < MIN_BARGE_SPEECH_HOPS {
        return BargeDecision::Resume;
    }
    let normalized = normalize_utterance(transcript);
    if normalized.is_empty() {
        return BargeDecision::Resume;
    }
    let rest = strip_wake_prefix(&normalized);
    if rest.is_empty() || is_thinking_continue_phrase(rest) || is_short_ack(rest) {
        return BargeDecision::Resume;
    }
    if is_thinking_stop_only(rest) {
        return BargeDecision::StopTalking;
    }
    BargeDecision::TakeTurn(transcript.trim().to_string())
}

/// New user text after interrupting in-flight work. Keeps the original request
/// as context so "do the python one instead" still makes sense.
pub(super) fn thinking_takeover_text(previous: Option<&str>, barge: &str) -> String {
    let barge = barge.trim();
    let prev = previous
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != barge);
    match prev {
        None => barge.to_string(),
        Some(prev) => format!("{barge}\n\n(Interrupted previous request: {prev})"),
    }
}

/// True when the utterance is only the wake word, so we should wait for a
/// follow-up ("Boris" … "stop") instead of treating it as resume immediately.
#[cfg(test)]
pub(super) fn thinking_needs_followup(transcript: &str) -> bool {
    let normalized = normalize_utterance(transcript);
    if normalized.is_empty() {
        return true;
    }
    strip_wake_prefix(&normalized).is_empty()
}

fn normalize_utterance(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = true;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_space = false;
        } else if (ch.is_whitespace() || matches!(ch, ',' | '.' | '!' | '?' | ';' | ':' | '-'))
            && !last_space
            && !out.is_empty()
        {
            out.push(' ');
            last_space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn strip_wake_prefix(text: &str) -> &str {
    const PREFIXES: &[&str] = &[
        "hey boris ",
        "hi boris ",
        "ok boris ",
        "okay boris ",
        "boris ",
    ];
    for prefix in PREFIXES {
        if let Some(rest) = text.strip_prefix(prefix) {
            return rest;
        }
    }
    if matches!(
        text,
        "boris" | "hey boris" | "hi boris" | "ok boris" | "okay boris"
    ) {
        return "";
    }
    text
}

fn is_continue_phrase(text: &str) -> bool {
    matches!(
        text,
        "continue"
            | "go on"
            | "keep going"
            | "keep talking"
            | "go ahead"
            | "carry on"
            | "never mind"
            | "nevermind"
            | "sorry"
            | "sorry go on"
            | "sorry continue"
            | "wait"
            | "wait no"
            | "thats fine"
            | "that is fine"
            | "its fine"
            | "nothing"
            | "nothing never mind"
    )
}

fn is_stop_phrase(text: &str) -> bool {
    matches!(
        text,
        "stop"
            | "stop talking"
            | "shut up"
            | "be quiet"
            | "quiet"
            | "silence"
            | "enough"
            | "thats enough"
            | "that is enough"
            | "cancel"
    )
}

fn is_thinking_stop_only(text: &str) -> bool {
    if is_bare_thinking_stop(text) {
        return true;
    }
    if let Some(rest) = strip_thinking_stop_lead(text) {
        return rest.is_empty()
            || is_bare_thinking_stop(rest)
            || is_stop_filler(rest)
            || is_thinking_continue_phrase(rest)
            || is_short_ack(rest);
    }
    if let Some(idx) = text.find("not what i said") {
        let after = text[idx + "not what i said".len()..].trim();
        return after.is_empty() || is_stop_filler(after) || is_bare_thinking_stop(after);
    }
    false
}

fn is_bare_thinking_stop(text: &str) -> bool {
    matches!(
        text,
        "stop"
            | "stop talking"
            | "stop that"
            | "stop it"
            | "cancel"
            | "cancel that"
            | "wait"
            | "wait stop"
            | "wait no"
            | "hold on"
            | "hold up"
            | "never mind"
            | "nevermind"
            | "abort"
            | "quit"
            | "enough"
            | "thats enough"
            | "that is enough"
            | "no"
            | "nope"
            | "nah"
            | "wrong"
            | "shut up"
            | "be quiet"
            | "quiet"
            | "silence"
            | "thats not what i said"
            | "that is not what i said"
            | "not what i said"
    )
}

fn strip_thinking_stop_lead(text: &str) -> Option<&str> {
    const LEADS: &[&str] = &[
        "stop ",
        "cancel ",
        "wait ",
        "hold on ",
        "hold up ",
        "never mind ",
        "nevermind ",
        "no ",
        "nope ",
        "nah ",
        "wrong ",
    ];
    for lead in LEADS {
        if let Some(rest) = text.strip_prefix(lead) {
            return Some(rest.trim_start());
        }
    }
    None
}

fn is_stop_filler(text: &str) -> bool {
    matches!(
        text,
        "that" | "it" | "this" | "now" | "please" | "right now" | "please stop"
    )
}

fn is_thinking_continue_phrase(text: &str) -> bool {
    matches!(
        text,
        "continue"
            | "go on"
            | "keep going"
            | "keep working"
            | "keep talking"
            | "go ahead"
            | "carry on"
            | "sorry"
            | "sorry go on"
            | "sorry continue"
            | "thats fine"
            | "that is fine"
            | "its fine"
            | "nothing"
            | "keep at it"
    )
}

fn is_short_ack(text: &str) -> bool {
    matches!(text, "ok" | "okay" | "yes" | "yeah" | "yep" | "sure")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct ScriptedWake {
        scores: Vec<f32>,
        idx: usize,
    }

    impl WakeWord for ScriptedWake {
        fn predict(&mut self, _audio: &[f32]) -> boris_core::Result<f32> {
            let i = self.idx.min(self.scores.len().saturating_sub(1));
            self.idx = self.idx.saturating_add(1);
            Ok(self.scores.get(i).copied().unwrap_or(0.0))
        }
    }

    fn fill_window(tx: &crossbeam_channel::Sender<ArcAudioBuffer>, amp: f32) {
        tx.send(Arc::from(vec![amp; WAKEWORD_WINDOW_SIZE]))
            .expect("send");
    }

    fn hops(amp: f32, n: usize) -> ArcAudioBuffer {
        Arc::from(vec![amp; n * VAD_WINDOW_SIZE])
    }

    #[test]
    fn silence_and_continue_resume() {
        assert_eq!(
            decide_barge_listen(0, "anything", false),
            BargeDecision::Resume
        );
        assert_eq!(decide_barge_listen(12, "  ", false), BargeDecision::Resume);
        assert_eq!(
            decide_barge_listen(12, "Boris", false),
            BargeDecision::Resume
        );
        assert_eq!(
            decide_barge_listen(12, "hey Boris, continue", false),
            BargeDecision::Resume
        );
        assert_eq!(
            decide_barge_listen(12, "never mind", false),
            BargeDecision::Resume
        );
        assert_eq!(decide_barge_listen(12, "ok", false), BargeDecision::Resume);
    }

    #[test]
    fn stop_discards_leftover() {
        assert_eq!(
            decide_barge_listen(12, "stop", false),
            BargeDecision::StopTalking
        );
        assert_eq!(
            decide_barge_listen(12, "Boris shut up", false),
            BargeDecision::StopTalking
        );
        assert_eq!(
            decide_barge_listen(12, "that's enough", false),
            BargeDecision::StopTalking
        );
    }

    #[test]
    fn real_request_takes_a_turn() {
        assert_eq!(
            decide_barge_listen(12, "what's the weather", false),
            BargeDecision::TakeTurn("what's the weather".into())
        );
        assert_eq!(
            decide_barge_listen(12, "Boris, open notes", false),
            BargeDecision::TakeTurn("Boris, open notes".into())
        );
    }

    #[test]
    fn yes_during_a_question_is_a_new_turn() {
        assert_eq!(
            decide_barge_listen(12, "yes", true),
            BargeDecision::TakeTurn("yes".into())
        );
        assert_eq!(
            decide_barge_listen(12, "ok", true),
            BargeDecision::TakeTurn("ok".into())
        );
        assert_eq!(
            decide_barge_listen(12, "continue", true),
            BargeDecision::Resume
        );
    }

    #[test]
    fn strong_wake_fires_without_liveness() {
        let (tx, rx) = crossbeam_channel::unbounded::<ArcAudioBuffer>();
        fill_window(&tx, 0.02);
        let mut wake = ScriptedWake {
            scores: vec![0.9],
            idx: 0,
        };
        let mut watch = BargeWatch::new(&rx, &mut wake);
        assert!(
            watch.poll().is_some(),
            "Armed-threshold wake must pause leftover"
        );
    }

    #[test]
    fn mid_wake_needs_a_streak() {
        let (tx, rx) = crossbeam_channel::unbounded::<ArcAudioBuffer>();
        let mut one = ScriptedWake {
            scores: vec![0.32, 0.05],
            idx: 0,
        };
        let mut watch = BargeWatch::new(&rx, &mut one);
        fill_window(&tx, 0.02);
        assert!(watch.poll().is_none(), "a single mid score must not fire");

        let mut two = ScriptedWake {
            scores: vec![0.32, 0.32],
            idx: 0,
        };
        let mut watch = BargeWatch::new(&rx, &mut two);
        fill_window(&tx, 0.02);
        assert!(
            watch.poll().is_some(),
            "two mid scores in the mixed window confirm barge-in"
        );
    }

    #[test]
    fn below_threshold_does_not_fire() {
        let (tx, rx) = crossbeam_channel::unbounded::<ArcAudioBuffer>();
        fill_window(&tx, 0.02);
        let mut wake = ScriptedWake {
            scores: vec![0.1],
            idx: 0,
        };
        let mut watch = BargeWatch::new(&rx, &mut wake);
        assert!(watch.poll().is_none());
    }

    #[test]
    fn close_talk_energy_fires_after_echo_warmup() {
        let (tx, rx) = crossbeam_channel::unbounded::<ArcAudioBuffer>();
        let mut wake = ScriptedWake {
            scores: vec![0.05],
            idx: 0,
        };
        let mut watch = BargeWatch::new(&rx, &mut wake);
        tx.send(hops(0.02, ENERGY_WARMUP_HOPS as usize)).unwrap();
        assert!(watch.poll().is_none(), "echo warmup must not fire");
        tx.send(hops(0.12, ENERGY_ONLY_STREAK as usize)).unwrap();
        assert!(
            watch.poll().is_some(),
            "sustained close-talk over echo must pause leftover"
        );
    }

    #[test]
    fn thinking_stop_includes_wait_and_never_mind() {
        assert_eq!(
            decide_thinking_barge_listen(12, "stop",),
            BargeDecision::StopTalking
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "Boris wait",),
            BargeDecision::StopTalking
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "never mind",),
            BargeDecision::StopTalking
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "that's not what I said",),
            BargeDecision::StopTalking
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "hold on",),
            BargeDecision::StopTalking
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "cancel that",),
            BargeDecision::StopTalking
        );
    }

    #[test]
    fn thinking_silence_and_continue_keep_the_turn() {
        assert_eq!(
            decide_thinking_barge_listen(0, "stop"),
            BargeDecision::Resume
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "Boris",),
            BargeDecision::Resume
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "hey Boris, continue",),
            BargeDecision::Resume
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "keep going",),
            BargeDecision::Resume
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "ok"),
            BargeDecision::Resume
        );
    }

    #[test]
    fn thinking_new_request_takes_a_turn() {
        assert_eq!(
            decide_thinking_barge_listen(12, "search rust docs instead",),
            BargeDecision::TakeTurn("search rust docs instead".into())
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "Boris open notes",),
            BargeDecision::TakeTurn("Boris open notes".into())
        );
        let redirect = "hey boris, no i dont think thats the right approach please just do xyz";
        assert_eq!(
            decide_thinking_barge_listen(12, redirect),
            BargeDecision::TakeTurn(redirect.into())
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "wait search rust docs instead"),
            BargeDecision::TakeTurn("wait search rust docs instead".into())
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "stop doing rust and write python"),
            BargeDecision::TakeTurn("stop doing rust and write python".into())
        );
        assert_eq!(
            decide_thinking_barge_listen(12, "that's not what I said, grep the other folder"),
            BargeDecision::TakeTurn("that's not what I said, grep the other folder".into())
        );
    }

    #[test]
    fn thinking_takeover_keeps_previous_request() {
        assert_eq!(thinking_takeover_text(None, " do xyz "), "do xyz");
        assert_eq!(
            thinking_takeover_text(Some("write a rust parser"), "do python instead"),
            "do python instead\n\n(Interrupted previous request: write a rust parser)"
        );
    }

    #[test]
    fn thinking_followup_only_after_bare_wake() {
        assert!(thinking_needs_followup("Boris"));
        assert!(thinking_needs_followup("hey boris"));
        assert!(thinking_needs_followup(""));
        assert!(!thinking_needs_followup("Boris stop"));
        assert!(!thinking_needs_followup("continue"));
    }

    #[test]
    fn poll_wake_only_ignores_close_talk_energy() {
        let (tx, rx) = crossbeam_channel::unbounded::<ArcAudioBuffer>();
        let mut wake = ScriptedWake {
            scores: vec![0.05],
            idx: 0,
        };
        let mut watch = BargeWatch::new(&rx, &mut wake);
        tx.send(hops(0.02, ENERGY_WARMUP_HOPS as usize)).unwrap();
        assert!(watch.poll_wake_only().is_none());
        tx.send(hops(0.12, ENERGY_ONLY_STREAK as usize)).unwrap();
        assert!(
            watch.poll_wake_only().is_none(),
            "thinking barge-in must not fire on energy without a wake"
        );
    }

    #[test]
    fn energy_at_playback_start_is_the_baseline() {
        let (tx, rx) = crossbeam_channel::unbounded::<ArcAudioBuffer>();
        let mut wake = ScriptedWake {
            scores: vec![0.05],
            idx: 0,
        };
        let mut watch = BargeWatch::new(&rx, &mut wake);
        tx.send(hops(
            0.08,
            (ENERGY_WARMUP_HOPS + ENERGY_ONLY_STREAK) as usize,
        ))
        .unwrap();
        assert!(
            watch.poll().is_none(),
            "constant speaker echo must not look like barge-in"
        );
    }
}
