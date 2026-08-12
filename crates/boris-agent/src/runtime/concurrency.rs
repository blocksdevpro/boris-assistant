//! Wave-scheduling batch planner: parallel read-only wave, then sequential writes.

use crate::runtime::RawToolCall;
use crate::tool::Tool;

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
}
