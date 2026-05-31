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

/// Global map of job IDs to their stdin handles.
pub static STDIN_HANDLES: once_cell::sync::Lazy<
    Mutex<HashMap<String, tokio::process::ChildStdin>>,
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

    // Spawn the child process with piped stdout/stderr (enables log rotation)
    let mut child_cmd = Command::new(&cmd[0]);
    if cmd.len() > 1 {
        child_cmd.args(&cmd[1..]);
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

/// Monitors a child process and updates job state on exit.
async fn monitor_job(id: String, store: Arc<Mutex<JobStore>>, mut child: Child) {
    let exit_status = child.wait().await;
    let exit_code = match &exit_status {
        Ok(status) => status.code().or(Some(-1)),
        Err(_) => Some(-1),
    };

    let new_state = match exit_code {
        Some(0) => JobState::Exited,
        Some(_) => JobState::Crashed,
        None => JobState::Crashed,
    };

    {
        let mut store = store.lock().await;
        if let Some(job) = store.get_mut(&id) {
            job.exit_code = exit_code;
            if job.state != JobState::Killed {
                let _ = job.transition(new_state.clone());
            }
        }
    }

    // Clean up stdin handle to prevent memory leak
    STDIN_HANDLES.lock().await.remove(&id);

    let job = {
        let store = store.lock().await;
        store.get(&id).cloned()
    };
    if let Some(job) = job {
        let _ = state::write_status(&job).await;
        info!(id = %id, state = %job.state.to_string(), "job exited");
    }
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
pub async fn send_stdin(
    id_or_name: &str,
    data: &str,
    store: Arc<Mutex<JobStore>>,
) -> Result<()> {
    // Try direct lookup by ID first, then by name
    let actual_id = {
        let handles = STDIN_HANDLES.lock().await;
        if handles.contains_key(id_or_name) {
            id_or_name.to_string()
        } else {
            // Resolve name to ID from the store
            let store_ref = store.lock().await;
            let resolved = store_ref
                .find_by_name(id_or_name)
                .ok_or_else(|| anyhow::anyhow!("job '{}' not found and no stdin handle exists", id_or_name))?;
            let id = resolved.id.clone();
            drop(store_ref);
            if handles.contains_key(&id) {
                id
            } else {
                // The job might have exited and its stdin handle was cleaned up
                anyhow::bail!("stdin handle for job '{}' is no longer available (process may have exited)", id_or_name);
            }
        }
    };

    let mut handles = STDIN_HANDLES.lock().await;
    let stdin = handles
        .get_mut(&actual_id)
        .ok_or_else(|| anyhow::anyhow!("no stdin handle for job {}", id_or_name))?;

    use tokio::io::AsyncWriteExt;
    stdin.write_all(data.as_bytes()).await?;
    Ok(())
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
        Some(proc) => (proc.cpu_usage(), proc.memory() / 1024, proc.run_time()),
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
async fn capture_output(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    log_path: std::path::PathBuf,
    id: &str,
    label: &str,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if file.write_all(&buf[..n]).await.is_err() {
                    break;
                }
                // Check for rotation
                if let Ok(meta) = tokio::fs::metadata(&log_path).await {
                    if meta.len() > 50 * 1024 * 1024 {
                        let _ = bgrun_daemon::log_manager::rotate_if_needed(id).await;
                        // Reopen the new log file
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

#[cfg(test)]
mod tests {
    #[test]
    fn runner_module_has_tests() {
        assert_eq!(2 + 2, 4);
    }
}
