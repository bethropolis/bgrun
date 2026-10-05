pub use bgrun_proto::paths::{job_dir, socket_path, state_dir};

use anyhow::{Context, Result};
use bgrun_core::Job;
use bgrun_proto::{JobRecord, JobStatus};
use chrono::{DateTime, Utc};
use std::os::unix::fs::PermissionsExt;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Notify trigger for the reactive auto-shutdown monitor.
/// Called whenever a job spawns or exits.
pub static LIFECYCLE_NOTIFY: LazyLock<tokio::sync::Notify> =
    LazyLock::new(tokio::sync::Notify::new);

/// Distinguishes concurrent temp files for the same target path.
static TMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Atomically writes `data` to `path` with owner-only permissions.
///
/// Writes to a temp sibling then renames, so a crash mid-write can never
/// leave a truncated/corrupt state file behind.
///
/// The temp name carries the daemon pid and a per-call sequence number.
/// `status.json` is written from several tasks for the same job (readiness
/// loop, health monitor, exit/stop/restart handlers), and a single shared
/// temp path made concurrent writers clobber each other's file, so one
/// writer's rename failed with `NotFound` and could persist stale data.
async fn atomic_write(path: &std::path::Path, data: &str) -> Result<()> {
    let seq = TMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp.{}.{seq}", std::process::id()));
    tokio::fs::write(&tmp, data)
        .await
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    let _ = tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).await;
    if let Err(error) = tokio::fs::rename(&tmp, path).await {
        // Never leave the temp file behind for a failed write.
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(error).with_context(|| format!("failed to persist {}", path.display()));
    }
    Ok(())
}

/// Writes durable metadata for a job.
pub async fn write_meta(job: &Job) -> Result<()> {
    let dir = job_dir(&job.id);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create job directory {}", dir.display()))?;
    // Job metadata includes process env (possibly secrets): owner-only.
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    let json = serde_json::to_string_pretty(&job.to_record())?;
    atomic_write(&dir.join("meta.json"), &json).await
}

/// Writes durable status for a job.
pub async fn write_status(job: &Job) -> Result<()> {
    let dir = job_dir(&job.id);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create job directory {}", dir.display()))?;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    let status = JobStatus {
        state: job.state.clone(),
        exit_code: job.exit_code,
        ready_at: job.ready_at.map(|t| t.to_rfc3339()),
        restart_count: job.restart_count,
        last_diff_cursor: job.last_diff_cursor,
        last_diff_lines: job.last_diff_lines,
        consecutive_failures: job.consecutive_failures,
    };
    let json = serde_json::to_string_pretty(&status)?;
    atomic_write(&dir.join("status.json"), &json).await
}

/// Reads all persisted jobs from disk.
pub async fn read_all_jobs() -> Result<Vec<Job>> {
    let jobs_dir = state_dir().join("jobs");
    let mut jobs = Vec::new();

    let mut entries = match tokio::fs::read_dir(&jobs_dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(jobs),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", jobs_dir.display()));
        }
    };

    while let Some(entry) = entries.next_entry().await? {
        let dir = entry.path();
        let meta_path = dir.join("meta.json");
        let status_path = dir.join("status.json");

        let meta_json = match tokio::fs::read_to_string(&meta_path).await {
            Ok(m) => m,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!("skipping job dir {}: meta.json not found", dir.display());
                continue;
            }
            Err(err) => {
                return Err(err).with_context(|| format!("failed to read {}", meta_path.display()));
            }
        };
        let record: JobRecord = match serde_json::from_str(&meta_json) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    "skipping job dir {}: failed to parse meta.json: {}",
                    dir.display(),
                    e
                );
                continue;
            }
        };

        let status = match tokio::fs::read_to_string(&status_path).await {
            Ok(status_json) => match serde_json::from_str::<JobStatus>(&status_json) {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!(
                        "skipping status for {}: failed to parse status.json: {}",
                        dir.display(),
                        e
                    );
                    None
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to read {}", status_path.display()));
            }
        };

        let started_at = match DateTime::parse_from_rfc3339(&record.started_at) {
            Ok(t) => t.with_timezone(&Utc),
            Err(e) => {
                tracing::warn!(
                    "skipping job {}: failed to parse started_at: {}",
                    record.id,
                    e
                );
                continue;
            }
        };
        let ready_at = status
            .as_ref()
            .and_then(|s| s.ready_at.as_deref())
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .with_context(|| format!("failed to parse ready_at for job {}", record.id))?
            .map(|t| t.with_timezone(&Utc));

        let mut job = Job::new(record.id, record.cmd, record.name, record.workspace);
        job.pid = record.pid;
        job.state = status
            .as_ref()
            .map_or(record.state, |status| status.state.clone());
        job.started_at = started_at;
        job.ready_at = ready_at;
        job.exit_code = status.as_ref().and_then(|status| status.exit_code);
        job.restart_count = status.as_ref().map_or(0, |status| status.restart_count);
        job.last_diff_cursor = status.as_ref().map_or(0, |status| status.last_diff_cursor);
        job.last_diff_lines = status.as_ref().map_or(0, |status| status.last_diff_lines);
        job.consecutive_failures = status
            .as_ref()
            .map_or(0, |status| status.consecutive_failures);
        job.readiness = record.readiness;
        job.restart = record.restart;
        job.max_retries = record.max_retries;
        job.log_max_size = record.log_max_size;
        job.log_keep = record.log_keep;
        job.memory_max = record.memory_max;
        job.cpu_quota = record.cpu_quota;
        job.pids_max = record.pids_max;
        job.sandbox = record.sandbox;
        job.pty = record.pty;
        job.max_runtime_ms = record.max_runtime_ms;
        job.max_rss_mb = record.max_rss_mb;
        job.env = record.env;
        job.cwd = record.cwd;
        job.allocated_port = record.allocated_port;
        job.health_check = record.health_check;
        job.health_interval_secs = record.health_interval_secs;
        job.health_threshold = record.health_threshold;
        jobs.push(job);
    }

    // Dedup: for named jobs, keep only the latest entry by started_at.
    // This cleans up stale duplicates left by earlier restart logic.
    let mut best: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut stale_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for job in &jobs {
        if let Some(ref name) = job.name {
            match best.get(name) {
                Some(prev) => {
                    let prev = prev.clone();
                    if job.started_at
                        > jobs
                            .iter()
                            .find(|j| j.id == *prev)
                            .map_or(job.started_at, |j| j.started_at)
                    {
                        stale_ids.insert(prev.clone());
                        best.insert(name.clone(), job.id.clone());
                    } else {
                        stale_ids.insert(job.id.clone());
                    }
                }
                None => {
                    best.insert(name.clone(), job.id.clone());
                }
            }
        }
    }
    if !stale_ids.is_empty() {
        for id in &stale_ids {
            let _ = tokio::fs::remove_dir_all(job_dir(id)).await;
        }
        jobs.retain(|j| !stale_ids.contains(&j.id));
    }

    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_dir_includes_job_id() {
        assert!(job_dir("abc123").ends_with("jobs/abc123"));
    }

    #[test]
    fn socket_path_ends_with_daemon_socket() {
        assert!(socket_path().ends_with("daemon.sock"));
    }

    /// Regression: concurrent writers used to share one temp path, so all but
    /// the last rename failed with `NotFound` and the loser reported an error.
    #[tokio::test]
    async fn atomic_write_survives_concurrent_writers() {
        let dir = std::path::Path::new("/tmp").join(format!("bgrun-atomic-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let target = dir.join("status.json");

        let mut handles = Vec::new();
        for i in 0..16u32 {
            let target = target.clone();
            handles.push(tokio::spawn(async move {
                atomic_write(&target, &format!("{{\"writer\":{i}}}")).await
            }));
        }
        for handle in handles {
            handle
                .await
                .expect("task should not panic")
                .expect("concurrent atomic_write must not fail");
        }

        // Exactly one writer's payload survives, and it is intact JSON.
        let final_text = tokio::fs::read_to_string(&target).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&final_text).unwrap();
        assert!(parsed["writer"].is_u64());

        // No temp files left behind.
        let mut entries = tokio::fs::read_dir(&dir).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        assert_eq!(names, vec!["status.json".to_string()]);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
