use anyhow::Result;
use bgrun_proto::{Command, ResourceStats};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::output_mode;

/// Shows resource stats for a running job.
pub async fn stats(id: String) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let response = client
        .send::<ResourceStats>(Command::Stats { id: id.clone() })
        .await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    if let Some(stats) = response.data {
        match output_mode() {
            crate::output::OutputMode::Human => {
                println!("Job:    {id}");
                println!("CPU:    {:.1}%", stats.cpu_pct);
                println!("Memory: {} MB", stats.rss_mb);
                println!("Uptime: {}s", stats.uptime_secs);
            }
            crate::output::OutputMode::Json => {
                println!("{}", serde_json::to_string(&stats)?);
            }
        }
    }

    Ok(())
}
