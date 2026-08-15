//! Background maintenance: durable session appends plus bounded best-effort work.
//!
//! Session-memory appends have their own lossless lane, so a slow LLM profile
//! extract cannot fill the general queue and discard conversation history.
//! Flushes wait on completion watermarks rather than queueing control messages.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use boris_ai::LlmClient;
use tracing::{info, warn};

use crate::memory::{
    extract_heuristic, extract_with_llm, should_llm_extract, MemoryIndex, ProfileStore,
    SessionMemoryTarget, UserProfile,
};
use crate::session::store::{SessionStore, SyncCursor};
use crate::session::types::SessionId;
use crate::tools::memory_tools::SharedLongTermMemory;
use crate::trace::TurnTrace;

/// Default bounded queue (backpressure once full).
const DEFAULT_QUEUE: usize = 32;
/// How long shutdown waits for in-flight jobs (including one LLM extract).
const FLUSH_TIMEOUT: Duration = Duration::from_secs(8);
/// Bound optional network extraction independently of worker shutdown.
const LLM_EXTRACT_TIMEOUT: Duration = Duration::from_secs(6);

/// One unit of post-turn work.
pub enum MaintenanceJob {
    AppendTurn {
        ltm: SharedLongTermMemory,
        target: SessionMemoryTarget,
        user: String,
        assistant: String,
    },
    ExtractPersonal {
        store: ProfileStore,
        profile: Arc<Mutex<UserProfile>>,
        llm_extract: bool,
        user: String,
        assistant: String,
        tools_used: Vec<String>,
        client: Arc<dyn LlmClient>,
    },
    SyncSession {
        store: SessionStore,
        id: SessionId,
        messages: Vec<(String, serde_json::Value)>,
        cursor: SyncCursor,
        /// Optional reply with the persisted cursor (tests / callers that wait).
        reply: Option<mpsc::Sender<Result<SyncCursor, String>>>,
    },
    AppendTrace {
        path: PathBuf,
        trace: TurnTrace,
    },
    IndexMemory {
        index: Arc<MemoryIndex>,
        path: String,
        body: String,
        source: String,
        salience: u32,
    },
    RebuildIndex {
        index: Arc<MemoryIndex>,
    },
    #[cfg(test)]
    TestBlock {
        started: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    },
    #[cfg(test)]
    TestDurableBlock {
        started: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    },
}

impl MaintenanceJob {
    fn is_durable(&self) -> bool {
        match self {
            Self::AppendTurn { .. } | Self::SyncSession { .. } | Self::AppendTrace { .. } => true,
            #[cfg(test)]
            Self::TestDurableBlock { .. } => true,
            _ => false,
        }
    }
}

struct LaneState {
    queue: VecDeque<MaintenanceJob>,
    capacity: Option<usize>,
    accepting: bool,
    shutdown: bool,
    submitted: u64,
    completed: u64,
    stopped: bool,
}

struct JobLane {
    state: Mutex<LaneState>,
    changed: Condvar,
}

impl JobLane {
    fn new(capacity: Option<usize>) -> Self {
        Self {
            state: Mutex::new(LaneState {
                queue: VecDeque::new(),
                capacity,
                accepting: true,
                shutdown: false,
                submitted: 0,
                completed: 0,
                stopped: false,
            }),
            changed: Condvar::new(),
        }
    }

    fn submit(&self, job: MaintenanceJob, wait_for_capacity: bool) -> Result<(), String> {
        let mut job = Some(job);
        let mut state = self
            .state
            .lock()
            .map_err(|_| "maintenance queue lock poisoned".to_string())?;
        loop {
            if !state.accepting {
                return Err("maintenance worker shutting down".into());
            }
            let full = state
                .capacity
                .is_some_and(|capacity| state.queue.len() >= capacity);
            if !full {
                state
                    .queue
                    .push_back(job.take().expect("maintenance job submitted once"));
                state.submitted = state.submitted.saturating_add(1);
                self.changed.notify_all();
                return Ok(());
            }
            if !wait_for_capacity {
                return Err("maintenance queue full".into());
            }
            state = self
                .changed
                .wait(state)
                .map_err(|_| "maintenance queue lock poisoned".to_string())?;
        }
    }

    fn target(&self) -> u64 {
        self.state.lock().map(|s| s.submitted).unwrap_or(0)
    }

    fn close(&self) -> u64 {
        let Ok(mut state) = self.state.lock() else {
            return 0;
        };
        state.accepting = false;
        state.shutdown = true;
        let target = state.submitted;
        self.changed.notify_all();
        target
    }

    fn next(&self) -> Option<MaintenanceJob> {
        let mut state = self.state.lock().ok()?;
        loop {
            if let Some(job) = state.queue.pop_front() {
                self.changed.notify_all();
                return Some(job);
            }
            if state.shutdown {
                state.stopped = true;
                self.changed.notify_all();
                return None;
            }
            state = self.changed.wait(state).ok()?;
        }
    }

    fn finish_one(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.completed = state.completed.saturating_add(1);
            self.changed.notify_all();
        }
    }

    fn wait_completed(&self, target: u64, deadline: Instant) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        loop {
            if state.completed >= target {
                return true;
            }
            if state.stopped {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let Ok((next, timeout)) = self.changed.wait_timeout(state, deadline - now) else {
                return false;
            };
            state = next;
            if timeout.timed_out() && state.completed < target {
                return false;
            }
        }
    }

    fn wait_stopped(&self, deadline: Instant) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        loop {
            if state.stopped {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let Ok((next, timeout)) = self.changed.wait_timeout(state, deadline - now) else {
                return false;
            };
            state = next;
            if timeout.timed_out() && !state.stopped {
                return false;
            }
        }
    }
}

/// Cloneable handle used by the agent / pipeline.
#[derive(Clone)]
pub struct MaintenanceHandle {
    general: Arc<JobLane>,
    durable: Arc<JobLane>,
}

impl MaintenanceHandle {
    /// Enqueue work without blocking. Durable session appends use a lossless
    /// dedicated lane; other jobs return `Err` when the bounded lane is full.
    pub fn submit(&self, job: MaintenanceJob) -> Result<(), String> {
        if job.is_durable() {
            self.durable.submit(job, false)
        } else {
            self.general.submit(job, false)
        }
    }

    /// Losslessly enqueue one turn trace for JSONL append.
    pub fn append_trace(&self, path: impl Into<PathBuf>, trace: TurnTrace) -> Result<(), String> {
        self.submit(MaintenanceJob::AppendTrace {
            path: path.into(),
            trace,
        })
    }

    /// Blocking enqueue for compatibility. Durable appends never need to wait;
    /// general work waits for bounded-lane capacity or shutdown.
    pub fn submit_blocking(&self, job: MaintenanceJob) -> Result<(), String> {
        if job.is_durable() {
            self.durable.submit(job, false)
        } else {
            self.general.submit(job, true)
        }
    }

    /// Wait for every job accepted before this call. The timeout covers the
    /// complete operation; no control message needs queue capacity.
    pub fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let durable_target = self.durable.target();
        let general_target = self.general.target();
        self.durable.wait_completed(durable_target, deadline)
            && self.general.wait_completed(general_target, deadline)
    }

    /// Wait only for durable session appends/transcript snapshots.
    pub fn flush_durable(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let target = self.durable.target();
        self.durable.wait_completed(target, deadline)
    }

    /// Signal shutdown (does not join). Prefer [`MaintenanceWorker::shutdown`].
    pub fn request_shutdown(&self) {
        self.durable.close();
        self.general.close();
    }
}

/// Owns independent durable and best-effort maintenance threads.
pub struct MaintenanceWorker {
    handle: MaintenanceHandle,
    general_thread: Option<JoinHandle<()>>,
    durable_thread: Option<JoinHandle<()>>,
}

impl MaintenanceWorker {
    pub fn spawn() -> Self {
        Self::spawn_with_capacity(DEFAULT_QUEUE)
    }

    pub fn spawn_with_capacity(capacity: usize) -> Self {
        let general = Arc::new(JobLane::new(Some(capacity.max(1))));
        let durable = Arc::new(JobLane::new(None));

        let general_lane = Arc::clone(&general);
        let general_thread = thread::Builder::new()
            .name("boris-maintenance".into())
            .spawn(move || run_worker(general_lane, true))
            .expect("spawn maintenance worker");
        let durable_lane = Arc::clone(&durable);
        let durable_thread = thread::Builder::new()
            .name("boris-maintenance-durable".into())
            .spawn(move || run_worker(durable_lane, false))
            .expect("spawn durable maintenance worker");
        Self {
            handle: MaintenanceHandle { general, durable },
            general_thread: Some(general_thread),
            durable_thread: Some(durable_thread),
        }
    }

    pub fn handle(&self) -> MaintenanceHandle {
        self.handle.clone()
    }

    /// Flush remaining jobs then join (session end / engine shutdown).
    pub fn shutdown(mut self) {
        let _ = self.finish_shutdown(FLUSH_TIMEOUT);
    }

    /// Close both lanes and wait up to `timeout` for accepted work to finish.
    /// Threads that exceed the deadline are detached rather than blocking the
    /// engine forever. The durable lane continues draining after detachment.
    pub fn shutdown_with_timeout(mut self, timeout: Duration) -> bool {
        self.finish_shutdown(timeout)
    }

    fn finish_shutdown(&mut self, timeout: Duration) -> bool {
        if self.general_thread.is_none() && self.durable_thread.is_none() {
            return true;
        }
        let deadline = Instant::now() + timeout;
        // Close first: no accepted job can appear after these watermarks.
        let durable_target = self.handle.durable.close();
        let general_target = self.handle.general.close();

        // Give durable disk writes priority over optional/profile work.
        let durable_drained = self.handle.durable.wait_completed(durable_target, deadline);
        let durable_stopped = self.handle.durable.wait_stopped(deadline);
        let general_drained = self.handle.general.wait_completed(general_target, deadline);
        let general_stopped = self.handle.general.wait_stopped(deadline);

        reap_thread(
            &mut self.durable_thread,
            durable_stopped,
            "durable maintenance",
        );
        reap_thread(
            &mut self.general_thread,
            general_stopped,
            "general maintenance",
        );
        durable_drained && durable_stopped && general_drained && general_stopped
    }
}

impl Drop for MaintenanceWorker {
    fn drop(&mut self) {
        let _ = self.finish_shutdown(FLUSH_TIMEOUT);
    }
}

fn reap_thread(thread: &mut Option<JoinHandle<()>>, stopped: bool, lane: &str) {
    let Some(thread) = thread.take() else {
        return;
    };
    if stopped {
        if let Err(e) = thread.join() {
            warn!(?e, lane, "maintenance thread panicked");
        }
    } else {
        warn!(
            lane,
            "maintenance shutdown deadline exceeded; detaching thread"
        );
        drop(thread);
    }
}

fn run_worker(lane: Arc<JobLane>, with_runtime: bool) {
    let rt = with_runtime
        .then(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .thread_name("boris-maint-rt")
                .build()
                .ok()
        })
        .flatten();

    let mut last_error: Option<String> = None;
    while let Some(job) = lane.next() {
        if let Err(e) = run_job(job, rt.as_ref()) {
            warn!(error = %e, "maintenance job failed");
            last_error = Some(e);
        }
        lane.finish_one();
    }
    info!(last_error = ?last_error, "maintenance worker stopped");
}

fn run_job(job: MaintenanceJob, rt: Option<&tokio::runtime::Runtime>) -> Result<(), String> {
    match job {
        MaintenanceJob::AppendTurn {
            ltm,
            target,
            user,
            assistant,
        } => ltm.append_turn_to(&target, &user, &assistant),
        MaintenanceJob::ExtractPersonal {
            store,
            profile,
            llm_extract,
            user,
            assistant,
            tools_used,
            client,
        } => run_extract(
            store,
            profile,
            llm_extract,
            &user,
            &assistant,
            &tools_used,
            client,
            rt,
        ),
        MaintenanceJob::SyncSession {
            store,
            id,
            messages,
            cursor: _,
            reply,
        } => {
            let result = store.sync_messages_from_persisted(&id, &messages);
            if let Some(tx) = reply {
                let _ = tx.send(result.clone());
            }
            result.map(|_| ())
        }
        MaintenanceJob::AppendTrace { path, trace } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create trace directory {}: {e}", parent.display()))?;
            }
            let mut line = trace
                .to_jsonl()
                .map_err(|e| format!("serialize turn trace: {e}"))?;
            line.push('\n');
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| format!("open turn trace {}: {e}", path.display()))?;
            file.write_all(line.as_bytes())
                .map_err(|e| format!("append turn trace {}: {e}", path.display()))
        }
        MaintenanceJob::IndexMemory {
            index,
            path,
            body,
            source,
            salience,
        } => index.upsert(&path, &body, &source, salience),
        MaintenanceJob::RebuildIndex { index } => index.rebuild(),
        #[cfg(test)]
        MaintenanceJob::TestBlock { started, release }
        | MaintenanceJob::TestDurableBlock { started, release } => {
            let _ = started.send(());
            let _ = release.recv();
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_extract(
    store: ProfileStore,
    profile: Arc<Mutex<UserProfile>>,
    llm_extract: bool,
    user: &str,
    assistant: &str,
    tools_used: &[String],
    client: Arc<dyn LlmClient>,
    rt: Option<&tokio::runtime::Runtime>,
) -> Result<(), String> {
    let started = Instant::now();
    let mut delta = extract_heuristic(user);
    let heuristic_hit = !delta.is_empty();

    let (turns_seen, profile_summary, do_llm) = {
        let mut p = profile
            .lock()
            .map_err(|_| "personal profile lock poisoned".to_string())?;
        p.turns_seen = p.turns_seen.saturating_add(1);
        let turns_seen = p.turns_seen;
        let summary = if p.is_empty() {
            "(empty)".to_string()
        } else {
            p.render_block(400)
        };
        let do_llm = llm_extract && should_llm_extract(user, tools_used, turns_seen, heuristic_hit);
        (turns_seen, summary, do_llm)
    };

    if do_llm {
        if let Some(rt) = rt {
            match rt.block_on(tokio::time::timeout(
                LLM_EXTRACT_TIMEOUT,
                extract_with_llm(client.as_ref(), user, assistant, &profile_summary),
            )) {
                Ok(Ok(llm_delta)) if !llm_delta.is_empty() => {
                    if let Some(n) = llm_delta.preferred_name.clone() {
                        delta.preferred_name = Some(n);
                    }
                    if let Some(a) = llm_delta.address_as.clone() {
                        delta.address_as = Some(a);
                    }
                    delta.preferences_add.extend(llm_delta.preferences_add);
                    delta.facts_add.extend(llm_delta.facts_add);
                    delta
                        .facts_remove_query
                        .extend(llm_delta.facts_remove_query);
                    delta.ongoing_add.extend(llm_delta.ongoing_add);
                    if llm_delta.ongoing_replace.is_some() {
                        delta.ongoing_replace = llm_delta.ongoing_replace;
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    warn!(
                        error = %e,
                        turns_seen,
                        ms = started.elapsed().as_millis() as u64,
                        "personal llm extract failed (maintenance)"
                    );
                }
                Err(_) => warn!(
                    turns_seen,
                    timeout_ms = LLM_EXTRACT_TIMEOUT.as_millis() as u64,
                    "personal llm extract timed out (maintenance)"
                ),
            }
        }
    }

    let mut p = profile
        .lock()
        .map_err(|_| "personal profile lock poisoned".to_string())?;
    if !delta.is_empty() {
        delta.apply(&mut p);
    }
    store.save(&p)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::LongTermMemory;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> std::path::PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("boris-maint-{nanos}-{n}-{label}"))
    }

    #[test]
    fn flush_does_not_drop_append() {
        let root = temp_root("append");
        let ltm = LongTermMemory::new(&root);
        ltm.ensure_dirs().unwrap();
        let session = root.join("sess");
        std::fs::create_dir_all(&session).unwrap();
        ltm.set_session_id(Some("sess".into()));
        ltm.set_session_dir(Some(session.clone()));
        let shared: SharedLongTermMemory = Arc::new(ltm);
        let target = shared.capture_session_target().unwrap().unwrap();

        let worker = MaintenanceWorker::spawn_with_capacity(8);
        worker
            .handle()
            .submit_blocking(MaintenanceJob::AppendTurn {
                ltm: shared.clone(),
                target,
                user: "hi".into(),
                assistant: "hello".into(),
            })
            .unwrap();
        assert!(worker.handle().flush(Duration::from_secs(2)));
        worker.shutdown();

        let raw = std::fs::read_to_string(session.join("memory.md")).unwrap();
        assert!(raw.contains("hi"));
        assert!(raw.contains("hello"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn queued_append_keeps_captured_session_after_rebind() {
        let root = temp_root("captured-session");
        let ltm = LongTermMemory::new(&root);
        ltm.ensure_dirs().unwrap();
        let first = root.join("first");
        let second = root.join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        ltm.set_session_id(Some("first".into()));
        ltm.set_session_dir(Some(first.clone()));
        let shared: SharedLongTermMemory = Arc::new(ltm);

        let worker = MaintenanceWorker::spawn_with_capacity(1);
        let handle = worker.handle();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        handle
            .submit(MaintenanceJob::TestDurableBlock {
                started: started_tx,
                release: release_rx,
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let target = shared.capture_session_target().unwrap().unwrap();
        handle
            .submit(MaintenanceJob::AppendTurn {
                ltm: shared.clone(),
                target,
                user: "belongs to first".into(),
                assistant: "captured".into(),
            })
            .unwrap();
        shared.set_session_dir(None);
        shared.set_session_id(None);
        shared.set_session_id(Some("second".into()));
        shared.set_session_dir(Some(second.clone()));

        release_tx.send(()).unwrap();
        assert!(handle.flush_durable(Duration::from_secs(2)));
        worker.shutdown();

        let first_raw = std::fs::read_to_string(first.join("memory.md")).unwrap();
        assert!(first_raw.contains("belongs to first"));
        assert!(first_raw.contains("Session memory — first"));
        assert!(!second.join("memory.md").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn durable_append_survives_full_general_queue() {
        let root = temp_root("full-queue");
        let session = root.join("session");
        std::fs::create_dir_all(&session).unwrap();
        let ltm = LongTermMemory::new(&root);
        ltm.ensure_dirs().unwrap();
        ltm.set_session_id(Some("session".into()));
        ltm.set_session_dir(Some(session.clone()));
        let shared: SharedLongTermMemory = Arc::new(ltm);

        let worker = MaintenanceWorker::spawn_with_capacity(1);
        let handle = worker.handle();
        let (started_one_tx, started_one_rx) = mpsc::channel();
        let (release_one_tx, release_one_rx) = mpsc::channel();
        handle
            .submit(MaintenanceJob::TestBlock {
                started: started_one_tx,
                release: release_one_rx,
            })
            .unwrap();
        started_one_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let (started_two_tx, _started_two_rx) = mpsc::channel();
        let (release_two_tx, release_two_rx) = mpsc::channel();
        handle
            .submit(MaintenanceJob::TestBlock {
                started: started_two_tx,
                release: release_two_rx,
            })
            .unwrap();
        let (rejected_started_tx, _rejected_started_rx) = mpsc::channel();
        let (_rejected_release_tx, rejected_release_rx) = mpsc::channel();
        assert!(handle
            .submit(MaintenanceJob::TestBlock {
                started: rejected_started_tx,
                release: rejected_release_rx,
            })
            .is_err());

        let target = shared.capture_session_target().unwrap().unwrap();
        handle
            .submit(MaintenanceJob::AppendTurn {
                ltm: shared,
                target,
                user: "must persist".into(),
                assistant: "even when full".into(),
            })
            .expect("durable lane is lossless");
        assert!(handle.flush_durable(Duration::from_secs(1)));
        let raw = std::fs::read_to_string(session.join("memory.md")).unwrap();
        assert!(raw.contains("must persist"));

        release_one_tx.send(()).unwrap();
        release_two_tx.send(()).unwrap();
        assert!(handle.flush(Duration::from_secs(2)));
        worker.shutdown();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flush_and_shutdown_deadlines_include_busy_worker() {
        let worker = MaintenanceWorker::spawn_with_capacity(1);
        let handle = worker.handle();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        handle
            .submit(MaintenanceJob::TestBlock {
                started: started_tx,
                release: release_rx,
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let started = Instant::now();
        assert!(!handle.flush(Duration::from_millis(40)));
        assert!(started.elapsed() < Duration::from_millis(250));

        let started = Instant::now();
        assert!(!worker.shutdown_with_timeout(Duration::from_millis(40)));
        assert!(started.elapsed() < Duration::from_millis(250));
        release_tx.send(()).unwrap();
    }

    #[test]
    fn queued_transcript_snapshots_reconcile_from_persisted_cursor() {
        let root = temp_root("transcript");
        let store = SessionStore::new(&root);
        let meta = store.create().unwrap();
        let first = vec![
            ("system".into(), serde_json::json!("prompt")),
            ("user".into(), serde_json::json!("hello")),
        ];
        let second = vec![
            ("system".into(), serde_json::json!("prompt")),
            ("user".into(), serde_json::json!("hello")),
            ("assistant".into(), serde_json::json!("hi")),
        ];

        let worker = MaintenanceWorker::spawn_with_capacity(1);
        let handle = worker.handle();
        for messages in [first, second] {
            handle
                .submit(MaintenanceJob::SyncSession {
                    store: store.clone(),
                    id: meta.id.clone(),
                    messages,
                    // Deliberately stale/invalid: the durable worker must use
                    // the store's actual persisted cursor for queued snapshots.
                    cursor: SyncCursor {
                        count: 999,
                        fingerprint: 42,
                    },
                    reply: None,
                })
                .unwrap();
        }
        assert!(handle.flush_durable(Duration::from_secs(2)));
        worker.shutdown();

        let records = store.load_transcript(&meta.id).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[2].role, "assistant");
        assert_eq!(records[2].content, serde_json::json!("hi"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn trace_jsonl_uses_lossless_durable_lane() {
        let root = temp_root("traces");
        let path = root.join("turns.jsonl");
        let worker = MaintenanceWorker::spawn_with_capacity(1);
        let handle = worker.handle();
        handle
            .append_trace(&path, TurnTrace::new("turn-1", Some("session-1".into())))
            .unwrap();
        handle
            .append_trace(&path, TurnTrace::new("turn-2", Some("session-1".into())))
            .unwrap();
        assert!(handle.flush_durable(Duration::from_secs(2)));
        worker.shutdown();

        let raw = std::fs::read_to_string(&path).unwrap();
        let traces: Vec<TurnTrace> = raw
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0].turn_id, "turn-1");
        assert_eq!(traces[1].turn_id, "turn-2");
        let _ = std::fs::remove_dir_all(&root);
    }
}
