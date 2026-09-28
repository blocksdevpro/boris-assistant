//! Wave-scheduling batch planner + keyed concurrency gates.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::runtime::RawToolCall;
use crate::tool::Tool;

/// Enforces the global parallel cap and each tool's `effective_max_concurrency`.
#[derive(Clone)]
pub struct ConcurrencyGate {
    global: Arc<Semaphore>,
    /// Per-tool semaphore plus the `max` it was created with. When a tool
    /// re-registers with a different `max` (e.g. config reload), the entry is
    /// recreated so the gate does not freeze at the first-seen limit.
    per_tool: Arc<std::sync::Mutex<HashMap<String, (Arc<Semaphore>, u32)>>>,
}

pub struct ConcurrencyPermits {
    _tool: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}

impl ConcurrencyGate {
    pub fn new(global_limit: u32) -> Self {
        Self {
            global: Arc::new(Semaphore::new(global_limit.max(1) as usize)),
            per_tool: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    fn tool_sem(&self, name: &str, max: u32) -> Arc<Semaphore> {
        let want = max.max(1);
        let mut map = self.per_tool.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(name) {
            Some((sem, stored)) if *stored == want => sem.clone(),
            _ => {
                // First-seen or limit changed: (re)create. Outstanding permits
                // hold the old `Arc`, so in-flight work still drains correctly;
                // new acquires use the fresh limit.
                let sem = Arc::new(Semaphore::new(want as usize));
                map.insert(name.to_string(), (sem.clone(), want));
                sem
            }
        }
    }

    pub async fn acquire(&self, tool_name: &str, tool_max: u32) -> ConcurrencyPermits {
        let tool = self.tool_sem(tool_name, tool_max);
        let _tool = tool
            .clone()
            .acquire_owned()
            .await
            .expect("concurrency semaphore closed");
        let _global = self
            .global
            .clone()
            .acquire_owned()
            .await
            .expect("global concurrency semaphore closed");
        ConcurrencyPermits { _tool, _global }
    }
}

/// Partition auto-allow calls into read-only (parallel) vs write (sequential).
///
/// Returns `(read_indices, write_indices)` as indices into the original `calls` slice,
/// each preserving original relative order.
pub fn partition_read_write(
    calls: &[RawToolCall],
    tools: &[std::sync::Arc<dyn Tool>],
) -> (Vec<usize>, Vec<usize>) {
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    for (i, call) in calls.iter().enumerate() {
        let ro = tools
            .iter()
            .find(|t| t.name() == call.name)
            .map(|t| t.meta().is_read_only())
            .unwrap_or(false);
        if ro {
            reads.push(i);
        } else {
            writes.push(i);
        }
    }
    (reads, writes)
}

/// Cap how many read-only futures we start at once.
///
/// `max_parallel` is the host-configured ceiling (default 16 via
/// `ToolRuntimeFeatures::max_parallel_tools`, env `BORIS_MAX_PARALLEL_TOOLS`).
/// Returns `min(n, max(1))`: never zero (at least one wave runs), never above
/// the ceiling. Pure function — no scheduling state, safe to call per wave.
pub fn clamp_parallel(n: usize, max_parallel: u32) -> usize {
    n.min(max_parallel.max(1) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Tool, ToolError, ToolMeta, ToolRisk};
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::sync::Arc;

    struct T {
        name: &'static str,
        ro: bool,
    }

    #[async_trait]
    impl Tool for T {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            ""
        }
        fn parameters(&self) -> Value {
            json!({"type":"object"})
        }
        fn meta(&self) -> ToolMeta {
            ToolMeta::with_risk(ToolRisk::Safe).read_only(self.ro)
        }
        async fn execute(
            &self,
            _: &crate::tool_context::ToolCallContext,
            _: Value,
        ) -> Result<String, ToolError> {
            Ok(String::new())
        }
    }

    #[tokio::test]
    async fn keyed_semaphore_serializes_same_tool() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::time::{Duration, Instant};
        let gate = ConcurrencyGate::new(16);
        let inflight = Arc::new(AtomicU32::new(0));
        let max = Arc::new(AtomicU32::new(0));
        let mut joins = Vec::new();
        for _ in 0..4 {
            let gate = gate.clone();
            let inflight = inflight.clone();
            let max = max.clone();
            joins.push(tokio::spawn(async move {
                let _p = gate.acquire("only", 1).await;
                let now = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(30)).await;
                inflight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        let started = Instant::now();
        for j in joins {
            j.await.unwrap();
        }
        assert_eq!(max.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() >= Duration::from_millis(100));
    }

    #[test]
    fn partitions_by_meta() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(T {
                name: "file_read",
                ro: true,
            }),
            Arc::new(T {
                name: "bash",
                ro: false,
            }),
            Arc::new(T {
                name: "glob",
                ro: true,
            }),
        ];
        let calls = vec![
            RawToolCall {
                call_id: "1".into(),
                name: "file_read".into(),
                args: json!({}),
            },
            RawToolCall {
                call_id: "2".into(),
                name: "bash".into(),
                args: json!({}),
            },
            RawToolCall {
                call_id: "3".into(),
                name: "glob".into(),
                args: json!({}),
            },
        ];
        let (r, w) = partition_read_write(&calls, &tools);
        assert_eq!(r, vec![0, 2]);
        assert_eq!(w, vec![1]);
    }

    /// Documented, checked contract relied on by `loop_/tool_batch.rs`'s
    /// `ordered[i].expect(...)`: every call index lands in exactly one of
    /// read_idx/write_idx, so their lengths always sum to `calls.len()`.
    #[test]
    fn partition_covers_every_index_exactly_once() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(T {
                name: "file_read",
                ro: true,
            }),
            Arc::new(T {
                name: "bash",
                ro: false,
            }),
        ];
        // Includes a call for an unknown tool name (not in `tools`) — must
        // still land in exactly one partition (writes, via the `unwrap_or(false)`
        // fallback), not be dropped or double-counted.
        let calls = vec![
            RawToolCall {
                call_id: "1".into(),
                name: "file_read".into(),
                args: json!({}),
            },
            RawToolCall {
                call_id: "2".into(),
                name: "bash".into(),
                args: json!({}),
            },
            RawToolCall {
                call_id: "3".into(),
                name: "unknown_tool".into(),
                args: json!({}),
            },
        ];
        let (r, w) = partition_read_write(&calls, &tools);
        assert_eq!(r.len() + w.len(), calls.len());
        let mut all: Vec<usize> = r.iter().chain(w.iter()).copied().collect();
        all.sort_unstable();
        assert_eq!(all, (0..calls.len()).collect::<Vec<_>>());
    }

    #[test]
    fn per_tool_limit_updates_on_reregister() {
        // The gate must not freeze at the first-seen max: re-registering the
        // same tool with a different limit recreates the semaphore.
        let gate = ConcurrencyGate::new(16);
        let first = gate.tool_sem("flex", 1);
        assert_eq!(first.available_permits(), 1);
        let same = gate.tool_sem("flex", 1);
        assert_eq!(same.available_permits(), 1);
        let grown = gate.tool_sem("flex", 4);
        assert_eq!(
            grown.available_permits(),
            4,
            "re-register with max=4 must recreate the semaphore, not reuse max=1"
        );
        let shrunk = gate.tool_sem("flex", 2);
        assert_eq!(shrunk.available_permits(), 2);
    }

    #[test]
    fn clamp_parallel_never_zero_never_above_ceiling() {
        assert_eq!(clamp_parallel(0, 16), 0);
        assert_eq!(clamp_parallel(100, 16), 16);
        assert_eq!(clamp_parallel(4, 0), 1, "max=0 clamps to min 1");
        assert_eq!(clamp_parallel(4, 16), 4);
    }
}
