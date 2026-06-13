use anyhow::Result;
use bgrun_proto::Command;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::output_mode;

/// Shows the last N lines from a job's in-memory screen buffer (non-blocking).
pub async fn screen(id: String, lines: usize, json: bool) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let response = client
        .send::<serde_json::Value>(Command::Screen { id: id.clone(), lines })
        .await?;

    if !response.ok {
        let err = response.error.unwrap_or_default();
        anyhow::bail!("screen: {err}");
    }

    if let Some(data) = response.data {
        match output_mode(json) {
            crate::output::OutputMode::Human => {
                if let Some(lines) = data.as_array() {
                    for line in lines {
                        if let Some(text) = line.as_str() {
                            println!("{text}");
                        }
                    }
                }
                if data.as_array().map_or(true, |a| a.is_empty()) {
                    println!("(no output in screen buffer)");
                }
            }
            crate::output::OutputMode::Json => {
                println!("{}", serde_json::to_string(&data)?);
            }
        }
    }

    Ok(())
}
