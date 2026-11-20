//! Structured per-turn latency trace (wake → STT → LLM → tools → TTS → audio).
//!
//! Audible playback duration is recorded separately and is **not** included in
//! [`TurnTrace::response_generation_ms`].

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One timestamped span or instant on a turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceEvent {
    pub name: String,
    /// Milliseconds since [`TurnTrace::started_at`] (or since process if unset).
    pub t_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// End-to-end turn timeline persisted as one JSONL object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnTrace {
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub started_unix_ms: u64,
    pub events: Vec<TraceEvent>,
    /// Wake → first spoken unit queued (excludes audible playback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_generation_ms: Option<u64>,
    /// Started → Drained of the last play job (not part of generation latency).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_ms: Option<u64>,
    /// End of the user's speech → first audible Boris audio of any kind
    /// (progress line, streamed answer, or the answer after acceptance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_end_to_first_audio_ms: Option<u64>,
    /// End of the user's speech → first audio of the answer itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_end_to_answer_audio_ms: Option<u64>,
    /// `"streamed"` when the answer played while the agent was still
    /// running, `"after_accept"` when it was synthesized after acceptance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_speech: Option<String>,
    /// Model + tool time of the agent across every Thinking pass of the turn
    /// (first pass and confirm/input resumes), excluding audio the host played
    /// while it waited. Set on every turn that ran the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_ms: Option<u64>,
    /// Audio played during Thinking: the streamed answer and progress line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_playback_ms: Option<u64>,
}

/// Events that mean Boris became audible, for `speech_end_to_first_audio_ms`.
pub const AUDIBLE_EVENTS: &[&str] = &[
    "audio_started",
    "progress_audio_started",
    "early_audio_started",
];

impl TurnTrace {
    pub fn new(turn_id: impl Into<String>, session_id: Option<String>) -> Self {
        Self {
            turn_id: turn_id.into(),
            session_id,
            started_unix_ms: unix_ms(),
            events: Vec::new(),
            response_generation_ms: None,
            playback_ms: None,
            speech_end_to_first_audio_ms: None,
            speech_end_to_answer_audio_ms: None,
            answer_speech: None,
            agent_ms: None,
            thinking_playback_ms: None,
        }
    }

    pub fn mark(&mut self, clock: Instant, name: impl Into<String>, meta: Option<Value>) {
        self.events.push(TraceEvent {
            name: name.into(),
            t_ms: clock.elapsed().as_millis() as u64,
            duration_ms: None,
            meta,
        });
    }

    pub fn span(
        &mut self,
        clock: Instant,
        name: impl Into<String>,
        duration_ms: u64,
        meta: Option<Value>,
    ) {
        self.events.push(TraceEvent {
            name: name.into(),
            t_ms: clock.elapsed().as_millis() as u64,
            duration_ms: Some(duration_ms),
            meta,
        });
    }

    /// Generation latency: first `wake_hit` / `speech_start` → `tts_first_chunk` or `audio_queued`.
    pub fn finalize_generation(&mut self) {
        let start = self
            .events
            .iter()
            .find(|e| e.name == "wake_hit" || e.name == "speech_start")
            .map(|e| e.t_ms)
            .or_else(|| self.events.first().map(|e| e.t_ms))
            .unwrap_or(0);
        let end = self
            .events
            .iter()
            .rev()
            .find(|e| {
                matches!(
                    e.name.as_str(),
                    "tts_first_chunk" | "audio_queued" | "llm_end" | "agent_end"
                )
            })
            .map(|e| e.t_ms);
        self.response_generation_ms = end.map(|end| end.saturating_sub(start));
        if let (Some(s), Some(d)) = (
            self.events.iter().find(|e| e.name == "audio_started"),
            self.events.iter().rev().find(|e| e.name == "audio_drained"),
        ) {
            self.playback_ms = Some(d.t_ms.saturating_sub(s.t_ms));
        }
        let speech_end = self
            .events
            .iter()
            .find(|e| e.name == "speech_end")
            .map(|e| e.t_ms);
        let answer_audio = self
            .events
            .iter()
            .find(|e| e.name == "audio_started")
            .map(|e| e.t_ms);
        if let Some(end) = speech_end {
            self.speech_end_to_first_audio_ms = self
                .events
                .iter()
                .filter(|e| AUDIBLE_EVENTS.contains(&e.name.as_str()) && e.t_ms >= end)
                .map(|e| e.t_ms - end)
                .min();
            self.speech_end_to_answer_audio_ms =
                answer_audio.map(|start| start.saturating_sub(end));
        }
        if answer_audio.is_some() {
            let streamed = self.events.iter().any(|e| e.name == "answer_streamed");
            self.answer_speech = Some(if streamed { "streamed" } else { "after_accept" }.into());
        }
    }

    pub fn to_jsonl(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Percentile of a sorted-or-not slice of milliseconds. Empty → 0.
pub fn percentile_ms(values: &[u64], pct: u8) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let pct = pct.min(100) as f64;
    let idx = ((pct / 100.0) * (v.len().saturating_sub(1) as f64)).round() as usize;
    v[idx.min(v.len() - 1)]
}

/// p50 / p95 of response-generation and playback, plus p50 / p90 of
/// speech-end → first audio, from a set of traces.
pub fn summarize_traces(traces: &[TurnTrace]) -> TraceSummary {
    let gen: Vec<u64> = traces
        .iter()
        .filter_map(|t| t.response_generation_ms)
        .collect();
    let play: Vec<u64> = traces.iter().filter_map(|t| t.playback_ms).collect();
    let first: Vec<u64> = traces
        .iter()
        .filter_map(|t| t.speech_end_to_first_audio_ms)
        .collect();
    TraceSummary {
        count: traces.len(),
        generation_p50_ms: percentile_ms(&gen, 50),
        generation_p95_ms: percentile_ms(&gen, 95),
        playback_p50_ms: percentile_ms(&play, 50),
        playback_p95_ms: percentile_ms(&play, 95),
        first_audio_p50_ms: percentile_ms(&first, 50),
        first_audio_p90_ms: percentile_ms(&first, 90),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceSummary {
    pub count: usize,
    pub generation_p50_ms: u64,
    pub generation_p95_ms: u64,
    pub playback_p50_ms: u64,
    pub playback_p95_ms: u64,
    #[serde(default)]
    pub first_audio_p50_ms: u64,
    #[serde(default)]
    pub first_audio_p90_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(name: &str, t_ms: u64) -> TraceEvent {
        TraceEvent {
            name: name.into(),
            t_ms,
            duration_ms: None,
            meta: None,
        }
    }

    #[test]
    fn first_audio_counts_any_audible_event_after_speech_end() {
        let mut t = TurnTrace::new("t1", None);
        for (name, at) in [
            ("wake_hit", 0),
            ("speech_end", 1_000),
            ("progress_audio_started", 9_000),
            ("answer_streamed", 20_000),
            ("audio_started", 21_500),
        ] {
            t.events.push(event(name, at));
        }
        t.finalize_generation();
        assert_eq!(t.speech_end_to_first_audio_ms, Some(8_000));
        assert_eq!(t.speech_end_to_answer_audio_ms, Some(20_500));
        assert_eq!(t.answer_speech.as_deref(), Some("streamed"));

        let mut plain = TurnTrace::new("t2", None);
        plain.events.push(event("speech_end", 500));
        plain.events.push(event("audio_started", 2_500));
        plain.finalize_generation();
        assert_eq!(plain.speech_end_to_first_audio_ms, Some(2_000));
        assert_eq!(plain.answer_speech.as_deref(), Some("after_accept"));
        let summary = summarize_traces(&[t, plain]);
        assert_eq!(summary.first_audio_p50_ms, 8_000);

        // Old trace lines without the new fields still parse.
        let old: TurnTrace =
            serde_json::from_str(r#"{"turn_id":"x","started_unix_ms":1,"events":[]}"#).unwrap();
        assert_eq!(old.speech_end_to_first_audio_ms, None);
    }

    #[test]
    fn generation_excludes_playback() {
        let mut t = TurnTrace::new("t1", None);
        let clock = Instant::now();
        t.events.push(TraceEvent {
            name: "wake_hit".into(),
            t_ms: 0,
            duration_ms: None,
            meta: None,
        });
        t.events.push(TraceEvent {
            name: "tts_first_chunk".into(),
            t_ms: 400,
            duration_ms: Some(20),
            meta: Some(json!({"unit": 0})),
        });
        t.events.push(TraceEvent {
            name: "audio_started".into(),
            t_ms: 410,
            duration_ms: None,
            meta: None,
        });
        t.events.push(TraceEvent {
            name: "audio_drained".into(),
            t_ms: 2410,
            duration_ms: None,
            meta: None,
        });
        t.finalize_generation();
        assert_eq!(t.response_generation_ms, Some(400));
        assert_eq!(t.playback_ms, Some(2000));
        let _ = clock;
    }

    #[test]
    fn agent_timing_fields_round_trip_and_stay_optional() {
        let mut t = TurnTrace::new("t1", None);
        let bare = t.to_jsonl().unwrap();
        assert!(!bare.contains("agent_ms") && !bare.contains("thinking_playback_ms"));

        t.agent_ms = Some(4_200);
        t.thinking_playback_ms = Some(0);
        let line = t.to_jsonl().unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["agent_ms"], json!(4_200));
        assert_eq!(value["thinking_playback_ms"], json!(0));
        let back: TurnTrace = serde_json::from_str(&line).unwrap();
        assert_eq!(back.agent_ms, Some(4_200));
        assert_eq!(back.thinking_playback_ms, Some(0));

        let old: TurnTrace =
            serde_json::from_str(r#"{"turn_id":"x","started_unix_ms":1,"events":[]}"#).unwrap();
        assert_eq!(old.agent_ms, None);
        assert_eq!(old.thinking_playback_ms, None);
    }

    #[test]
    fn percentiles() {
        let vals = [10u64, 20, 30, 40, 50];
        assert_eq!(percentile_ms(&vals, 50), 30);
        assert_eq!(percentile_ms(&vals, 95), 50);
        assert_eq!(percentile_ms(&[], 50), 0);
    }

    #[test]
    fn failed_turn_without_response_does_not_report_zero_latency() {
        let mut trace = TurnTrace::new("failed", None);
        trace.events.push(TraceEvent {
            name: "wake_hit".into(),
            t_ms: 0,
            duration_ms: None,
            meta: None,
        });
        trace.events.push(TraceEvent {
            name: "stt_error".into(),
            t_ms: 25,
            duration_ms: None,
            meta: None,
        });
        trace.finalize_generation();
        assert_eq!(trace.response_generation_ms, None);
    }
}
