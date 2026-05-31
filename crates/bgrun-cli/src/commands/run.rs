use std::collections::HashMap;

use anyhow::Result;
use bgrun_proto::{Command, JobRecord, ReadinessStrategy, RunArgs};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::{output_mode, print_job};

/// Optional flags for the run command.
pub struct RunFlags {
    pub ready_when: Option<String>,
    pub ready_when_port: Option<u16>,
    pub ready_when_url: Option<String>,
    pub ready_when_file: Option<String>,
    pub after: Option<String>,
    pub pty: bool,
    pub restart: Option<String>,
    pub backoff: Option<String>,
}

/// Runs a command in the background via the daemon.
pub async fn run(
    mut cmd: Vec<String>,
    mut name: Option<String>,
    mut workspace: Option<String>,
    mut flags: RunFlags,
) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    // Try to resolve from bgrun.toml if cmd is a single word matching a job name
    if cmd.len() == 1 {
        if let Some(resolved) = try_resolve_from_config(&cmd[0]).await {
            cmd = resolved.cmd;
            name = name.or(resolved.name);
            workspace = workspace.or(resolved.workspace);
            if flags.ready_when.is_none() && flags.ready_when_port.is_none() {
                match resolved.readiness {
                    Some(ReadinessStrategy::LogPattern(p)) => flags.ready_when = Some(p),
                    Some(ReadinessStrategy::TcpPort(p)) => flags.ready_when_port = Some(p),
                    Some(ReadinessStrategy::HttpPoll(u)) => flags.ready_when_url = Some(u),
                    Some(ReadinessStrategy::FileExists(f)) => flags.ready_when_file = Some(f),
                    None => {}
                }
            }
            flags.after = flags.after.or(resolved.after);
        }
    }

    let mut client = DaemonClient::connect(&socket_path).await?;

    // Resolve readiness strategy from flags (first match wins)
    let readiness = flags
        .ready_when
        .map(ReadinessStrategy::LogPattern)
        .or_else(|| flags.ready_when_port.map(ReadinessStrategy::TcpPort))
        .or_else(|| flags.ready_when_url.map(ReadinessStrategy::HttpPoll))
        .or_else(|| flags.ready_when_file.map(ReadinessStrategy::FileExists));

    // Resolve restart policy
    let restart = match flags.restart.as_deref() {
        Some("on-crash") => {
            let backoff_ms = flags
                .backoff
                .as_ref()
                .and_then(|b| parse_backoff_ms(b))
                .unwrap_or(2000);
            Some(bgrun_proto::RestartPolicy::OnCrash { backoff_ms })
        }
        _ => None,
    };

    let args = RunArgs {
        cmd,
        name,
        workspace,
        readiness,
        restart,
        pty: flags.pty,
        max_runtime_ms: None,
        env: HashMap::new(),
        after: flags.after,
    };

    let response = client.send::<JobRecord>(Command::Run(args)).await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    if let Some(record) = response.data {
        print_job(&record, output_mode())?;
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

/// Parses a backoff duration string like "2s", "500ms" into milliseconds.
fn parse_backoff_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix("ms") {
        n.parse().ok()
    } else if let Some(n) = s.strip_suffix('s') {
        n.parse::<u64>().ok().map(|n| n * 1_000)
    } else if let Some(n) = s.strip_suffix('m') {
        n.parse::<u64>().ok().map(|n| n * 60_000)
    } else {
        s.parse::<u64>().ok().map(|n| n * 1_000)
    }
}
