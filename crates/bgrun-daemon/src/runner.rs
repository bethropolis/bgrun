use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bgrun_core::{Job, JobStore};
use bgrun_proto::{JobRecord, JobState, RestartPolicy, RunArgs};
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{error, info};

use crate::state;

/// Shared sysinfo::System instance for resource monitoring.
static SYSINFO_SYSTEM: once_cell::sync::Lazy<Arc<std::sync::Mutex<sysinfo::System>>> =
    once_cell::sync::Lazy::new(|| Arc::new(std::sync::Mutex::new(sysinfo::System::new())));

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

/// Global map of job IDs to their PTY master handles (for resize operations).
pub static PTY_PAIRS: once_cell::sync::Lazy<
    Mutex<HashMap<String, Box<dyn portable_pty::MasterPty + Send>>>,
> = once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

/// Broadcast senders for raw PTY output to attached clients.
pub static JOB_BROADCASTS: once_cell::sync::Lazy<
    Mutex<HashMap<String, tokio::sync::broadcast::Sender<Vec<u8>>>>,
> = once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

// Re-export for convenience — the static is defined in state.rs so the
// library crate (orphan.rs) and binary crate (runner.rs) share one instance.
pub use crate::state::LIFECYCLE_NOTIFY;

/// Per-job mutex that serializes concurrent stdout/stderr writes to the
/// shared log file. Prevents interleaved NDJSON entries.
pub static LOG_WRITE_LOCKS: once_cell::sync::Lazy<
    Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
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
    if !args.env.is_empty() {
        child_cmd.envs(&args.env);
    }

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
    job.max_rss_mb = args.max_rss_mb;
    job.env = args.env.clone();
    job.cwd = args.cwd.clone();

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

    // Spawn memory limit monitor if configured
    if let Some(max_rss_mb) = args.max_rss_mb {
        let store_clone = store.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            monitor_memory_limit(id_clone, max_rss_mb, store_clone).await;
        });
    }

    info!(pid, id = %id, cmd = %cmd.join(" "), "job spawned");
    LIFECYCLE_NOTIFY.notify_one();
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
            // Reset consecutive_failures on clean exit
            if exit_code == Some(0) {
                job.consecutive_failures = 0;
            }
            if job.state != JobState::Killed {
                let _ = job.transition(new_state);
            }
        }
    }

    // Clean up stdin/PTY handles and global states
    STDIN_HANDLES.lock().await.remove(&id);
    PTY_WRITERS.lock().await.remove(&id);
    PTY_PAIRS.lock().await.remove(&id);
    JOB_BROADCASTS.lock().await.remove(&id);
    LOG_WRITE_LOCKS.lock().await.remove(&id);

    // Persist status
    let job = {
        let store = store.lock().await;
        store.get(&id).cloned()
    };
    if let Some(ref job) = job {
        let _ = state::write_status(job).await;
        info!(id = %id, state = %job.state.to_string(), "job exited");
    }

    // Exponential backoff restart for crashed jobs with OnCrash policy
    if let Some(ref job) = job {
        if job.state == JobState::Crashed {
            let restart_policy = job.restart.clone();
            if let Some(RestartPolicy::OnCrash { backoff_ms }) = restart_policy {
                let backoff_ms = backoff_ms.max(1_000); // floor at 1s
                let consecutive = job.consecutive_failures;
                let running_10s = (chrono::Utc::now() - job.started_at)
                    > chrono::Duration::seconds(10);

                // Reset counter if the job ran for more than 10s
                if running_10s {
                    {
                        let mut store = store.lock().await;
                        if let Some(j) = store.get_mut(&id) {
                            j.consecutive_failures = 0;
                        }
                    }
                    // Only actually restart with base backoff after a longer run
                    let store_clone = store.clone();
                    let job_id = id.clone();
                    tokio::task::spawn_blocking(move || {
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(async {
                            sleep(Duration::from_millis(backoff_ms)).await;
                            restart_job(job_id, store_clone).await;
                        });
                    });
                } else {
                    // Exponential backoff: base * 2^consecutive + jitter
                    let exponent = 1u64 << consecutive.min(8); // cap exponent at 256x
                    let delay_ms = backoff_ms.saturating_mul(exponent);
                    let delay_ms = delay_ms.min(300_000); // cap at 5 min
                    // Simple jitter: 0–1000ms based on nanosecond timestamp bits
                    let jitter_ms = (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .subsec_nanos()
                        % 1000) as u64;
                    let total_ms = delay_ms + jitter_ms;

                    // Increment consecutive_failures
                    {
                        let mut store = store.lock().await;
                        if let Some(j) = store.get_mut(&id) {
                            j.consecutive_failures = consecutive + 1;
                        }
                    }

                    info!(
                        id = %id,
                        backoff_ms = %total_ms,
                        consecutive = %consecutive,
                        "scheduled restart after crash"
                    );

                    let store_clone = store.clone();
                    let job_id = id.clone();
                    tokio::task::spawn_blocking(move || {
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(async {
                            sleep(Duration::from_millis(total_ms)).await;
                            restart_job(job_id, store_clone).await;
                        });
                    });
                }
            }
        }
    }

    LIFECYCLE_NOTIFY.notify_one();
}

/// Re-spawns a job that previously crashed, using the stored Job data
/// to reconstruct RunArgs. Removes the old crashed record first so
/// named jobs never accumulate stale entries.
async fn restart_job(id: String, store: Arc<Mutex<JobStore>>) {
    let job = {
        let store_ref = store.lock().await;
        store_ref.get(&id).cloned()
    };
    let Some(job) = job else {
        error!(id = %id, "restart: job not found");
        return;
    };

    // Don't re-spawn if the job was killed or manually put into a terminal state
    if !matches!(job.state, JobState::Crashed) {
        return;
    }

    // If a job with the same name is already alive, skip restart entirely.
    // This prevents `bgrun kill <name>` from being defeated by a pending
    // restart scheduled for an older UUID.
    if let Some(ref name) = job.name {
        let store_ref = store.lock().await;
        if let Some(existing) = store_ref.find_by_name(name) {
            if existing.is_alive() {
                info!(name = %name, "restart skipped: job already alive");
                return;
            }
        }
    }

    let args = RunArgs {
        cmd: job.cmd.clone(),
        name: job.name.clone(),
        workspace: job.workspace.clone(),
        readiness: job.readiness.clone(),
        restart: job.restart.clone(),
        pty: job.pty,
        max_runtime_ms: job.max_runtime_ms,
        env: job.env.clone(),
        after: None, // skip dependency check on restart
        cwd: job.cwd.clone(),
        pty_cols: None, // use defaults
        pty_rows: None,
        max_rss_mb: job.max_rss_mb,
    };

    // Remove the old crashed record from both store and disk so named
    // jobs never accumulate stale entries.
    store.lock().await.remove(&id);
    let _ = tokio::fs::remove_dir_all(state::job_dir(&id)).await;

    info!(id = %id, "restarting crashed job");
    match spawn_job(args, store).await {
        Ok(_) => info!(id = %id, "restart succeeded"),
        Err(e) => error!(id = %id, error = %e, "restart failed"),
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

/// Monitors a PTY child process using zero-overhead blocking OS wait.
async fn monitor_pty_job(
    id: String,
    store: Arc<Mutex<JobStore>>,
    mut child: Box<dyn portable_pty::Child + Send>,
) {
    let exit_code = tokio::task::spawn_blocking(move || {
        match child.wait() {
            Ok(status) => Some(status.exit_code() as i32),
            Err(_) => Some(-1),
        }
    })
    .await
    .unwrap_or(Some(-1));

    handle_job_exit(id, store, exit_code).await;
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
    let cols = args.pty_cols.unwrap_or(80);
    let rows = args.pty_rows.unwrap_or(24);
    let pair = pty_system
        .openpty(PtySize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("failed to open PTY")?;

    // Destructure the PtyPair to get master and slave
    let master = pair.master;
    let slave = pair.slave;

    let mut cmd_builder = CommandBuilder::new(&cmd[0]);
    if cmd.len() > 1 {
        cmd_builder.args(&cmd[1..]);
    }
    if let Some(ref cwd) = args.cwd {
        cmd_builder.cwd(cwd);
    }
    for (k, v) in &args.env {
        cmd_builder.env(k, v);
    }

    let child = slave
        .spawn_command(cmd_builder)
        .context("failed to spawn process in PTY")?;
    let pid = child
        .process_id()
        .ok_or_else(|| anyhow::anyhow!("PTY child did not report a pid"))?;

    // Clone the PTY master reader for output capture (sync reads)
    let reader = master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;

    // Take the PTY master writer for stdin injection (can only be taken once)
    let writer = master
        .take_writer()
        .context("failed to take PTY writer")?;
    PTY_WRITERS
        .lock()
        .await
        .insert(id.clone(), Arc::new(std::sync::Mutex::new(writer)));

    // Store the master handle for resize operations
    PTY_PAIRS
        .lock()
        .await
        .insert(id.clone(), master);

    // Create broadcast channel for PTY output to attached clients
    let (tx, _rx) = tokio::sync::broadcast::channel(1024);
    JOB_BROADCASTS
        .lock()
        .await
        .insert(id.clone(), tx.clone());

    // Spawn blocking task to capture PTY output (sync reads on PTY master)
    // Broadcast sender is passed through so raw bytes reach attached clients
    let log_path = job_dir.join("stdout.log");
    let id_clone = id.clone();
    tokio::task::spawn_blocking(move || {
        capture_pty_output(reader, &log_path, &id_clone, tx);
    });

    // Create Job record
    let mut job = Job::new(id.clone(), cmd.clone(), args.name, args.workspace);
    job.pid = Some(pid);
    job.state = JobState::Running;
    job.readiness = args.readiness.clone();
    job.restart = args.restart.clone();
    job.pty = true;
    job.max_runtime_ms = args.max_runtime_ms;
    job.max_rss_mb = args.max_rss_mb;
    job.env = args.env.clone();
    job.cwd = args.cwd.clone();

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

    // Spawn memory limit monitor if configured
    if let Some(max_rss_mb) = args.max_rss_mb {
        let store_clone = store.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            monitor_memory_limit(id_clone, max_rss_mb, store_clone).await;
        });
    }

    info!(pid, id = %id, cmd = %cmd.join(" "), "job spawned (pty)");
    LIFECYCLE_NOTIFY.notify_one();
    Ok(record)
}

/// Kills a job by its ID. Sends SIGTERM to the process group, then SIGKILL after 5s.
pub async fn kill_job(id_or_name: &str, store: Arc<Mutex<JobStore>>) -> Result<()> {
    let (pid, is_alive, id) = {
        let store_ref = store.lock().await;
        let actual_id = store_ref
            .resolve_id(id_or_name)
            .ok_or_else(|| anyhow::anyhow!("job not found"))?;
        let job = store_ref
            .get(&actual_id)
            .ok_or_else(|| anyhow::anyhow!("job not found"))?;
        (job.pid, job.is_alive(), actual_id)
    };

    let pid = pid.ok_or_else(|| anyhow::anyhow!("job has no pid"))?;

    if !is_alive {
        anyhow::bail!("cannot kill job: it is already in a terminal state");
    }

    let pgid = Pid::from_raw(pid as i32);
    killpg(pgid, Signal::SIGTERM).context("failed to send SIGTERM")?;

    // Spawn task to SIGKILL after 5s if still alive
    let id_clone = id.clone();
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
        if let Some(job) = store.get_mut(&id) {
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

/// Kills all alive jobs. Sends SIGTERM to each process group, then SIGKILL after 3s.
pub async fn kill_all_jobs(store: Arc<Mutex<JobStore>>) {
    let alive: Vec<(String, u32)> = {
        let store_ref = store.lock().await;
        store_ref
            .list_workspace(None)
            .into_iter()
            .filter(|j| j.is_alive())
            .filter_map(|j| j.pid.map(|p| (j.id.clone(), p)))
            .collect()
    };

    if alive.is_empty() {
        return;
    }

    info!(count = %alive.len(), "shutting down all jobs");

    // SIGTERM all process groups
    for (_, pid) in &alive {
        let pgid = Pid::from_raw(*pid as i32);
        let _ = killpg(pgid, Signal::SIGTERM);
    }

    // Wait 3s then SIGKILL survivors
    sleep(Duration::from_secs(3)).await;

    for (id, pid) in &alive {
        let still_alive = {
            let store_ref = store.lock().await;
            store_ref.get(id).is_some_and(|j| j.is_alive())
        };
        if still_alive {
            let pgid = Pid::from_raw(*pid as i32);
            if let Err(e) = killpg(pgid, Signal::SIGKILL) {
                error!(id = %id, error = %e, "failed to SIGKILL during shutdown");
            }
        }
    }
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
    let id = store_ref
        .resolve_id(id_or_name)
        .ok_or_else(|| anyhow::anyhow!("job '{}' not found", id_or_name))?;
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
    id_or_name: &str,
    store: Arc<Mutex<JobStore>>,
) -> Result<bgrun_proto::ResourceStats> {
    let pid = {
        let store_ref = store.lock().await;
        let actual_id = store_ref
            .resolve_id(id_or_name)
            .ok_or_else(|| anyhow::anyhow!("job not found"))?;
        store_ref
            .get(&actual_id)
            .and_then(|j| j.pid)
            .ok_or_else(|| anyhow::anyhow!("job has no pid"))?
    };

    let mut sys = SYSINFO_SYSTEM.lock().unwrap();
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

/// Returns the RSS in KB for a given PID, or None if the process is gone.
fn get_process_rss_kb(pid: u32) -> Option<u64> {
    let mut sys = SYSINFO_SYSTEM.lock().unwrap();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All);
    sys.process(sysinfo::Pid::from_u32(pid))
        .map(|p| p.memory())
}

/// Monitors a job's RSS every second and kills it if it exceeds max_rss_mb.
async fn monitor_memory_limit(
    id: String,
    max_rss_mb: u64,
    store: Arc<Mutex<JobStore>>,
) {
    loop {
        sleep(Duration::from_secs(1)).await;

        let (pid, is_alive) = {
            let store_ref = store.lock().await;
            match store_ref.get(&id) {
                Some(j) => (j.pid, j.is_alive()),
                None => (None, false),
            }
        };

        let Some(pid) = pid else { break };
        if !is_alive {
            break;
        }
        let rss_kb = get_process_rss_kb(pid);
        if let Some(rss_kb) = rss_kb {
            let rss_mb = rss_kb / 1024;
            if rss_mb > max_rss_mb {
                tracing::warn!(id = %id, rss_mb, max_rss_mb, "memory limit exceeded, killing job");
                let _ = kill_job(&id, store.clone()).await;
                break;
            }
        }
    }
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

/// Writes a single line to the log file as a structured NDJSON entry.
/// Serializes per-job to prevent interleaved stdout/stderr lines.
async fn write_ts_line(
    file: &mut tokio::fs::File,
    line: &[u8],
    id: &str,
    label: &str,
) {
    use tokio::io::AsyncWriteExt;

    let content = String::from_utf8_lossy(line)
        .trim_end_matches('\n')
        .to_string();
    let entry = bgrun_daemon::log_manager::DiskLogEntry {
        t: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        s: label.to_string(),
        c: content,
    };
    let json = serde_json::to_string(&entry).unwrap_or_default();

    // Acquire per-job lock to prevent interleaving between stdout/stderr tasks
    let lock = {
        let mut locks = LOG_WRITE_LOCKS.lock().await;
        locks.entry(id.to_string()).or_default().clone()
    };
    let _guard = lock.lock().await;

    if file.write_all(json.as_bytes()).await.is_err() {
        error!(id = %id, label = %label, "failed to write log entry");
        return;
    }
    if file.write_all(b"\n").await.is_err() {
        error!(id = %id, label = %label, "failed to write newline to log");
    }
}

/// Synchronous version of write_ts_line for PTY capture (called from spawn_blocking).
fn write_ts_line_sync(file: &mut std::fs::File, line: &[u8], id: &str) {
    use std::io::Write;

    let content = String::from_utf8_lossy(line)
        .trim_end_matches('\n')
        .to_string();
    let entry = bgrun_daemon::log_manager::DiskLogEntry {
        t: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        s: "pty".to_string(),
        c: content,
    };
    let json = serde_json::to_string(&entry).unwrap_or_default();

    if file.write_all(json.as_bytes()).is_err() {
        error!(id = %id, "failed to write PTY log entry");
        return;
    }
    if file.write_all(b"\n").is_err() {
        error!(id = %id, "failed to write newline to PTY log");
    }
}

/// Captures output from a PTY master reader (sync, called from spawn_blocking).
/// Like capture_output but uses std::io::Read/Write for synchronous PTY reads.
fn capture_pty_output(
    mut reader: Box<dyn std::io::Read + Send>,
    log_path: &std::path::Path,
    id: &str,
    broadcast_tx: tokio::sync::broadcast::Sender<Vec<u8>>,
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
                // Broadcast raw bytes to attached clients (ignore if none)
                let _ = broadcast_tx.send(buf[..n].to_vec());

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
