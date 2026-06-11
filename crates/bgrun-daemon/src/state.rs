pub use bgrun_proto::paths::{job_dir, socket_path, state_dir};

use anyhow::{Context, Result};
use bgrun_core::Job;
use bgrun_proto::{JobRecord, JobStatus};
use chrono::{DateTime, Utc};

/// Writes durable metadata for a job.
pub async fn write_meta(job: &Job) -> Result<()> {
    let dir = job_dir(&job.id);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create job directory {}", dir.display()))?;
    let json = serde_json::to_string_pretty(&job.to_record())?;
    tokio::fs::write(dir.join("meta.json"), json)
        .await
        .with_context(|| format!("failed to write metadata for job {}", job.id))
}

/// Writes durable status for a job.
pub async fn write_status(job: &Job) -> Result<()> {
    let dir = job_dir(&job.id);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create job directory {}", dir.display()))?;
    let status = JobStatus {
        state: job.state.clone(),
        exit_code: job.exit_code,
        ready_at: job.ready_at.map(|t| t.to_rfc3339()),
        restart_count: job.restart_count,
        last_diff_cursor: job.last_diff_cursor,
    };
    let json = serde_json::to_string_pretty(&status)?;
    tokio::fs::write(dir.join("status.json"), json)
        .await
        .with_context(|| format!("failed to write status for job {}", job.id))
}

/// Reads all persisted jobs from disk.
pub async fn read_all_jobs() -> Result<Vec<Job>> {
    let jobs_dir = state_dir().join("jobs");
    let mut jobs = Vec::new();

    let mut entries = match tokio::fs::read_dir(&jobs_dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(jobs),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", jobs_dir.display()))
        }
    };

    while let Some(entry) = entries.next_entry().await? {
        let dir = entry.path();
        let meta_path = dir.join("meta.json");
        let status_path = dir.join("status.json");

        let meta_json = match tokio::fs::read_to_string(&meta_path).await {
            Ok(m) => m,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(
                    "skipping job dir {}: meta.json not found",
                    dir.display()
                );
                continue;
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to read {}", meta_path.display()))
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
            Ok(status_json) => Some(
                serde_json::from_str::<JobStatus>(&status_json)
                    .with_context(|| format!("failed to parse {}", status_path.display()))?,
            ),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to read {}", status_path.display()));
            }
        };

        let started_at = DateTime::parse_from_rfc3339(&record.started_at)
            .with_context(|| format!("failed to parse started_at for job {}", record.id))?
            .with_timezone(&Utc);
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
        job.readiness = record.readiness;
        job.restart = record.restart;
        job.pty = record.pty;
        job.max_runtime_ms = record.max_runtime_ms;
        job.max_rss_mb = record.max_rss_mb;
        job.env = record.env;
        jobs.push(job);
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
}
