use anyhow::Result;
use bgrun_proto::Command;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::output_mode;

/// Shows log lines since the last diff call.
pub async fn diff(id: String) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let response = client
        .send::<serde_json::Value>(Command::Diff { id: id.clone() })
        .await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    if let Some(data) = response.data {
        match output_mode() {
            crate::output::OutputMode::Human => {
                if let Some(log_lines) = data["lines"].as_array() {
                    for line in log_lines {
                        let num = line["line_number"].as_u64().unwrap_or(0);
                        let content = line["content"].as_str().unwrap_or("");
                        let lower = content.to_lowercase();
                        if lower.contains("error") {
                            println!("\x1b[31m{num:>6} | {content}\x1b[0m");
                        } else if lower.contains("warn") {
                            println!("\x1b[33m{num:>6} | {content}\x1b[0m");
                        } else {
                            println!("{num:>6} | {content}");
                        }
                    }
                    if log_lines.is_empty() {
                        println!("No new lines.");
                    }
                }
            }
            crate::output::OutputMode::Json => {
                println!("{}", serde_json::to_string(&data)?);
            }
        }
    }

    Ok(())
}
