use anyhow::Result;
use bgrun_proto::{Command, JobRecord, RunArgs};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::{output_mode, print_jobs};

/// Runs multiple named jobs in parallel.
///
/// Each name is resolved through bgrun.toml if found; otherwise used as a raw command.
pub async fn run_group(names: Vec<String>, json: bool) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    // Resolve each name to RunArgs via bgrun.toml
    let config = load_config().await;
    let jobs: Vec<RunArgs> = names
        .iter()
        .map(|name| {
            if let Some(ref cfg) = config {
                if let Ok(args) = bgrun_core::config::resolve_job_args(name, cfg) {
                    return args;
                }
            }
            // Fallback: treat name as a raw command
            RunArgs {
                cmd: vec![name.clone()],
                name: Some(name.clone()),
                workspace: None,
                readiness: None,
                restart: None,
                pty: false,
                max_runtime_ms: None,
                max_rss_mb: None,
                env: std::collections::HashMap::new(),
                after: None,
                cwd: std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()),
                allocate_port: None,
                health_check: None,
                health_interval_secs: None,
                health_threshold: None,
                pty_cols: None,
                pty_rows: None,
            }
        })
        .collect();

    let response = client
        .send::<Vec<JobRecord>>(Command::RunGroup { jobs })
        .await?;

    if !response.ok {
        let err = response.error.unwrap_or_default();
        anyhow::bail!("run-group: {err}");
    }

    if let Some(records) = response.data {
        print_jobs(&records, output_mode(json))?;
    }

    Ok(())
}

/// Loads and parses bgrun.toml from the current directory upward.
async fn load_config() -> Option<bgrun_core::BgrunToml> {
    let start = std::env::current_dir().ok()?;
    let mut current = start;
    loop {
        let candidate = current.join("bgrun.toml");
        if let Ok(content) = tokio::fs::read_to_string(&candidate).await {
            if let Ok(config) = bgrun_core::config::parse_config(&content) {
                return Some(config);
            }
        }
        if current.join(".git").exists() {
            return None;
        }
        current = current.parent()?.to_path_buf();
    }
}
