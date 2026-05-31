use anyhow::Result;
use bgrun_proto::Command;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::output_mode;

/// Sends data to a job's stdin.
pub async fn send(id: String, data: String) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let response = client
        .send::<serde_json::Value>(Command::Send {
            id: id.clone(),
            data,
        })
        .await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    if output_mode() == crate::output::OutputMode::Human {
        println!("Sent stdin to job {id}");
    } else if let Some(val) = response.data {
        println!("{}", serde_json::to_string(&val)?);
    }

    Ok(())
}
