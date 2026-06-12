use std::fmt;

use bgrun_proto::{JobState, ReadinessStrategy, RestartPolicy};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// Errors that can occur during job operations.
#[derive(Debug)]
pub enum JobError {
    InvalidTransition { from: JobState, to: JobState },
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JobError::InvalidTransition { from, to } => {
                write!(f, "invalid transition from {from} to {to}")
            }
        }
    }
}

impl std::error::Error for JobError {}

/// A managed process tracked by the daemon.
#[derive(Clone)]
pub struct Job {
    pub id: String,
    pub name: Option<String>,
    pub workspace: Option<String>,
    pub cmd: Vec<String>,
    pub pid: Option<u32>,
    pub state: JobState,
    pub started_at: DateTime<Utc>,
    pub ready_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub restart_count: u32,
    pub last_diff_cursor: u64,
    pub consecutive_failures: u32,
    pub readiness: Option<ReadinessStrategy>,
    pub restart: Option<RestartPolicy>,
    pub pty: bool,
    pub max_runtime_ms: Option<u64>,
    pub max_rss_mb: Option<u64>,
    pub env: HashMap<String, String>,
    pub cwd: Option<String>,
    pub allocated_port: Option<u16>,
}

impl Job {
    /// Creates a new job in the Starting state.
    pub fn new(
        id: String,
        cmd: Vec<String>,
        name: Option<String>,
        workspace: Option<String>,
    ) -> Self {
        Job {
            id,
            name,
            workspace,
            cmd,
            pid: None,
            state: JobState::Starting,
            started_at: Utc::now(),
            ready_at: None,
            exit_code: None,
            restart_count: 0,
            last_diff_cursor: 0,
            consecutive_failures: 0,
            readiness: None,
            restart: None,
            pty: false,
            max_runtime_ms: None,
            max_rss_mb: None,
            env: HashMap::new(),
            cwd: None,
            allocated_port: None,
        }
    }

    /// Transitions the job to a new state, returning an error if the transition is invalid.
    ///
    /// Valid transitions:
    /// - Starting -> Running, Ready
    /// - Running -> Ready, Exited, Crashed, Killed
    /// - Ready -> Exited, Crashed, Killed
    pub fn transition(&mut self, next: JobState) -> Result<(), JobError> {
        let valid = matches!(
            (&self.state, &next),
            (JobState::Starting, JobState::Running)
                | (JobState::Starting, JobState::Ready)
                | (JobState::Running, JobState::Ready)
                | (JobState::Running, JobState::Exited)
                | (JobState::Running, JobState::Crashed)
                | (JobState::Running, JobState::Killed)
                | (JobState::Ready, JobState::Exited)
                | (JobState::Ready, JobState::Crashed)
                | (JobState::Ready, JobState::Killed)
        );

        if !valid {
            return Err(JobError::InvalidTransition {
                from: self.state.clone(),
                to: next,
            });
        }

        self.state = next;
        Ok(())
    }

    /// Returns true if the job is in an alive state (Starting, Running, or Ready).
    pub fn is_alive(&self) -> bool {
        matches!(
            self.state,
            JobState::Starting | JobState::Running | JobState::Ready
        )
    }

    /// Converts the job into a JobRecord for serialization.
    pub fn to_record(&self) -> bgrun_proto::JobRecord {
        bgrun_proto::JobRecord {
            id: self.id.clone(),
            name: self.name.clone(),
            workspace: self.workspace.clone(),
            cmd: self.cmd.clone(),
            pid: self.pid,
            state: self.state.clone(),
            started_at: self.started_at.to_rfc3339(),
            readiness: self.readiness.clone(),
            restart: self.restart.clone(),
            pty: self.pty,
            max_runtime_ms: self.max_runtime_ms,
            max_rss_mb: self.max_rss_mb,
            env: self.env.clone(),
            cwd: self.cwd.clone(),
            allocated_port: self.allocated_port,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_job() -> Job {
        Job::new(
            "test-id".into(),
            vec!["sleep".into(), "60".into()],
            None,
            None,
        )
    }

    #[test]
    fn test_job_new_starts_in_starting() {
        let job = make_job();
        assert_eq!(job.state, JobState::Starting);
        assert!(job.is_alive());
    }

    #[test]
    fn test_transition_starting_to_running() {
        let mut job = make_job();
        job.transition(JobState::Running).unwrap();
        assert_eq!(job.state, JobState::Running);
        assert!(job.is_alive());
    }

    #[test]
    fn test_transition_starting_to_ready() {
        let mut job = make_job();
        job.transition(JobState::Ready).unwrap();
        assert_eq!(job.state, JobState::Ready);
        assert!(job.is_alive());
    }

    #[test]
    fn test_transition_running_to_ready() {
        let mut job = make_job();
        job.transition(JobState::Running).unwrap();
        job.transition(JobState::Ready).unwrap();
        assert_eq!(job.state, JobState::Ready);
        assert!(job.is_alive());
    }

    #[test]
    fn test_transition_running_to_exited() {
        let mut job = make_job();
        job.transition(JobState::Running).unwrap();
        job.transition(JobState::Exited).unwrap();
        assert_eq!(job.state, JobState::Exited);
        assert!(!job.is_alive());
    }

    #[test]
    fn test_transition_running_to_crashed() {
        let mut job = make_job();
        job.transition(JobState::Running).unwrap();
        job.transition(JobState::Crashed).unwrap();
        assert_eq!(job.state, JobState::Crashed);
        assert!(!job.is_alive());
    }

    #[test]
    fn test_transition_running_to_killed() {
        let mut job = make_job();
        job.transition(JobState::Running).unwrap();
        job.transition(JobState::Killed).unwrap();
        assert_eq!(job.state, JobState::Killed);
        assert!(!job.is_alive());
    }

    #[test]
    fn test_transition_ready_to_exited() {
        let mut job = make_job();
        job.transition(JobState::Ready).unwrap();
        job.transition(JobState::Exited).unwrap();
        assert_eq!(job.state, JobState::Exited);
    }

    #[test]
    fn test_transition_killed_to_ready_invalid() {
        let mut job = make_job();
        job.transition(JobState::Running).unwrap();
        job.transition(JobState::Killed).unwrap();
        let err = job.transition(JobState::Ready).unwrap_err();
        assert!(matches!(err, JobError::InvalidTransition { .. }));
    }

    #[test]
    fn test_transition_exited_to_running_invalid() {
        let mut job = make_job();
        job.transition(JobState::Running).unwrap();
        job.transition(JobState::Exited).unwrap();
        let err = job.transition(JobState::Running).unwrap_err();
        assert!(matches!(err, JobError::InvalidTransition { .. }));
    }

    #[test]
    fn test_is_alive_various_states() {
        let mut job = make_job();
        assert!(job.is_alive()); // Starting

        job.transition(JobState::Running).unwrap();
        assert!(job.is_alive()); // Running

        job.transition(JobState::Ready).unwrap();
        assert!(job.is_alive()); // Ready

        job.transition(JobState::Exited).unwrap();
        assert!(!job.is_alive()); // Exited
    }

    #[test]
    fn test_to_record() {
        let job = make_job();
        let record = job.to_record();
        assert_eq!(record.id, "test-id");
        assert_eq!(record.state, JobState::Starting);
        assert!(record.cmd.contains(&"sleep".to_string()));
    }

    #[test]
    fn test_error_display() {
        let err = JobError::InvalidTransition {
            from: JobState::Killed,
            to: JobState::Ready,
        };
        let msg = err.to_string();
        assert!(msg.contains("killed"));
        assert!(msg.contains("ready"));
    }
}
