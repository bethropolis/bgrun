use serde::{Deserialize, Serialize};

use std::collections::HashMap;

use crate::types::{JobState, ReadinessStrategy, RestartPolicy};

/// A record of a job returned to the client.
///
/// Includes full RunArgs fields for meta.json persistence across daemon restarts.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct JobRecord {
    pub id: String,
    pub name: Option<String>,
    pub workspace: Option<String>,
    pub cmd: Vec<String>,
    pub pid: Option<u32>,
    pub state: JobState,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub readiness: Option<ReadinessStrategy>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub restart: Option<RestartPolicy>,
    #[serde(skip_serializing_if = "is_false", default)]
    pub pty: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_runtime_ms: Option<u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_rss_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub allocated_port: Option<u16>,
}

/// Helper for serde skip_serializing_if on bool fields.
fn is_false(b: &bool) -> bool {
    !*b
}

/// The current status of a job.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct JobStatus {
    pub state: JobState,
    pub exit_code: Option<i32>,
    pub ready_at: Option<String>,
    pub restart_count: u32,
    #[serde(default)]
    pub last_diff_cursor: u64,
    #[serde(default)]
    pub consecutive_failures: u32,
}

/// Response envelope for all daemon replies.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Response<T> {
    pub id: String,
    pub ok: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> Response<T> {
    /// Creates a success response with the given data.
    pub fn ok(id: String, data: T) -> Self {
        Response {
            id,
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    /// Creates an error response with the given message.
    pub fn err(id: String, msg: impl Into<String>) -> Self {
        Response {
            id,
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

/// A single log line.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LogLine {
    pub line_number: u64,
    pub content: String,
    pub timestamp: Option<String>,
}

/// Result of a wait command.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WaitResult {
    pub ready: bool,
    pub elapsed_ms: u64,
    pub exit_code: Option<i32>,
    pub state: Option<String>,
}

/// Digest summary of a job's log.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LogDigest {
    pub total_lines: u64,
    pub errors: u64,
    pub warnings: u64,
    pub last_error: Option<String>,
    pub last_error_line: Option<u64>,
}

/// Resource stats for a running process.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ResourceStats {
    pub cpu_pct: f32,
    pub rss_mb: u64,
    pub uptime_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_ok_constructor() {
        let r = Response::ok("req-1".into(), "hello".to_string());
        assert!(r.ok);
        assert_eq!(r.data, Some("hello".to_string()));
        assert_eq!(r.error, None);
    }

    #[test]
    fn test_response_err_constructor() {
        let r = Response::<String>::err("req-1".into(), "something went wrong");
        assert!(!r.ok);
        assert_eq!(r.data, None);
        assert_eq!(r.error, Some("something went wrong".into()));
    }

    #[test]
    fn test_job_state_display() {
        assert_eq!(JobState::Starting.to_string(), "starting");
        assert_eq!(JobState::Running.to_string(), "running");
        assert_eq!(JobState::Ready.to_string(), "ready");
        assert_eq!(JobState::Exited.to_string(), "exited");
        assert_eq!(JobState::Crashed.to_string(), "crashed");
        assert_eq!(JobState::Killed.to_string(), "killed");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let state = JobState::Running;
        let json = serde_json::to_string(&state).unwrap();
        let parsed: JobState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, parsed);
    }

    #[test]
    fn test_response_serialization() {
        let r = Response::ok("req-1".into(), JobState::Ready);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"ready\""));
    }
}
