use std::collections::HashMap;

use anyhow::Result;
use bgrun_proto::{Command, JobRecord, ReadinessStrategy, RunArgs};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::duration::parse_duration_ms;
use crate::output::{output_mode, print_job};

/// Optional flags for the run command.
pub struct RunFlags {
    pub ready_when: Option<String>,
    pub ready_when_regex: Option<String>,
    pub ready_when_port: Option<u16>,
    pub ready_when_url: Option<String>,
    pub ready_when_file: Option<String>,
    pub after: Option<String>,
    pub pty: bool,
    pub restart: Option<String>,
    pub backoff: Option<String>,
    pub pty_cols: Option<u16>,
    pub pty_rows: Option<u16>,
    pub max_rss_mb: Option<u64>,
    pub max_runtime_ms: Option<u64>,
    pub allocate_port: Option<String>,
    pub health_check_url: Option<String>,
    pub health_check_port: Option<u16>,
    pub health_interval: Option<u64>,
    pub health_threshold: Option<u32>,
}

/// Runs a command in the background via the daemon.
pub async fn run(
    mut cmd: Vec<String>,
    mut name: Option<String>,
    mut workspace: Option<String>,
    mut flags: RunFlags,
    json: bool,
) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    // Try to resolve from bgrun.toml if cmd is a single word matching a job name
    if cmd.len() == 1 {
        if let Some(resolved) = try_resolve_from_config(&cmd[0]).await {
            cmd = resolved.cmd;
            name = name.or(resolved.name);
            workspace = workspace.or(resolved.workspace);
            if flags.ready_when.is_none() && flags.ready_when_regex.is_none()
                && flags.ready_when_port.is_none() && flags.ready_when_url.is_none()
                && flags.ready_when_file.is_none()
            {
                match resolved.readiness {
                    Some(ReadinessStrategy::LogPattern(p)) => flags.ready_when = Some(p),
                    Some(ReadinessStrategy::LogPatternRegex(p)) => flags.ready_when_regex = Some(p),
                    Some(ReadinessStrategy::TcpPort(p)) => flags.ready_when_port = Some(p),
                    Some(ReadinessStrategy::HttpPoll(u)) => flags.ready_when_url = Some(u),
                    Some(ReadinessStrategy::FileExists(f)) => flags.ready_when_file = Some(f),
                    None => {}
                }
            }
            flags.after = flags.after.or(resolved.after);
        }
    }

    // Single-element strings that didn't match a config job: run via shell
    if cmd.len() == 1 {
        let shell_cmd = std::mem::take(&mut cmd[0]);
        cmd = vec!["sh".into(), "-c".into(), shell_cmd];
    }

    let mut client = DaemonClient::connect(&socket_path).await?;

    // Collect terminal and locale env vars to prevent TUI rendering corruption
    let mut env = HashMap::new();
    for (key, val) in std::env::vars() {
        if key == "TERM"
            || key == "COLORTERM"
            || key == "LANG"
            || key.starts_with("LC_")
        {
            env.insert(key, val);
        }
    }

    // Resolve readiness strategy from flags (first match wins)
    let readiness = flags
        .ready_when_regex
        .map(ReadinessStrategy::LogPatternRegex)
        .or_else(|| flags.ready_when.map(ReadinessStrategy::LogPattern))
        .or_else(|| flags.ready_when_port.map(ReadinessStrategy::TcpPort))
        .or_else(|| flags.ready_when_url.map(ReadinessStrategy::HttpPoll))
        .or_else(|| flags.ready_when_file.map(ReadinessStrategy::FileExists));

    // Resolve restart policy
    let restart = match flags.restart.as_deref() {
        Some("on-crash") => {
            let backoff_ms = match flags.backoff {
                Some(ref b) => Some(parse_duration_ms(b)?),
                None => None,
            }.unwrap_or(2000);
            Some(bgrun_proto::RestartPolicy::OnCrash { backoff_ms })
        }
        Some(other) => anyhow::bail!("invalid restart policy: {other:?} (expected 'on-crash')"),
        None => None,
    };

    // Resolve health check strategy
    let health_check = flags
        .health_check_url
        .map(|u| ReadinessStrategy::HttpPoll(u.clone()))
        .or_else(|| flags.health_check_port.map(ReadinessStrategy::TcpPort));

    let args = RunArgs {
        cmd,
        name,
        workspace,
        readiness,
        restart,
        pty: flags.pty,
        max_runtime_ms: flags.max_runtime_ms,
        env,
        after: flags.after,
        max_rss_mb: flags.max_rss_mb,
        cwd: std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()),
        pty_cols: flags.pty_cols,
        pty_rows: flags.pty_rows,
        allocate_port: flags.allocate_port,
        health_check,
        health_interval_secs: flags.health_interval,
        health_threshold: flags.health_threshold,
    };

    let response = client.send::<JobRecord>(Command::Run(args)).await?;

    if !response.ok {
        let err = response.error.unwrap_or_default();
        anyhow::bail!("run: {err}");
    }

    if let Some(record) = response.data {
        print_job(&record, output_mode(json))?;
    }

    Ok(())
}

/// Tries to find and parse a bgrun.toml, returning resolved RunArgs if the name matches.
async fn try_resolve_from_config(name: &str) -> Option<bgrun_proto::RunArgs> {
    let config_path = find_config(std::env::current_dir().ok()?).await?;
    let content = tokio::fs::read_to_string(&config_path).await.ok()?;
    let toml_str = content.as_str();
    let config = bgrun_core::config::parse_config(toml_str).ok()?;
    bgrun_core::config::resolve_job_args(name, &config).ok()
}

/// Walks from start to git root looking for bgrun.toml.
async fn find_config(start: std::path::PathBuf) -> Option<std::path::PathBuf> {
    let mut current = start;
    loop {
        let candidate = current.join("bgrun.toml");
        if tokio::fs::metadata(&candidate).await.is_ok() {
            return Some(candidate);
        }
        // Stop at git root
        if current.join(".git").exists() {
            return None;
        }
        current = current.parent()?.to_path_buf();
    }
}


