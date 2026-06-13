use std::sync::Arc;

use anyhow::Result;
use bgrun_core::JobStore;
use bgrun_proto::JobState;
use tokio::sync::Mutex;
use tracing::info;

use crate::state::LIFECYCLE_NOTIFY;
use crate::state;

/// Re-adopts previously running jobs from disk.
///
/// For each persisted job in Running/Ready state:
/// - If the PID is still alive: insert into store, schedule monitoring
/// - If dead: mark as Crashed
pub async fn readopt_all(store: Arc<Mutex<JobStore>>) -> Result<()> {
    let jobs = state::read_all_jobs().await?;
    let mut adopted = Vec::new();
    let mut crashed = 0;

    for job in jobs {
        if !matches!(job.state, JobState::Running | JobState::Ready) {
            continue;
        }

        let pid = match job.pid {
            Some(p) => p,
            None => continue,
        };

        let alive = is_process_alive(pid);

        if alive {
            info!(
                id = %job.id,
                pid = pid,
                "re-adopting live job"
            );
            let mut store_ref = store.lock().await;
            store_ref.insert(job);
            adopted.push((id_from_store(&store_ref, pid), pid));
        } else {
            info!(
                id = %job.id,
                pid = pid,
                "job dead on restart, marking crashed"
            );
            let mut dead_job = job;
            dead_job.state = JobState::Crashed;
            let _ = state::write_status(&dead_job).await;
            crashed += 1;
        }
    }

    let count = adopted.len();
    if count > 0 || crashed > 0 {
        info!(adopted = count, crashed, "orphan re-adoption complete");
    }

    // Spawn background monitor for re-adopted jobs
    for (id, pid) in adopted {
        let store_clone = store.clone();
        tokio::spawn(async move {
            poll_adopted_job(id, pid, store_clone).await;
        });
    }

    Ok(())
}

/// Polls a re-adopted job every 2 seconds, transitioning to Crashed if it dies.
async fn poll_adopted_job(id: String, pid: u32, store: Arc<Mutex<JobStore>>) {
    info!(id = %id, pid = pid, "started orphan monitor");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        if !is_process_alive(pid) {
            info!(id = %id, pid = pid, "re-adopted process died");
            let mut store_ref = store.lock().await;
            if let Some(job) = store_ref.get_mut(&id) {
                if job.is_alive() {
                    job.exit_code = Some(-1);
                    let _ = job.transition(JobState::Crashed);
                    let _ = state::write_status(job).await;

                    // Notify the reactive shutdown loop to check active task count
                    LIFECYCLE_NOTIFY.notify_one();
                }
            }
            return;
        }
    }
}

/// Checks if a process is alive using kill(pid, 0).
fn is_process_alive(pid: u32) -> bool {
    // SAFETY: kill(2) with signal 0 (null signal) only checks
    // whether the process exists and we have permission to signal it;
    // no actual signal is sent. The pid originates from the kernel's
    // own PID assignment and is passed by value — no pointer or
    // memory-safety concern exists.
    #[allow(unsafe_code)]
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Finds the job ID for a given PID in the store.
fn id_from_store(store: &JobStore, pid: u32) -> String {
    store
        .list_workspace(None)
        .into_iter()
        .find(|j| j.pid == Some(pid))
        .map(|j| j.id.clone())
        .unwrap_or_default()
}
