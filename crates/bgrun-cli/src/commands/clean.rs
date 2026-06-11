use anyhow::Result;
use bgrun_proto::Command;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;

/// Removes all terminal-state (crashed/exited/killed) jobs.
pub async fn clean(workspace: Option<String>) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let response = client
        .send::<serde_json::Value>(Command::Clean { workspace })
        .await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    if let Some(data) = response.data {
        let removed = data["removed"].as_u64().unwrap_or(0);
        println!("Removed {removed} terminated job(s).");
    }

    Ok(())
}
