//! Live STT partials for offline models (Parakeet-style pseudo-streaming).
//!
//! Parakeet TDT is a full-context model with no chunked state, so true
//! streaming is not possible without model surgery. This module implements the
//! cheap imitation instead: while the user is still speaking, re-decode the
//! growing prefix every ~1 s on the engine thread and publish the text as a
//! live partial. The final [`boris_inference::SpeechToText::transcribe`] at
//! VAD endpoint stays authoritative — partials are advisory and may change.
//!
//! Two wins for one cost:
//! 1. The overlay shows words before the turn ends (perceived latency).
//! 2. When a partial is *stable* across consecutive snapshots, the capture
//!    loop may endpoint on a shorter trailing silence
//!    ([`PartialConfig::early_silence_samples`]) instead of the full freeform
//!    window, saving ~300-400 ms of dead air.
//!
//! Cost: up to [`PartialConfig::max_partials_per_turn`] extra full decodes per
//! turn, run inline (the mic channel buffers while a partial decodes).
//! Disabled with `BORIS_STT_PARTIALS=0`. Cold turns (model not yet loaded)
//! skip partials and keep the existing preload-then-decode path.

use std::time::Duration;

use boris_core::AUDIO_TARGET_RATE;

use crate::env_util::env_truthy;

/// Env: master switch for live partials during capture.
pub const PARTIALS_ENV: &str = "BORIS_STT_PARTIALS";
/// Env: snapshot cadence in ms (how often a prefix is re-decoded).
pub const PARTIAL_INTERVAL_ENV: &str = "BORIS_STT_PARTIAL_INTERVAL_MS";
/// Env: minimum recorded audio before the first partial.
pub const PARTIAL_MIN_ENV: &str = "BORIS_STT_PARTIAL_MIN_MS";
/// Env: max prefix re-decodes per turn.
pub const PARTIAL_MAX_ENV: &str = "BORIS_STT_PARTIAL_MAX";

/// Default snapshot cadence: prefix re-decode about once a second.
const DEFAULT_INTERVAL: Duration = Duration::from_millis(900);
/// Default gate: don't burn a decode on sub-second blips.
const DEFAULT_MIN_AUDIO: Duration = Duration::from_millis(1500);
/// Cap on extra decodes per turn (bounds the added Hearing cost).
const DEFAULT_MAX_PARTIALS: u32 = 3;
/// Consecutive identical partials before a transcript counts as stable.
const DEFAULT_STABILITY_NEEDED: u32 = 2;
/// Trailing silence that endpoints a turn with a stable partial.
/// Shorter than the 700 ms freeform window; confirms already use 250 ms.
const DEFAULT_EARLY_SILENCE: Duration = Duration::from_millis(350);

/// Tunables for live partials (see module docs).
#[derive(Debug, Clone, Copy)]
pub struct PartialConfig {
    /// Master switch (`BORIS_STT_PARTIALS`, default on).
    pub enabled: bool,
    /// Minimum recorded samples before the first snapshot.
    pub min_samples: usize,
    /// Recorded-audio growth between snapshots.
    pub interval_samples: usize,
    /// Max extra decodes per turn.
    pub max_partials_per_turn: u32,
    /// Identical partials needed for [`PartialTracker::is_stable`].
    pub stability_needed: u32,
    /// Trailing-silence endpoint budget once stable (samples).
    pub early_silence_samples: usize,
}

impl Default for PartialConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_samples: duration_samples(DEFAULT_MIN_AUDIO),
            interval_samples: duration_samples(DEFAULT_INTERVAL),
            max_partials_per_turn: DEFAULT_MAX_PARTIALS,
            stability_needed: DEFAULT_STABILITY_NEEDED,
            early_silence_samples: duration_samples(DEFAULT_EARLY_SILENCE),
        }
    }
}

impl PartialConfig {
    /// Read env overrides; unknown/malformed values fall back to defaults.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if env_truthy(PARTIALS_ENV) == Some(false) {
            cfg.enabled = false;
        }
        if let Ok(raw) = std::env::var(PARTIAL_INTERVAL_ENV) {
            if let Ok(ms) = raw.trim().parse::<u64>() {
                if (200..=5000).contains(&ms) {
                    cfg.interval_samples = duration_samples(Duration::from_millis(ms));
                }
            }
        }
        if let Ok(raw) = std::env::var(PARTIAL_MIN_ENV) {
            if let Ok(ms) = raw.trim().parse::<u64>() {
                if (500..=10000).contains(&ms) {
                    cfg.min_samples = duration_samples(Duration::from_millis(ms));
                }
            }
        }
        if let Ok(raw) = std::env::var(PARTIAL_MAX_ENV) {
            if let Ok(n) = raw.trim().parse::<u32>() {
                if (1..=24).contains(&n) {
                    cfg.max_partials_per_turn = n;
                }
            }
        }
        cfg
    }
}

fn duration_samples(d: Duration) -> usize {
    let secs = d.as_secs_f64();
    (secs * f64::from(AUDIO_TARGET_RATE)).round() as usize
}

/// Tracks prefix snapshots and partial-text stability for one utterance.
///
/// Pure logic (no audio, no model) so it unit-tests without ONNX.
#[derive(Debug, Default)]
pub struct PartialTracker {
    snapshots_taken: u32,
    samples_at_last_snapshot: usize,
    last_text: String,
    stable_count: u32,
}

impl PartialTracker {
    /// Create a fresh tracker for one capture.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a new prefix snapshot is due.
    ///
    /// True once recorded audio passes `cfg.min_samples` and has grown by
    /// `cfg.interval_samples` since the last snapshot, while the per-turn
    /// decode budget remains.
    pub fn should_snapshot(&self, cfg: &PartialConfig, recorded_samples: usize) -> bool {
        if !cfg.enabled || self.snapshots_taken >= cfg.max_partials_per_turn {
            return false;
        }
        if recorded_samples < cfg.min_samples {
            return false;
        }
        recorded_samples.saturating_sub(self.samples_at_last_snapshot) >= cfg.interval_samples
    }

    /// Record that a snapshot decode ran (even if it errored or was empty).
    pub fn note_snapshot(&mut self, recorded_samples: usize) {
        self.snapshots_taken = self.snapshots_taken.saturating_add(1);
        self.samples_at_last_snapshot = recorded_samples;
    }

    /// Feed a decoded partial; returns `true` when it is non-empty.
    ///
    /// Stability counts consecutive *normalized-identical* non-empty partials.
    /// Empty/errored partials reset the streak (the prefix is not yet stable).
    pub fn note_partial(&mut self, text: &str) -> bool {
        let norm = normalize_partial(text);
        if norm.is_empty() {
            self.stable_count = 0;
            return false;
        }
        if normalize_partial(&self.last_text) == norm {
            self.stable_count = self.stable_count.saturating_add(1);
        } else {
            self.last_text = text.trim().to_string();
            self.stable_count = 1;
        }
        true
    }

    /// Snapshots decoded so far this utterance.
    pub fn snapshots_taken(&self) -> u32 {
        self.snapshots_taken
    }

    /// Latest non-empty partial text (raw, unnormalized).
    pub fn last_text(&self) -> &str {
        &self.last_text
    }

    /// True once the same transcript repeated `cfg.stability_needed` times.
    pub fn is_stable(&self, cfg: &PartialConfig) -> bool {
        !self.last_text.trim().is_empty() && self.stable_count >= cfg.stability_needed.max(1)
    }
}

/// Lowercase alphanumeric fold for stability comparison (mirrors the
/// barge-in utterance normalizer; kept local so `hear` stays UI-agnostic).
fn normalize_partial(text: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> PartialConfig {
        PartialConfig::default()
    }

    #[test]
    fn defaults_match_declared_tuning() {
        let c = PartialConfig::default();
        assert!(c.enabled);
        assert_eq!(c.min_samples, duration_samples(DEFAULT_MIN_AUDIO));
        assert_eq!(c.interval_samples, duration_samples(DEFAULT_INTERVAL));
        assert_eq!(c.max_partials_per_turn, DEFAULT_MAX_PARTIALS);
        assert_eq!(c.stability_needed, DEFAULT_STABILITY_NEEDED);
        assert_eq!(
            c.early_silence_samples,
            duration_samples(DEFAULT_EARLY_SILENCE)
        );
        // Early endpoint must beat the 700 ms freeform window or it saves nothing.
        assert!(c.early_silence_samples < duration_samples(Duration::from_millis(700)));
    }

    #[test]
    fn snapshot_gating_needs_min_then_interval() {
        let c = cfg();
        let t = PartialTracker::new();
        assert!(!t.should_snapshot(&c, 0));
        assert!(!t.should_snapshot(&c, c.min_samples - 1));

        // First snapshot is due once min audio is recorded (last snapshot at 0).
        let mut t = PartialTracker::new();
        assert!(t.should_snapshot(&c, c.min_samples + c.interval_samples));
        t.note_snapshot(c.min_samples + c.interval_samples);
        assert_eq!(t.snapshots_taken(), 1);
        // Needs a full interval of growth before the next one.
        assert!(!t.should_snapshot(&c, c.min_samples + c.interval_samples + 1));
        assert!(t.should_snapshot(&c, c.min_samples + 2 * c.interval_samples));
    }

    #[test]
    fn snapshot_budget_is_capped() {
        let mut c = cfg();
        c.max_partials_per_turn = 1;
        let mut t = PartialTracker::new();
        assert!(t.should_snapshot(&c, usize::MAX));
        t.note_snapshot(usize::MAX);
        assert!(!t.should_snapshot(&c, usize::MAX));
    }

    #[test]
    fn max_partials_env_parses_and_clamps() {
        // from_env honors BORIS_STT_PARTIAL_MAX inside 1..=24; anything else
        // keeps the default. Serializes on the process env via a static lock
        // so parallel tests can't observe a half-set value.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prior = std::env::var(PARTIAL_MAX_ENV).ok();
        let restore = |prior: Option<String>| match prior {
            Some(v) => std::env::set_var(PARTIAL_MAX_ENV, v),
            None => std::env::remove_var(PARTIAL_MAX_ENV),
        };

        std::env::set_var(PARTIAL_MAX_ENV, "8");
        assert_eq!(PartialConfig::from_env().max_partials_per_turn, 8);
        std::env::set_var(PARTIAL_MAX_ENV, "0");
        assert_eq!(
            PartialConfig::from_env().max_partials_per_turn,
            DEFAULT_MAX_PARTIALS
        );
        std::env::set_var(PARTIAL_MAX_ENV, "99");
        assert_eq!(
            PartialConfig::from_env().max_partials_per_turn,
            DEFAULT_MAX_PARTIALS
        );
        std::env::set_var(PARTIAL_MAX_ENV, "nope");
        assert_eq!(
            PartialConfig::from_env().max_partials_per_turn,
            DEFAULT_MAX_PARTIALS
        );
        restore(prior);
    }

    #[test]
    fn stability_counts_repeats_and_resets_on_change_or_empty() {
        let c = cfg();
        let mut t = PartialTracker::new();
        assert!(!t.is_stable(&c));

        assert!(t.note_partial("  Hello, world! "));
        assert!(!t.is_stable(&c));
        // Case/punctuation-insensitive repeat stabilizes.
        assert!(t.note_partial("hello world"));
        assert!(t.is_stable(&c));
        assert_eq!(t.last_text(), "Hello, world!");

        // Change resets to 1.
        assert!(t.note_partial("hello worlds"));
        assert!(!t.is_stable(&c));
        // Empty resets.
        assert!(!t.note_partial("   "));
        assert!(!t.is_stable(&c));
    }

    #[test]
    fn disabled_config_never_snapshots() {
        let mut c = cfg();
        c.enabled = false;
        let t = PartialTracker::new();
        assert!(!t.should_snapshot(&c, usize::MAX));
    }
}
