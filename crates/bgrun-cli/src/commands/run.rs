use std::collections::HashMap;

use anyhow::Result;
use bgrun_proto::{Command, JobRecord, ReadinessStrategy, RunArgs};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::duration::{BgrunDuration, BgrunSize};
use crate::output::{output_mode, print_job};

/// Optional flags for the run command.
#[derive(Debug)]
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
    pub max_retries: Option<u32>,
    pub log_max_size: Option<String>,
    pub log_keep: Option<u32>,
    pub pty_cols: Option<u16>,
    pub pty_rows: Option<u16>,
    pub max_rss_mb: Option<u64>,
    pub max_runtime_ms: Option<u64>,
    pub allocate_port: Option<String>,
    pub health_check_url: Option<String>,
    pub health_check_port: Option<u16>,
    pub health_interval: Option<u64>,
    pub health_threshold: Option<u32>,
    pub env: Vec<String>,
    pub cwd: Option<String>,
    pub replace: bool,
    pub wait: bool,
    pub wait_timeout: String,
}

/// Parses `KEY=VAL` entries into a map. Errors on missing `=` or empty keys.
fn parse_env_list(entries: &[String]) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for entry in entries {
        let (key, val) = entry.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("invalid --env {entry:?}: expected KEY=VAL")
        })?;
        if key.is_empty() {
            anyhow::bail!("invalid --env {entry:?}: key must not be empty");
        }
        out.insert(key.to_string(), val.to_string());
    }
    Ok(out)
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

    if flags.replace && name.is_none() {
        anyhow::bail!("run: --replace requires --name");
    }

    // Validate --wait timeout early so typos fail before spawning.
    if flags.wait {
        flags.wait_timeout.parse::<BgrunDuration>()?;
    }

    let mut client = DaemonClient::connect(&socket_path).await?;

    // --replace: kill any alive job with the same name first (best effort).
    if flags.replace {
        if let Some(ref n) = name {
            let kill_args = bgrun_proto::KillArgs {
                id: Some(n.clone()),
                workspace: None,
            };
            // Ignore errors: job may not exist, which is the common case.
            let _ = client
                .send::<serde_json::Value>(Command::Kill(kill_args))
                .await;
        }
    }

    // Collect terminal and locale env vars to prevent TUI rendering corruption,
    // then overlay explicit --env entries (explicit wins).
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
    env.extend(parse_env_list(&flags.env)?);

    // Resolve restart policy first: it borrows `flags` wholesale, which must
    // happen before the readiness chain below partially moves flag fields
    // into its closures.
    let restart = resolve_restart_policy(&flags)?;

    // Resolve readiness strategy from flags (first match wins)
    let readiness = flags
        .ready_when_regex
        .map(ReadinessStrategy::LogPatternRegex)
        .or_else(|| flags.ready_when.map(ReadinessStrategy::LogPattern))
        .or_else(|| flags.ready_when_port.map(ReadinessStrategy::TcpPort))
        .or_else(|| flags.ready_when_url.map(ReadinessStrategy::HttpPoll))
        .or_else(|| flags.ready_when_file.map(ReadinessStrategy::FileExists));

    // Resolve health check strategy
    let health_check = flags
        .health_check_url
        .map(|u| ReadinessStrategy::HttpPoll(u.clone()))
        .or_else(|| flags.health_check_port.map(ReadinessStrategy::TcpPort));

    let cwd = match flags.cwd {
        Some(c) => Some(c),
        None => std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
    };

    let log_max_size = match flags.log_max_size {
        Some(ref s) => Some(s.parse::<BgrunSize>()?.0),
        None => None,
    };
    if let Some(keep) = flags.log_keep {
        if keep == 0 {
            anyhow::bail!("invalid --log-keep 0: must retain at least one rotated file");
        }
    }

    let args = RunArgs {
        cmd,
        name,
        workspace,
        readiness,
        restart,
        max_retries: flags.max_retries,
        log_max_size,
        log_keep: flags.log_keep,
        pty: flags.pty,
        max_runtime_ms: flags.max_runtime_ms,
        env,
        after: flags.after,
        max_rss_mb: flags.max_rss_mb,
        cwd,
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

    let record = response.data.ok_or_else(|| anyhow::anyhow!("run: empty response from daemon"))?;
    print_job(&record, output_mode(json))?;

    // --wait: block until Ready (single-shot run+wait for agents).
    if flags.wait {
        wait_for_started_job(&mut client, &record.id, &flags.wait_timeout, json).await?;
    }

    Ok(())
}

/// Resolves `--restart`/`--backoff` into a policy. The backoff is shared by
/// all restart-on-exit policies.
fn resolve_restart_policy(flags: &RunFlags) -> Result<Option<bgrun_proto::RestartPolicy>> {
    let backoff_ms = match flags.backoff {
        Some(ref b) => Some(b.parse::<BgrunDuration>()?.0),
        None => None,
    }
    .unwrap_or(2000);
    match flags.restart.as_deref() {
        Some("never") => Ok(Some(bgrun_proto::RestartPolicy::Never)),
        Some("on-crash") => Ok(Some(bgrun_proto::RestartPolicy::OnCrash { backoff_ms })),
        Some("on-failure") => Ok(Some(bgrun_proto::RestartPolicy::OnFailure { backoff_ms })),
        Some("always") => Ok(Some(bgrun_proto::RestartPolicy::Always { backoff_ms })),
        Some(other) => anyhow::bail!(
            "invalid restart policy: {other:?} (expected 'never', 'on-crash', 'on-failure' or 'always')"
        ),
        None => Ok(None),
    }
}

/// Waits for a just-started job to become Ready and reports the outcome.
async fn wait_for_started_job(
    client: &mut DaemonClient,
    job_id: &str,
    timeout: &str,
    json: bool,
) -> Result<()> {
    let timeout_ms = timeout.parse::<BgrunDuration>()?.0;
    let wait_resp = client
        .send::<bgrun_proto::WaitResult>(Command::Wait {
            id: job_id.to_string(),
            timeout_ms,
        })
        .await?;
    if !wait_resp.ok {
        let err = wait_resp.error.unwrap_or_default();
        anyhow::bail!("run --wait: {err}");
    }
    if let Some(result) = wait_resp.data {
        print_wait_result(job_id, &result, json)?;
    }
    Ok(())
}

/// Prints a wait outcome in human or JSON form.
fn print_wait_result(job_id: &str, result: &bgrun_proto::WaitResult, json: bool) -> Result<()> {
    match output_mode(json) {
        crate::output::OutputMode::Human => {
            if result.ready {
                println!("Job {job_id} is ready ({}ms)", result.elapsed_ms);
            } else if let Some(ref s) = result.state {
                let ec = result
                    .exit_code
                    .map(|c| format!(", exit_code={c}"))
                    .unwrap_or_default();
                println!("Job {job_id} reached terminal state {s}{ec} ({}ms)", result.elapsed_ms);
            } else {
                println!(
                    "Job {job_id} did not become ready within {}ms",
                    result.elapsed_ms
                );
            }
        }
        crate::output::OutputMode::Json => {
            println!("{}", serde_json::to_string(result)?);
        }
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


