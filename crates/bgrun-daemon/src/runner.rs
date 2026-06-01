use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bgrun_core::{Job, JobStore};
use bgrun_proto::{JobRecord, JobState, RunArgs};
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{error, info};

use crate::state;

/// Global map of job IDs to their stdin handles (piped mode).
pub static STDIN_HANDLES: once_cell::sync::Lazy<
    Mutex<HashMap<String, tokio::process::ChildStdin>>,
> = once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

/// Global map of job IDs to their PTY master writers (for --pty mode).
/// Wrapped in Arc<std::sync::Mutex> for thread-safe synchronous writes.
type PtyWriter = Box<dyn std::io::Write + Send + 'static>;
pub static PTY_WRITERS: once_cell::sync::Lazy<
    Mutex<HashMap<String, Arc<std::sync::Mutex<PtyWriter>>>>,
> = once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

/// Spawns a new job process and returns its record.
pub async fn spawn_job(args: RunArgs, store: Arc<Mutex<JobStore>>) -> Result<JobRecord> {
    // Idempotency: if a named job is already alive, return it
    if let Some(ref name) = args.name {
        let store_ref = store.lock().await;
        if let Some(existing) = store_ref.find_by_name(name) {
            if existing.is_alive() {
                info!(name = %name, id = %existing.id, "returning existing alive job");
                return Ok(existing.to_record());
            }
        }
    }

    // Dependency: wait for named job to reach Ready (with timeout)
    if let Some(ref dep_name) = args.after {
        info!(dependency = %dep_name, "waiting for dependency");
        let dep_timeout = std::time::Duration::from_secs(120);
        let start = tokio::time::Instant::now();
        loop {
            if start.elapsed() >= dep_timeout {
                anyhow::bail!("dependency '{}' did not become ready within 120s", dep_name);
            }
            let store_ref = store.lock().await;
            match store_ref.find_by_name(dep_name) {
                Some(job) if job.state == JobState::Ready => {
                    info!(dependency = %dep_name, "dependency ready");
                    break;
                }
                Some(job) if job.state == JobState::Exited || job.state == JobState::Crashed || job.state == JobState::Killed => {
                    info!(dependency = %dep_name, state = ?job.state, "dependency finished without ready check");
                    break;
                }
                Some(_) => {
                    drop(store_ref);
                    sleep(Duration::from_millis(100)).await;
                }
                None => {
                    anyhow::bail!("dependency '{}' not found", dep_name);
                }
            }
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    let cmd = args.cmd.clone();

    if cmd.is_empty() {
        anyhow::bail!("command must not be empty");
    }

    let job_dir = state::job_dir(&id);
    tokio::fs::create_dir_all(&job_dir)
        .await
        .context("failed to create job directory")?;

    // PTY mode: allocate a pseudo-terminal and spawn the process via PTY
    if args.pty {
        return spawn_pty_job(args, store, id, job_dir, cmd).await;
    }

    // Piped mode: spawn the child process with piped stdout/stderr
    let mut child_cmd = Command::new(&cmd[0]);
    if cmd.len() > 1 {
        child_cmd.args(&cmd[1..]);
    }
    if let Some(ref cwd) = args.cwd {
        child_cmd.current_dir(cwd);
    }
    child_cmd.process_group(0);
    child_cmd.stdin(std::process::Stdio::piped());
    child_cmd.stdout(std::process::Stdio::piped());
    child_cmd.stderr(std::process::Stdio::piped());

    let mut child = child_cmd.spawn().context("failed to spawn process")?;
    let pid = child
        .id()
        .ok_or_else(|| anyhow::anyhow!("child process did not report a pid after spawn"))?;

    // Store stdin handle for send command
    if let Some(stdin) = child.stdin.take() {
        STDIN_HANDLES.lock().await.insert(id.clone(), stdin);
    }

    // Spawn async task to capture stdout/stderr to log file with rotation
    let stdout_log = job_dir.join("stdout.log");
    if let Some(stdout) = child.stdout.take() {
        let log_path = stdout_log.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            capture_output(stdout, log_path, &id_clone, "stdout").await;
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let log_path = stdout_log;
        let id_clone = id.clone();
        tokio::spawn(async move {
            capture_output(stderr, log_path, &id_clone, "stderr").await;
        });
    }

    // Create Job record
    let mut job = Job::new(id.clone(), cmd.clone(), args.name, args.workspace);
    job.pid = Some(pid);
    job.state = JobState::Running;
    job.readiness = args.readiness.clone();
    job.restart = args.restart.clone();
    job.pty = args.pty;
    job.max_runtime_ms = args.max_runtime_ms;
    job.env = args.env.clone();

    state::write_meta(&job).await?;
    state::write_status(&job).await?;

    // Insert into store
    let record = job.to_record();
    {
        let mut store = store.lock().await;
        store.insert(job);
    }

    // Spawn monitor task
    let store_clone = store.clone();
    let id_clone = id.clone();
    tokio::spawn(async move {
        monitor_job(id_clone, store_clone, child).await;
    });

    // Spawn readiness loop if configured (fixed 60s timeout, independent of max_runtime)
    if let Some(ref strategy) = args.readiness {
        let checker = bgrun_daemon::readiness::build_checker(strategy, &job_dir);
        let store_clone = store.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            bgrun_daemon::readiness::readiness_loop(id_clone, store_clone, checker, 60_000).await;
        });
    }

    // Spawn max runtime timeout if configured
    if let Some(max_ms) = args.max_runtime_ms {
        let store_clone = store.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(max_ms)).await;
            // Check if job is still alive before killing
            let alive = {
                let store_ref = store_clone.lock().await;
                store_ref.get(&id_clone).is_some_and(|j| j.is_alive())
            };
            if alive {
                info!(id = %id_clone, "max runtime reached, killing job");
                if let Err(e) = kill_job(&id_clone, store_clone).await {
                    error!(id = %id_clone, error = %e, "failed to kill job after max runtime");
                }
            }
        });
    }

    info!(pid, id = %id, cmd = %cmd.join(" "), "job spawned");
    Ok(record)
}

/// Handles a job's exit: performs post-exit readiness check, transitions
/// state, cleans up handles, and persists status. Shared by piped and PTY
/// monitor tasks.
async fn handle_job_exit(
    id: String,
    store: Arc<Mutex<JobStore>>,
    exit_code: Option<i32>,
) {
    // Post-exit readiness check: if the job has a LogPattern readiness and
    // exited cleanly before the readiness loop fired, check the log and
    // transition through Ready first.
    let needs_readiness_check = {
        let store_ref = store.lock().await;
        store_ref.get(&id).is_some_and(|job| {
            job.state == JobState::Running
                && matches!(
                    job.readiness,
                    Some(bgrun_proto::ReadinessStrategy::LogPattern(_))
                )
                && exit_code == Some(0)
        })
    };

    if needs_readiness_check {
        let pattern = {
            let store_ref = store.lock().await;
            store_ref.get(&id).and_then(|job| match &job.readiness {
                Some(bgrun_proto::ReadinessStrategy::LogPattern(p)) => Some(p.clone()),
                _ => None,
            })
        };
        if let Some(pattern) = pattern {
            let log_path = state::job_dir(&id).join("stdout.log");
            // Poll the log for the pattern with a 3s timeout, giving the capture
            // task time to flush any buffered output after the child exited.
            let pattern_matched = poll_log_for_pattern(&log_path, &pattern, 3000).await;
            if pattern_matched {
                let mut store = store.lock().await;
                if let Some(job) = store.get_mut(&id) {
                    if job.state == JobState::Running {
                        let _ = job.transition(JobState::Ready);
                        job.ready_at = Some(chrono::Utc::now());
                        info!(id = %id, "post-exit readiness check: pattern matched, transitioning through Ready");
                    }
                }
            }
        }
    }

    // Transition to terminal state
    let new_state = match exit_code {
        Some(0) => JobState::Exited,
        _ => JobState::Crashed,
    };
    {
        let mut store = store.lock().await;
        if let Some(job) = store.get_mut(&id) {
            job.exit_code = exit_code;
            if job.state != JobState::Killed {
                let _ = job.transition(new_state);
            }
        }
    }

    // Clean up stdin/PTY handles
    STDIN_HANDLES.lock().await.remove(&id);
    PTY_WRITERS.lock().await.remove(&id);

    // Persist status
    let job = {
        let store = store.lock().await;
        store.get(&id).cloned()
    };
    if let Some(job) = job {
        let _ = state::write_status(&job).await;
        info!(id = %id, state = %job.state.to_string(), "job exited");
    }
}

/// Monitors a piped child process and calls handle_job_exit on exit.
async fn monitor_job(id: String, store: Arc<Mutex<JobStore>>, mut child: Child) {
    let exit_status = child.wait().await;
    let exit_code = match &exit_status {
        Ok(status) => status.code().or(Some(-1)),
        Err(_) => Some(-1),
    };
    handle_job_exit(id, store, exit_code).await;
}

/// Monitors a PTY child process by polling try_wait and calls handle_job_exit on exit.
async fn monitor_pty_job(
    id: String,
    store: Arc<Mutex<JobStore>>,
    mut child: Box<dyn portable_pty::Child + Send>,
) {
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        match child.try_wait() {
            Ok(Some(status)) => {
                let exit_code = Some(status.exit_code() as i32);
                handle_job_exit(id, store, exit_code).await;
                return;
            }
            Ok(None) => continue,
            Err(e) => {
                error!(id = %id, "error polling PTY child: {}", e);
                handle_job_exit(id, store, Some(-1)).await;
                return;
            }
        }
    }
}

/// Polls the log file for a pattern with a timeout, returning true if found.
async fn poll_log_for_pattern(log_path: &std::path::Path, pattern: &str, timeout_ms: u64) -> bool {
    use tokio::io::AsyncReadExt;

    let start = tokio::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);

    loop {
        if start.elapsed() >= timeout {
            return false;
        }

        let mut file = match tokio::fs::OpenOptions::new().read(true).open(log_path).await {
            Ok(f) => f,
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
        };

        let mut content = String::new();
        if file.read_to_string(&mut content).await.is_err() {
            return false;
        }

        if content.contains(pattern) {
            return true;
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Spawns a job in a PTY (pseudo-terminal) using portable-pty.
///
/// Attaches the process to the PTY slave, reads from the PTY master for
/// output capture, and stores the PTY master writer for `send_stdin`.
async fn spawn_pty_job(
    args: RunArgs,
    store: Arc<Mutex<JobStore>>,
    id: String,
    job_dir: std::path::PathBuf,
    cmd: Vec<String>,
) -> Result<JobRecord> {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize::default())
        .context("failed to open PTY")?;

    let mut cmd_builder = CommandBuilder::new(&cmd[0]);
    if cmd.len() > 1 {
        cmd_builder.args(&cmd[1..]);
    }
    if let Some(ref cwd) = args.cwd {
        cmd_builder.cwd(cwd);
    }

    let child = pair
        .slave
        .spawn_command(cmd_builder)
        .context("failed to spawn process in PTY")?;
    let pid = child
        .process_id()
        .ok_or_else(|| anyhow::anyhow!("PTY child did not report a pid"))?;

    // Clone the PTY master reader for output capture (sync reads)
    let reader = pair
        .master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;

    // Take the PTY master writer for stdin injection (can only be taken once)
    let writer = pair
        .master
        .take_writer()
        .context("failed to take PTY writer")?;
    PTY_WRITERS
        .lock()
        .await
        .insert(id.clone(), Arc::new(std::sync::Mutex::new(writer)));

    // Spawn blocking task to capture PTY output (sync reads on PTY master)
    let log_path = job_dir.join("stdout.log");
    let id_clone = id.clone();
    tokio::task::spawn_blocking(move || {
        capture_pty_output(reader, &log_path, &id_clone);
    });

    // Create Job record
    let mut job = Job::new(id.clone(), cmd.clone(), args.name, args.workspace);
    job.pid = Some(pid);
    job.state = JobState::Running;
    job.readiness = args.readiness.clone();
    job.restart = args.restart.clone();
    job.pty = true;
    job.max_runtime_ms = args.max_runtime_ms;
    job.env = args.env.clone();

    state::write_meta(&job).await?;
    state::write_status(&job).await?;

    // Insert into store
    let record = job.to_record();
    {
        let mut store = store.lock().await;
        store.insert(job);
    }

    // Spawn PTY monitor task (polls try_wait every 100ms)
    let store_clone = store.clone();
    let id_clone = id.clone();
    tokio::spawn(async move {
        monitor_pty_job(id_clone, store_clone, child).await;
    });

    // Spawn readiness loop if configured
    if let Some(ref strategy) = args.readiness {
        let checker = bgrun_daemon::readiness::build_checker(strategy, &job_dir);
        let store_clone = store.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            bgrun_daemon::readiness::readiness_loop(id_clone, store_clone, checker, 60_000).await;
        });
    }

    // Spawn max runtime timeout if configured
    if let Some(max_ms) = args.max_runtime_ms {
        let store_clone = store.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(max_ms)).await;
            let alive = {
                let store_ref = store_clone.lock().await;
                store_ref.get(&id_clone).is_some_and(|j| j.is_alive())
            };
            if alive {
                info!(id = %id_clone, "max runtime reached, killing job");
                if let Err(e) = kill_job(&id_clone, store_clone).await {
                    error!(id = %id_clone, error = %e, "failed to kill job after max runtime");
                }
            }
        });
    }

    info!(pid, id = %id, cmd = %cmd.join(" "), "job spawned (pty)");
    Ok(record)
}

/// Kills a job by its ID. Sends SIGTERM to the process group, then SIGKILL after 5s.
pub async fn kill_job(id: &str, store: Arc<Mutex<JobStore>>) -> Result<()> {
    let (pid, is_alive) = {
        let store_ref = store.lock().await;
        let job = store_ref
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("job not found"))?;
        (job.pid, job.is_alive())
    };

    let pid = pid.ok_or_else(|| anyhow::anyhow!("job has no pid"))?;

    if !is_alive {
        anyhow::bail!("cannot kill job: it is already in a terminal state");
    }

    let pgid = Pid::from_raw(pid as i32);
    killpg(pgid, Signal::SIGTERM).context("failed to send SIGTERM")?;

    // Spawn task to SIGKILL after 5s if still alive
    let id_clone = id.to_string();
    let store_clone = store.clone();
    tokio::spawn(async move {
        sleep(Duration::from_secs(5)).await;
        let should_kill = {
            let store = store_clone.lock().await;
            store.get(&id_clone).is_some_and(|job| job.is_alive())
        };
        if !should_kill {
            return;
        }
        let pgid = Pid::from_raw(pid as i32);
        if let Err(e) = killpg(pgid, Signal::SIGKILL) {
            error!(id = %id_clone, error = %e, "failed to SIGKILL after timeout");
        }
    });

    // Update state
    let job = {
        let mut store = store.lock().await;
        if let Some(job) = store.get_mut(id) {
            let _ = job.transition(JobState::Killed);
            Some(job.clone())
        } else {
            None
        }
    };
    if let Some(job) = job {
        state::write_status(&job).await?;
    }

    info!(id = %id, pid = %pid, "job killed");
    Ok(())
}

/// Sends data to a job's stdin.
///
/// Accepts either a job ID (UUID) or a named job name.
/// Checks PTY writers first, then piped stdin handles.
pub async fn send_stdin(
    id_or_name: &str,
    data: &str,
    store: Arc<Mutex<JobStore>>,
) -> Result<()> {
    // Resolve ID or name to actual job ID
    let actual_id = resolve_job_id(id_or_name, store.clone()).await?;

    // Try PTY writers first
    {
        let mut pty_writers = PTY_WRITERS.lock().await;
        if let Some(writer_arc) = pty_writers.get_mut(&actual_id) {
            let mut writer = writer_arc.lock().unwrap();
            use std::io::Write;
            writer
                .write_all(data.as_bytes())
                .map_err(|e| anyhow::anyhow!("stdin write to PTY failed: {}", e))?;
            return Ok(());
        }
    }

    // Fall back to piped stdin
    let mut handles = STDIN_HANDLES.lock().await;
    let stdin = handles
        .get_mut(&actual_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no stdin handle for job '{}' (process may have exited or was started with --pty)",
                id_or_name
            )
        })?;

    use tokio::io::AsyncWriteExt;
    stdin.write_all(data.as_bytes()).await?;
    Ok(())
}

/// Resolves a job ID or name to the actual job ID. Checks by ID first, then
/// by name lookup in the store.
async fn resolve_job_id(
    id_or_name: &str,
    store: Arc<Mutex<JobStore>>,
) -> Result<String> {
    // Check by ID in both handle maps
    {
        let piped = STDIN_HANDLES.lock().await;
        let pty = PTY_WRITERS.lock().await;
        if piped.contains_key(id_or_name) || pty.contains_key(id_or_name) {
            return Ok(id_or_name.to_string());
        }
    }

    // Resolve name to ID from the store
    let store_ref = store.lock().await;
    let resolved = store_ref
        .find_by_name(id_or_name)
        .ok_or_else(|| anyhow::anyhow!("job '{}' not found", id_or_name))?;
    let id = resolved.id.clone();
    drop(store_ref);

    // Verify the resolved ID has a handle
    {
        let piped = STDIN_HANDLES.lock().await;
        let pty = PTY_WRITERS.lock().await;
        if piped.contains_key(&id) || pty.contains_key(&id) {
            return Ok(id);
        }
    }

    anyhow::bail!(
        "stdin handle for job '{}' is no longer available (process may have exited)",
        id_or_name
    )
}

/// Returns resource stats for a running process using the shared sysinfo instance.
pub async fn get_stats(
    id: &str,
    store: Arc<Mutex<JobStore>>,
    sys: Arc<Mutex<sysinfo::System>>,
) -> Result<bgrun_proto::ResourceStats> {
    let pid = {
        let store_ref = store.lock().await;
        store_ref
            .get(id)
            .and_then(|j| j.pid)
            .ok_or_else(|| anyhow::anyhow!("job not found or has no pid"))?
    };

    let mut sys = sys.lock().await;
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All);

    let sysinfo_pid = sysinfo::Pid::from_u32(pid);
    let (cpu_pct, rss_mb, uptime_secs) = match sys.process(sysinfo_pid) {
        Some(proc) => (proc.cpu_usage(), proc.memory() / (1024 * 1024), proc.run_time()),
        None => (0.0, 0, 0),
    };

    Ok(bgrun_proto::ResourceStats {
        cpu_pct,
        rss_mb,
        uptime_secs,
    })
}

/// Reads from a child's stdout/stderr pipe and appends to the log file,
/// rotating when the file exceeds 50MB.
/// Each complete line is prefixed with an ISO 8601 timestamp.
async fn capture_output(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    log_path: std::path::PathBuf,
    id: &str,
    label: &str,
) {
    use tokio::io::AsyncReadExt;

    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            error!(id = %id, label = %label, error = %e, "failed to open log file");
            return;
        }
    };

    let mut buf = [0u8; 8192];
    let mut partial = Vec::new();
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => {
                // Flush remaining partial line
                if !partial.is_empty() {
                    write_ts_line(&mut file, &partial, id, label).await;
                }
                break;
            }
            Ok(n) => {
                let mut start = 0;
                for i in 0..n {
                    if buf[i] == b'\n' {
                        partial.extend_from_slice(&buf[start..=i]);
                        write_ts_line(&mut file, &partial, id, label).await;
                        partial.clear();
                        start = i + 1;
                    }
                }
                if start < n {
                    partial.extend_from_slice(&buf[start..n]);
                }
                // Check for rotation (only once per chunk)
                if let Ok(meta) = tokio::fs::metadata(&log_path).await {
                    if meta.len() > 50 * 1024 * 1024 {
                        let _ = bgrun_daemon::log_manager::rotate_if_needed(id).await;
                        if let Ok(new_file) = tokio::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&log_path)
                            .await
                        {
                            file = new_file;
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }
}

/// Writes a single line (with trailing \n) to the log file, prepending an
/// ISO 8601 timestamp with millisecond precision.
async fn write_ts_line(
    file: &mut tokio::fs::File,
    line: &[u8],
    id: &str,
    label: &str,
) {
    use tokio::io::AsyncWriteExt;

    let ts = chrono::Utc::now().format("[%Y-%m-%dT%H:%M:%S%.3fZ] ");
    let ts_bytes = ts.to_string().into_bytes();

    if file.write_all(&ts_bytes).await.is_err() {
        error!(id = %id, label = %label, "failed to write timestamp to log");
        return;
    }
    if file.write_all(line).await.is_err() {
        error!(id = %id, label = %label, "failed to write line to log");
    }
}

/// Synchronous version of write_ts_line for PTY capture (called from spawn_blocking).
fn write_ts_line_sync(file: &mut std::fs::File, line: &[u8], id: &str) {
    use std::io::Write;

    let ts = chrono::Utc::now().format("[%Y-%m-%dT%H:%M:%S%.3fZ] ");
    let ts_bytes = ts.to_string().into_bytes();

    if file.write_all(&ts_bytes).is_err() {
        error!(id = %id, "failed to write timestamp to PTY log");
        return;
    }
    if file.write_all(line).is_err() {
        error!(id = %id, "failed to write line to PTY log");
    }
}

/// Captures output from a PTY master reader (sync, called from spawn_blocking).
/// Like capture_output but uses std::io::Read/Write for synchronous PTY reads.
fn capture_pty_output(
    mut reader: Box<dyn std::io::Read + Send>,
    log_path: &std::path::Path,
    id: &str,
) {
    use std::io::Read;

    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        Ok(f) => f,
        Err(e) => {
            error!(id = %id, "failed to open log file for PTY output: {}", e);
            return;
        }
    };

    let mut buf = [0u8; 8192];
    let mut partial = Vec::new();
    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                if !partial.is_empty() {
                    write_ts_line_sync(&mut file, &partial, id);
                }
                break;
            }
            Ok(n) => {
                let mut start = 0;
                for i in 0..n {
                    if buf[i] == b'\n' {
                        partial.extend_from_slice(&buf[start..=i]);
                        write_ts_line_sync(&mut file, &partial, id);
                        partial.clear();
                        start = i + 1;
                    }
                }
                if start < n {
                    partial.extend_from_slice(&buf[start..n]);
                }
            }
            Err(e) => {
                error!(id = %id, "error reading PTY output: {}", e);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_check_log_for_pattern_matches() {
        let dir = std::env::temp_dir().join("bgrun-test-check-pattern");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let log_path = dir.join("stdout.log");
        tokio::fs::write(
            &log_path,
            "[2026-06-01T10:00:00.000Z] listening on :8080\n",
        )
        .await
        .unwrap();

        assert!(poll_log_for_pattern(&log_path, "listening on", 100).await);
        assert!(!poll_log_for_pattern(&log_path, "nonexistent", 100).await);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_check_log_for_pattern_missing_file() {
        let log_path = std::env::temp_dir().join("bgrun-test-nonexistent");
        assert!(!poll_log_for_pattern(&log_path, "anything", 100).await);
    }

    #[tokio::test]
    async fn test_check_log_for_pattern_empty_log() {
        let dir = std::env::temp_dir().join("bgrun-test-empty");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let log_path = dir.join("stdout.log");
        tokio::fs::write(&log_path, "").await.unwrap();

        assert!(!poll_log_for_pattern(&log_path, "anything", 100).await);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
