//! Per-session shell state: persistent working directory and background jobs.
//!
//! A [`ShellSession`] is shared by `bash`, `bash_output`, and `bash_kill`, and
//! owned by the [`crate::Agent`], which binds it on session bind and clears it
//! on unbind.
//!
//! - **cwd**: the last working directory a `bash` call used is reused by the
//!   next call in the same session (an explicit `cwd` arg always wins). A `cd`
//!   inside a command is not tracked — it would need shell-specific output
//!   parsing that is not robust across Git Bash and the PowerShell fallback.
//! - **Background jobs**: `bash` with `run_in_background: true` spawns the
//!   command with stdout+stderr redirected by the OS to
//!   `<session_dir>/jobs/<id>.log` and returns a job id at once. Status is
//!   polled (`try_wait`) when the model asks; there is no reader thread or
//!   task. At most [`MAX_RUNNING_JOBS`] run at a time.
//! - **Cleanup**: jobs are killed (whole process tree) on session change,
//!   unbind, and drop of the session, which happens when the agent is dropped
//!   at app exit. On Windows each job lives in a Job Object with
//!   `KILL_ON_JOB_CLOSE`, so the OS also kills the tree if Boris crashes.
//!
//! These are OS processes managed by the tool runtime, not engine workers:
//! nothing here touches the voice pipeline thread or adds a scheduler.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::output::truncate_output;
use super::CAPTURE_MAX_BYTES;
use crate::tool::{
    require_object, require_string, soft_wrap_text, truncate_tool_result, Tool, ToolError,
    ToolKind, ToolMeta, ToolRisk, DEFAULT_SOFT_WRAP_WIDTH,
};

/// Background jobs allowed to run at the same time per session.
pub const MAX_RUNNING_JOBS: usize = 4;
/// Finished jobs kept for `bash_output` before the oldest is forgotten.
const MAX_KEPT_JOBS: usize = 16;
/// A job whose log grows past this is killed (runaway output guard).
const MAX_LOG_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobExit {
    Code(i32),
    Killed,
}

struct Job {
    id: String,
    seq: u64,
    command: String,
    cwd: PathBuf,
    log_path: PathBuf,
    started: Instant,
    child: Child,
    tree: ProcessTree,
    exit: Option<JobExit>,
    read_offset: u64,
    note: Option<String>,
}

impl Job {
    fn poll(&mut self) -> Option<JobExit> {
        if self.exit.is_none() {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.exit = Some(JobExit::Code(status.code().unwrap_or(-1)));
            }
        }
        self.exit
    }

    fn kill(&mut self) {
        if self.poll().is_some() {
            return;
        }
        self.tree.kill(&mut self.child);
        let _ = self.child.wait();
        self.exit = Some(JobExit::Killed);
    }

    fn status_line(&self) -> String {
        let secs = self.started.elapsed().as_secs();
        match self.exit {
            None => format!("running ({secs}s)"),
            Some(JobExit::Code(c)) => format!("exited with code {c}"),
            Some(JobExit::Killed) => "killed".to_string(),
        }
    }
}

#[derive(Default)]
struct Inner {
    session: Option<String>,
    jobs_dir: Option<PathBuf>,
    cwd: Option<PathBuf>,
    jobs: HashMap<String, Job>,
    next_seq: u64,
}

impl Inner {
    fn kill_all(&mut self) {
        for job in self.jobs.values_mut() {
            job.kill();
        }
        self.jobs.clear();
    }
}

/// Shell state shared by the bash tools of one agent.
#[derive(Default)]
pub struct ShellSession {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for ShellSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellSession").finish_non_exhaustive()
    }
}

impl ShellSession {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Bind to a session. A different session kills the previous session's
    /// jobs and forgets its cwd; rebinding the same session keeps both.
    pub fn bind(&self, session_id: &str, session_dir: &Path) {
        let mut inner = self.lock();
        if inner.session.as_deref() != Some(session_id) {
            inner.kill_all();
            inner.cwd = None;
        }
        inner.session = Some(session_id.to_string());
        inner.jobs_dir = Some(session_dir.join("jobs"));
    }

    /// Session end: kill every job and forget the cwd.
    pub fn unbind(&self) {
        let mut inner = self.lock();
        inner.kill_all();
        inner.session = None;
        inner.jobs_dir = None;
        inner.cwd = None;
    }

    /// Kill every background job (process trees). Returns how many ran.
    pub fn kill_all(&self) -> usize {
        let mut inner = self.lock();
        let running = inner
            .jobs
            .values_mut()
            .map(|j| j.poll().is_none())
            .filter(|running| *running)
            .count();
        inner.kill_all();
        running
    }

    /// Last working directory a `bash` call used in this session.
    pub fn cwd(&self) -> Option<PathBuf> {
        self.lock().cwd.clone()
    }

    pub(crate) fn remember_cwd(&self, cwd: &Path) {
        self.lock().cwd = Some(cwd.to_path_buf());
    }

    /// Number of jobs still running.
    pub fn running_jobs(&self) -> usize {
        self.lock()
            .jobs
            .values_mut()
            .map(|j| j.poll().is_none())
            .filter(|running| *running)
            .count()
    }

    /// Spawn `cmd` (stdio not yet configured) as a background job.
    pub(crate) fn spawn(
        &self,
        mut cmd: std::process::Command,
        command: &str,
        cwd: &Path,
        fallback_dir: &Path,
    ) -> Result<String, ToolError> {
        let mut inner = self.lock();
        let running: Vec<String> = inner
            .jobs
            .values_mut()
            .filter_map(|j| j.poll().is_none().then(|| j.id.clone()))
            .collect();
        if running.len() >= MAX_RUNNING_JOBS {
            let mut ids = running;
            ids.sort();
            return Err(ToolError::failed(format!(
                "{MAX_RUNNING_JOBS} background jobs are already running ({}). \
                 Stop one with bash_kill, or read its output with bash_output, before starting another.",
                ids.join(", ")
            )));
        }
        let dir = inner
            .jobs_dir
            .clone()
            .unwrap_or_else(|| fallback_dir.join("jobs"));
        std::fs::create_dir_all(&dir)
            .map_err(|e| ToolError::failed(format!("create {}: {e}", dir.display())))?;
        inner.next_seq += 1;
        let seq = inner.next_seq;
        let id = format!("job-{seq}");
        let log_path = dir.join(format!("{id}.log"));
        let log = File::create(&log_path)
            .map_err(|e| ToolError::failed(format!("create {}: {e}", log_path.display())))?;
        let log_err = log
            .try_clone()
            .map_err(|e| ToolError::failed(format!("clone log handle: {e}")))?;
        cmd.stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        ProcessTree::prepare(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::failed(format!("spawn failed: {e}")))?;
        let tree = ProcessTree::attach(&child);
        if let Err(e) = &tree {
            // Without a tree handle the job could outlive Boris; refuse it.
            let _ = child.kill();
            let _ = child.wait();
            return Err(ToolError::failed(format!(
                "could not track the background process tree: {e}"
            )));
        }
        let pid = child.id();
        inner.jobs.insert(
            id.clone(),
            Job {
                id: id.clone(),
                seq,
                command: command.to_string(),
                cwd: cwd.to_path_buf(),
                log_path: log_path.clone(),
                started: Instant::now(),
                child,
                tree: tree.expect("checked"),
                exit: None,
                read_offset: 0,
                note: None,
            },
        );
        prune_finished(&mut inner.jobs);
        Ok(format!(
            "Started background job {id} (pid {pid}).\n\
             cwd: {}\n\
             log: {}\n\
             Read new output and status with bash_output {{\"job_id\": \"{id}\"}}; \
             stop it with bash_kill. It is killed automatically when the session ends.",
            cwd.display(),
            log_path.display()
        ))
    }

    /// New output since the last read plus status.
    pub(crate) fn read_new(&self, id: &str) -> Result<String, ToolError> {
        let mut inner = self.lock();
        let known = known_ids(&inner.jobs);
        let job = inner
            .jobs
            .get_mut(id)
            .ok_or_else(|| unknown_job(id, &known))?;
        job.poll();
        let len = std::fs::metadata(&job.log_path)
            .map(|m| m.len())
            .unwrap_or(0);
        if job.exit.is_none() && len > MAX_LOG_BYTES {
            job.kill();
            job.note = Some(format!(
                "killed: log exceeded {} MB",
                MAX_LOG_BYTES / (1024 * 1024)
            ));
        }
        let (text, skipped) = read_from(&job.log_path, job.read_offset, len)?;
        job.read_offset = len.max(job.read_offset);
        let mut out = format!(
            "Job {id}: {}{}\ncommand: {}\ncwd: {}\nlog: {}\n",
            job.status_line(),
            job.note
                .as_deref()
                .map(|n| format!(" ({n})"))
                .unwrap_or_default(),
            job.command,
            job.cwd.display(),
            job.log_path.display()
        );
        if skipped > 0 {
            out.push_str(&format!(
                "[{skipped} older bytes skipped; file_read the log for them]\n"
            ));
        }
        out.push('\n');
        if text.trim().is_empty() {
            out.push_str("(no new output)\n");
        } else {
            out.push_str(&truncate_output(soft_wrap_text(
                &text,
                DEFAULT_SOFT_WRAP_WIDTH,
            )));
        }
        Ok(truncate_tool_result(out))
    }

    /// Kill one job's process tree.
    pub(crate) fn kill_job(&self, id: &str) -> Result<String, ToolError> {
        let mut inner = self.lock();
        let known = known_ids(&inner.jobs);
        let job = inner
            .jobs
            .get_mut(id)
            .ok_or_else(|| unknown_job(id, &known))?;
        if let Some(exit) = job.poll() {
            return Ok(format!(
                "Job {id} was not running ({}). Nothing to kill.",
                match exit {
                    JobExit::Code(c) => format!("exited with code {c}"),
                    JobExit::Killed => "already killed".into(),
                }
            ));
        }
        job.kill();
        Ok(format!(
            "Killed job {id} and its child processes. Log: {}",
            job.log_path.display()
        ))
    }
}

impl Drop for ShellSession {
    fn drop(&mut self) {
        self.lock().kill_all();
    }
}

fn known_ids(jobs: &HashMap<String, Job>) -> Vec<String> {
    let mut v: Vec<(u64, String)> = jobs.values().map(|j| (j.seq, j.id.clone())).collect();
    v.sort();
    v.into_iter().map(|(_, id)| id).collect()
}

fn unknown_job(id: &str, known: &[String]) -> ToolError {
    if known.is_empty() {
        ToolError::failed(format!(
            "No background job `{id}` in this session (none are running)."
        ))
    } else {
        ToolError::failed(format!(
            "No background job `{id}` in this session. Known jobs: {}.",
            known.join(", ")
        ))
    }
}

/// Forget the oldest finished jobs past [`MAX_KEPT_JOBS`].
fn prune_finished(jobs: &mut HashMap<String, Job>) {
    while jobs.len() > MAX_KEPT_JOBS {
        let oldest = jobs
            .values_mut()
            .filter_map(|j| j.poll().is_some().then_some((j.seq, j.id.clone())))
            .min();
        match oldest {
            Some((_, id)) => {
                jobs.remove(&id);
            }
            None => break,
        }
    }
}

/// Read `[offset, len)`; when that exceeds the capture cap keep only the tail.
fn read_from(path: &Path, offset: u64, len: u64) -> Result<(String, u64), ToolError> {
    if len <= offset {
        return Ok((String::new(), 0));
    }
    let available = len - offset;
    let cap = CAPTURE_MAX_BYTES as u64;
    let (start, skipped) = if available > cap {
        (len - cap, available - cap)
    } else {
        (offset, 0)
    };
    let mut f =
        File::open(path).map_err(|e| ToolError::failed(format!("open {}: {e}", path.display())))?;
    f.seek(SeekFrom::Start(start))
        .map_err(|e| ToolError::failed(format!("seek log: {e}")))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.take(len - start)
        .read_to_end(&mut buf)
        .map_err(|e| ToolError::failed(format!("read log: {e}")))?;
    Ok((String::from_utf8_lossy(&buf).into_owned(), skipped))
}

// ── Process-tree handles ─────────────────────────────────────────────────────

#[cfg(windows)]
struct ProcessTree(windows_job::JobHandle);

#[cfg(windows)]
impl ProcessTree {
    fn prepare(_cmd: &mut std::process::Command) {}

    fn attach(child: &Child) -> Result<Self, String> {
        use std::os::windows::io::AsRawHandle;
        windows_job::JobHandle::for_process(child.as_raw_handle()).map(ProcessTree)
    }

    fn kill(&self, child: &mut Child) {
        if !self.0.terminate() {
            let _ = child.kill();
        }
    }
}

#[cfg(windows)]
mod windows_job {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Owned Job Object handle with `KILL_ON_JOB_CLOSE`.
    pub(super) struct JobHandle(HANDLE);

    // SAFETY: a Job Object handle is a kernel handle usable from any thread.
    unsafe impl Send for JobHandle {}
    unsafe impl Sync for JobHandle {}

    impl JobHandle {
        pub(super) fn for_process(
            process: std::os::windows::io::RawHandle,
        ) -> Result<Self, String> {
            // SAFETY: plain Win32 calls on handles we own; failures are checked.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err(format!(
                        "CreateJobObjectW: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                let handle = JobHandle(job);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(format!(
                        "SetInformationJobObject: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                if AssignProcessToJobObject(job, process as HANDLE) == 0 {
                    return Err(format!(
                        "AssignProcessToJobObject: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                Ok(handle)
            }
        }

        /// Kill every process in the job. `false` if the call failed.
        pub(super) fn terminate(&self) -> bool {
            // SAFETY: valid owned handle.
            unsafe { TerminateJobObject(self.0, 1) != 0 }
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {
            // Closing the last handle kills the tree (KILL_ON_JOB_CLOSE).
            // SAFETY: valid owned handle, closed once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
struct ProcessTree(u32);

#[cfg(not(windows))]
impl ProcessTree {
    fn prepare(cmd: &mut std::process::Command) {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    fn attach(child: &Child) -> Result<Self, String> {
        Ok(ProcessTree(child.id()))
    }

    fn kill(&self, child: &mut Child) {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{}", self.0)])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
    }
}

// ── Tools ────────────────────────────────────────────────────────────────────

/// Read new output and status of a background `bash` job.
#[derive(Debug, Clone)]
pub struct BashOutputTool {
    shell: Arc<ShellSession>,
}

impl BashOutputTool {
    pub fn new(shell: Arc<ShellSession>) -> Self {
        Self { shell }
    }
}

#[async_trait]
impl Tool for BashOutputTool {
    fn name(&self) -> &str {
        "bash_output"
    }

    fn description(&self) -> &str {
        "Read a background bash job's output written since your last bash_output call, \
         plus its status (running / exited with code / killed). Jobs come from bash with \
         run_in_background: true."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": {
                    "type": "string",
                    "description": "Job id returned by bash, e.g. job-1"
                }
            },
            "required": ["job_id"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Execute)
            .timeout(Duration::from_secs(10))
            .read_only(true)
            .max_concurrency(4)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let id = require_string(obj, "job_id")?;
        self.shell.read_new(id.trim())
    }
}

/// Stop a background `bash` job and its child processes.
#[derive(Debug, Clone)]
pub struct BashKillTool {
    shell: Arc<ShellSession>,
}

impl BashKillTool {
    pub fn new(shell: Arc<ShellSession>) -> Self {
        Self { shell }
    }
}

#[async_trait]
impl Tool for BashKillTool {
    fn name(&self) -> &str {
        "bash_kill"
    }

    fn description(&self) -> &str {
        "Stop a background bash job (and every process it started) by job id."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": {
                    "type": "string",
                    "description": "Job id returned by bash, e.g. job-1"
                }
            },
            "required": ["job_id"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Execute)
            .timeout(Duration::from_secs(15))
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let id = require_string(obj, "job_id")?.trim().to_string();
        let shell = Arc::clone(&self.shell);
        tokio::task::spawn_blocking(move || shell.kill_job(&id))
            .await
            .map_err(|e| ToolError::failed(format!("kill task failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "boris-jobs-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn wait_until(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..100 {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn read_from_keeps_tail_past_cap() {
        let dir = temp_dir("tail");
        let p = dir.join("log");
        let body = "x".repeat(CAPTURE_MAX_BYTES + 10) + "END";
        std::fs::write(&p, &body).unwrap();
        let (text, skipped) = read_from(&p, 0, body.len() as u64).unwrap();
        assert_eq!(skipped, 13);
        assert!(text.ends_with("END"));
        let (none, _) = read_from(&p, body.len() as u64, body.len() as u64).unwrap();
        assert!(none.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_job_lists_known_ids() {
        let shell = ShellSession::default();
        let err = shell.read_new("job-9").unwrap_err();
        assert!(err.message.contains("none are running"), "{}", err.message);
    }

    #[test]
    fn bind_keeps_cwd_for_same_session_and_resets_for_new() {
        let shell = ShellSession::default();
        let dir = temp_dir("cwd");
        shell.bind("s1", &dir);
        shell.remember_cwd(&dir);
        shell.bind("s1", &dir);
        assert_eq!(shell.cwd(), Some(dir.clone()));
        shell.bind("s2", &dir);
        assert_eq!(shell.cwd(), None);
        shell.remember_cwd(&dir);
        shell.unbind();
        assert_eq!(shell.cwd(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A long-running child is spawned, reported running, then killed.
    #[test]
    fn background_job_runs_reports_and_dies_on_unbind() {
        let dir = temp_dir("job");
        let shell = ShellSession::default();
        shell.bind("s", &dir);
        #[cfg(windows)]
        let mut cmd = {
            let mut c = std::process::Command::new("powershell.exe");
            c.args([
                "-NoProfile",
                "-Command",
                "Write-Output started; Start-Sleep -Seconds 60",
            ]);
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "echo started; sleep 60"]);
            c
        };
        cmd.current_dir(&dir);
        let started = shell.spawn(cmd, "sleepy", &dir, &dir).expect("spawn");
        assert!(started.contains("job-1"), "{started}");
        assert!(dir.join("jobs").join("job-1.log").is_file());

        assert!(
            wait_until(|| shell.read_new("job-1").unwrap().contains("started")),
            "no output"
        );
        let out = shell.read_new("job-1").unwrap();
        assert!(out.contains("running"), "{out}");
        assert!(out.contains("(no new output)"), "offset advanced: {out}");
        assert_eq!(shell.running_jobs(), 1);

        let killed = shell.kill_job("job-1").unwrap();
        assert!(killed.contains("Killed job job-1"), "{killed}");
        assert!(shell.read_new("job-1").unwrap().contains("killed"));
        assert_eq!(shell.running_jobs(), 0);
        shell.unbind();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_job_cap_is_enforced() {
        let dir = temp_dir("cap");
        let shell = ShellSession::default();
        shell.bind("s", &dir);
        let sleeper = || {
            #[cfg(windows)]
            {
                let mut c = std::process::Command::new("powershell.exe");
                c.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 60"]);
                c
            }
            #[cfg(not(windows))]
            {
                let mut c = std::process::Command::new("sleep");
                c.arg("60");
                c
            }
        };
        for _ in 0..MAX_RUNNING_JOBS {
            shell.spawn(sleeper(), "sleep", &dir, &dir).expect("spawn");
        }
        let err = shell.spawn(sleeper(), "sleep", &dir, &dir).unwrap_err();
        assert!(err.message.contains("already running"), "{}", err.message);
        assert_eq!(shell.kill_all(), MAX_RUNNING_JOBS);
        assert_eq!(shell.running_jobs(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
