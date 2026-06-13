use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::{ReadinessStrategy, RestartPolicy};

/// Arguments for the Run command.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct RunArgs {
    pub cmd: Vec<String>,
    pub name: Option<String>,
    pub workspace: Option<String>,
    pub readiness: Option<ReadinessStrategy>,
    pub restart: Option<RestartPolicy>,
    pub pty: bool,
    pub max_runtime_ms: Option<u64>,
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pty_cols: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pty_rows: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_rss_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub allocate_port: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub health_check: Option<ReadinessStrategy>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub health_interval_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub health_threshold: Option<u32>,
}

/// Arguments for the Kill command.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct KillArgs {
    pub id: Option<String>,
    pub workspace: Option<String>,
}

/// Arguments for the Tail command.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct TailArgs {
    pub id: String,
    pub lines: usize,
    pub digest: bool,
    pub level: Option<String>,
    #[serde(default)]
    pub strip_ansi: bool,
    /// Filter by stream source: "stdout", "stderr", "pty". None = all.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stream: Option<String>,
    /// Byte cursor for follow mode (return lines after this offset).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cursor: Option<u64>,
    /// If true, response includes a cursor for follow-up polling.
    #[serde(default)]
    pub follow: bool,
    /// Regex filter: only return lines matching this pattern.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub filter_regex: Option<String>,
}

/// All commands the daemon can handle.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(tag = "command", content = "args")]
pub enum Command {
    Run(RunArgs),
    Status { id: String },
    List { workspace: Option<String> },
    Kill(KillArgs),
    Tail(TailArgs),
    Diff {
        id: String,
        lines: Option<usize>,
        #[serde(default)]
        strip_ansi: bool,
        /// Filter by stream source: "stdout", "stderr", "pty". None = all.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        stream: Option<String>,
        /// Regex filter: only return lines matching this pattern.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        filter_regex: Option<String>,
    },
    Wait { id: String, timeout_ms: u64 },
    Send { id: String, data: String },
    Stats { id: String },
    RunGroup { jobs: Vec<RunArgs> },
    Expect {
        id: String,
        pattern: String,
        is_regex: bool,
        timeout_ms: u64,
    },
    Attach { id: String },
    ResizePty { id: String, cols: u16, rows: u16 },
    /// Remove all terminal-state (crashed/exited/killed) jobs.
    Clean { workspace: Option<String> },
    /// Stream log lines in real time (connection-hijacking, like Attach).
    StreamLogs { id: String },
    /// Read the last N lines from the in-memory screen buffer (non-blocking).
    Screen { id: String, lines: usize },
}

/// A request sent from CLI to daemon.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub command: Command,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_command_tag() {
        let request = Request {
            id: "req-1".into(),
            command: Command::Status { id: "job-1".into() },
        };

        let json = serde_json::to_string(&request).expect("request should serialize");
        assert!(json.contains("\"command\":\"Status\""));
        assert!(json.contains("\"args\":{\"id\":\"job-1\"}"));
    }
}
