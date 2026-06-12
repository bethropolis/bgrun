use std::collections::HashMap;

use bgrun_proto::{ReadinessStrategy, RestartPolicy, RunArgs};
use serde::Deserialize;

/// Parsed representation of a bgrun.toml config file.
#[derive(Deserialize, Debug, Clone)]
pub struct BgrunToml {
    #[serde(default)]
    pub jobs: HashMap<String, JobConfig>,
}

/// Supports either a single quoted string or an array of strings for `cmd`.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum TomlCmd {
    /// A single string that will be lexed with `shlex`.
    Single(String),
    /// Pre-split argument array.
    Array(Vec<String>),
}

/// Configuration for a single named job.
#[derive(Deserialize, Debug, Clone)]
pub struct JobConfig {
    pub cmd: TomlCmd,
    #[serde(rename = "ready-when")]
    pub ready_when: Option<String>,
    #[serde(rename = "ready-when-port")]
    pub ready_when_port: Option<u16>,
    #[serde(rename = "ready-when-url")]
    pub ready_when_url: Option<String>,
    #[serde(rename = "ready-when-file")]
    pub ready_when_file: Option<String>,
    pub restart: Option<String>,
    pub workspace: Option<String>,
    pub after: Option<String>,
    pub pty: Option<bool>,
    #[serde(rename = "max-rss-mb")]
    pub max_rss_mb: Option<u64>,
    #[serde(rename = "max-runtime-ms")]
    pub max_runtime_ms: Option<u64>,
    #[serde(rename = "backoff-ms")]
    pub backoff_ms: Option<u64>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    #[serde(rename = "allocate-port")]
    pub allocate_port: Option<String>,
    #[serde(rename = "health-check-url")]
    pub health_check_url: Option<String>,
    #[serde(rename = "health-check-port")]
    pub health_check_port: Option<u16>,
    #[serde(rename = "health-interval-secs")]
    pub health_interval_secs: Option<u64>,
    #[serde(rename = "health-threshold")]
    pub health_threshold: Option<u32>,
}

/// Errors during config parsing or resolution.
#[derive(Debug)]
pub enum ConfigError {
    /// The named job was not found in the config.
    JobNotFound(String),
    /// Failed to parse the TOML content.
    ParseError(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::JobNotFound(name) => write!(f, "job '{}' not found in config", name),
            ConfigError::ParseError(msg) => write!(f, "config parse error: {}", msg),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Parses a bgrun.toml string into a BgrunToml struct (pure, no I/O).
pub fn parse_config(content: &str) -> Result<BgrunToml, ConfigError> {
    toml::from_str(content).map_err(|e| ConfigError::ParseError(e.to_string()))
}

/// Resolves a named job from the config into RunArgs.
pub fn resolve_job_args(name: &str, config: &BgrunToml) -> Result<RunArgs, ConfigError> {
    let job = config
        .jobs
        .get(name)
        .ok_or_else(|| ConfigError::JobNotFound(name.into()))?;

    // Parse cmd: array is used directly, single string goes through shlex
    let cmd: Vec<String> = match &job.cmd {
        TomlCmd::Array(arr) => arr.clone(),
        TomlCmd::Single(s) => {
            shlex::split(s).ok_or_else(|| ConfigError::ParseError(format!(
                "failed to parse cmd string for job '{}': unmatched quotes or invalid syntax", name
            )))?
        }
    };

    // Resolve readiness strategy
    let readiness = job
        .ready_when
        .as_ref()
        .map(|p| ReadinessStrategy::LogPattern(p.clone()))
        .or_else(|| job.ready_when_port.map(ReadinessStrategy::TcpPort))
        .or_else(|| {
            job.ready_when_url
                .as_ref()
                .map(|u| ReadinessStrategy::HttpPoll(u.clone()))
        })
        .or_else(|| {
            job.ready_when_file
                .as_ref()
                .map(|f| ReadinessStrategy::FileExists(f.clone()))
        });

    // Resolve restart policy from config string with optional backoff
    let backoff_ms = job.backoff_ms.unwrap_or(2000);
    let restart = job.restart.as_deref().and_then(|s| match s {
        "on-crash" => Some(RestartPolicy::OnCrash { backoff_ms }),
        _ => None,
    });

    // Resolve health check strategy (separate from readiness)
    let health_check = job
        .health_check_url
        .as_ref()
        .map(|u| ReadinessStrategy::HttpPoll(u.clone()))
        .or_else(|| job.health_check_port.map(ReadinessStrategy::TcpPort));

    Ok(RunArgs {
        cmd,
        name: Some(name.into()),
        workspace: job.workspace.clone(),
        readiness,
        restart,
        pty: job.pty.unwrap_or(false),
        max_runtime_ms: job.max_runtime_ms,
        max_rss_mb: job.max_rss_mb,
        env: job.env.clone().unwrap_or_default(),
        after: job.after.clone(),
        cwd: job.cwd.clone(),
        allocate_port: job.allocate_port.clone(),
        health_check,
        health_interval_secs: job.health_interval_secs,
        health_threshold: job.health_threshold,
        pty_cols: None,
        pty_rows: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_toml() -> &'static str {
        r#"
[jobs.server]
cmd = "cargo run --release"
ready-when = "listening on"
workspace = "myproject"

[jobs.db]
cmd = "docker run --rm -p 5432:5432 postgres:16"
ready-when-port = 5432
workspace = "myproject"

[jobs.worker]
cmd = "cargo run --bin worker"
after = "db"
workspace = "myproject"

[jobs.reliable]
cmd = "python worker.py"
restart = "on-crash"
workspace = "myproject"
"#
    }

    #[test]
    fn test_parse_config() {
        let config = parse_config(sample_toml()).unwrap();
        assert!(config.jobs.contains_key("server"));
        assert!(config.jobs.contains_key("db"));
        assert!(config.jobs.contains_key("worker"));
    }

    #[test]
    fn test_resolve_server() {
        let config = parse_config(sample_toml()).unwrap();
        let args = resolve_job_args("server", &config).unwrap();
        assert_eq!(args.cmd, vec!["cargo", "run", "--release"]);
        assert_eq!(args.name, Some("server".into()));
        assert_eq!(
            args.readiness,
            Some(ReadinessStrategy::LogPattern("listening on".into()))
        );
        assert_eq!(args.workspace, Some("myproject".into()));
    }

    #[test]
    fn test_resolve_db_with_port() {
        let config = parse_config(sample_toml()).unwrap();
        let args = resolve_job_args("db", &config).unwrap();
        assert_eq!(
            args.cmd,
            vec!["docker", "run", "--rm", "-p", "5432:5432", "postgres:16"]
        );
        assert_eq!(args.readiness, Some(ReadinessStrategy::TcpPort(5432)));
    }

    #[test]
    fn test_resolve_missing_job() {
        let config = parse_config(sample_toml()).unwrap();
        let err = resolve_job_args("nonexistent", &config).unwrap_err();
        assert!(matches!(err, ConfigError::JobNotFound(_)));
    }

    #[test]
    fn test_resolve_worker_no_readiness() {
        let config = parse_config(sample_toml()).unwrap();
        let args = resolve_job_args("worker", &config).unwrap();
        assert_eq!(args.cmd, vec!["cargo", "run", "--bin", "worker"]);
        assert!(args.readiness.is_none());
    }

    #[test]
    fn test_resolve_restart_policy() {
        let config = parse_config(sample_toml()).unwrap();
        let args = resolve_job_args("reliable", &config).unwrap();
        assert_eq!(args.restart, Some(RestartPolicy::OnCrash { backoff_ms: 2000 }));
    }

    #[test]
    fn test_array_cmd_syntax() {
        let toml = r#"
[jobs.server]
cmd = ["cargo", "run", "--release"]
ready-when = "listening on"
"#;
        let config = parse_config(toml).unwrap();
        let args = resolve_job_args("server", &config).unwrap();
        assert_eq!(args.cmd, vec!["cargo", "run", "--release"]);
    }

    #[test]
    fn test_quoted_cmd_with_spaces() {
        let toml = r#"
[jobs.test]
cmd = "echo 'hello world'"
"#;
        let config = parse_config(toml).unwrap();
        let args = resolve_job_args("test", &config).unwrap();
        assert_eq!(args.cmd, vec!["echo", "hello world"]);
    }

    #[test]
    fn test_empty_toml() {
        let config = parse_config("").unwrap();
        assert!(config.jobs.is_empty());
    }

    #[test]
    fn test_invalid_toml() {
        let err = parse_config("not valid toml {{{").unwrap_err();
        assert!(matches!(err, ConfigError::ParseError(_)));
    }
}
