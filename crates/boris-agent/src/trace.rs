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
}

impl TurnTrace {
    pub fn new(turn_id: impl Into<String>, session_id: Option<String>) -> Self {
        Self {
            turn_id: turn_id.into(),
            session_id,
            started_unix_ms: unix_ms(),
            events: Vec::new(),
            response_generation_ms: None,
            playback_ms: None,
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

/// p50 / p95 of response-generation and playback from a set of traces.
pub fn summarize_traces(traces: &[TurnTrace]) -> TraceSummary {
    let gen: Vec<u64> = traces
        .iter()
        .filter_map(|t| t.response_generation_ms)
        .collect();
    let play: Vec<u64> = traces.iter().filter_map(|t| t.playback_ms).collect();
    TraceSummary {
        count: traces.len(),
        generation_p50_ms: percentile_ms(&gen, 50),
        generation_p95_ms: percentile_ms(&gen, 95),
        playback_p50_ms: percentile_ms(&play, 50),
        playback_p95_ms: percentile_ms(&play, 95),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceSummary {
    pub count: usize,
    pub generation_p50_ms: u64,
    pub generation_p95_ms: u64,
    pub playback_p50_ms: u64,
    pub playback_p95_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
