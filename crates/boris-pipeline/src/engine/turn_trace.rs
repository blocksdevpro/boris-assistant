use std::path::PathBuf;
use std::time::Instant;

use boris_agent::maintenance::MaintenanceHandle;
use boris_agent::trace::TraceEvent;
use boris_agent::TurnTrace;
use serde_json::Value;

/// Per-iteration trace guard. Every exit path (`continue`, `return`, or normal
/// completion) persists a finalized record through the durable worker lane.
pub(super) struct TurnTraceGuard {
    clock: Instant,
    trace: Option<TurnTrace>,
    maintenance: MaintenanceHandle,
    path: PathBuf,
}

impl TurnTraceGuard {
    pub(super) fn new(
        turn_id: impl Into<String>,
        session_id: Option<String>,
        maintenance: MaintenanceHandle,
        path: PathBuf,
        start_event: &'static str,
    ) -> Self {
        let clock = Instant::now();
        let mut trace = TurnTrace::new(turn_id, session_id);
        trace.mark(clock, start_event, None);
        Self {
            clock,
            trace: Some(trace),
            maintenance,
            path,
        }
    }

    pub(super) fn elapsed_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    pub(super) fn mark(&mut self, name: impl Into<String>, meta: Option<Value>) {
        if let Some(trace) = self.trace.as_mut() {
            trace.mark(self.clock, name, meta);
        }
    }

    pub(super) fn mark_at(
        &mut self,
        name: impl Into<String>,
        t_ms: u64,
        duration_ms: Option<u64>,
        meta: Option<Value>,
    ) {
        if let Some(trace) = self.trace.as_mut() {
            trace.events.push(TraceEvent {
                name: name.into(),
                t_ms,
                duration_ms,
                meta,
            });
        }
    }

    pub(super) fn span(&mut self, name: impl Into<String>, duration_ms: u64, meta: Option<Value>) {
        if let Some(trace) = self.trace.as_mut() {
            trace.span(self.clock, name, duration_ms, meta);
        }
    }
}

impl Drop for TurnTraceGuard {
    fn drop(&mut self) {
        let Some(mut trace) = self.trace.take() else {
            return;
        };
        trace.finalize_generation();
        if let Err(error) = self.maintenance.append_trace(self.path.clone(), trace) {
            tracing::warn!(%error, "turn trace enqueue failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boris_agent::MaintenanceWorker;

    #[test]
    fn drop_enqueues_a_valid_trace() {
        let root = std::env::temp_dir().join(format!("boris-turn-trace-{}", std::process::id()));
        let path = root.join("turns.jsonl");
        let worker = MaintenanceWorker::spawn();
        {
            let mut trace = TurnTraceGuard::new(
                "turn-1",
                Some("session-1".into()),
                worker.handle(),
                path.clone(),
                "wake_hit",
            );
            trace.mark("agent_end", None);
        }
        assert!(worker
            .handle()
            .flush_durable(std::time::Duration::from_secs(2)));
        let line = std::fs::read_to_string(&path).unwrap();
        let parsed: TurnTrace = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed.turn_id, "turn-1");
        worker.shutdown();
        let _ = std::fs::remove_dir_all(&root);
    }
}
