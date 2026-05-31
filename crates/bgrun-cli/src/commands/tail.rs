use anyhow::Result;
use bgrun_proto::Command;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::output_mode;

/// Shows the last N lines of a job's log.
pub async fn tail(id: String, lines: usize, digest: bool, level: Option<String>) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let response = client
        .send::<serde_json::Value>(Command::Tail(bgrun_proto::TailArgs {
            id: id.clone(),
            lines,
            digest,
            level,
        }))
        .await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    if let Some(data) = response.data {
        match output_mode() {
            crate::output::OutputMode::Human => {
                if digest {
                    // Show digest summary
                    let total = data["total_lines"].as_u64().unwrap_or(0);
                    let errors = data["errors"].as_u64().unwrap_or(0);
                    let warnings = data["warnings"].as_u64().unwrap_or(0);
                    println!("Lines: {total}  Errors: {errors}  Warnings: {warnings}");
                    if let Some(err) = data["last_error"].as_str() {
                        println!(
                            "Last error (line {}): {err}",
                            data["last_error_line"].as_u64().unwrap_or(0)
                        );
                    }
                } else if let Some(log_lines) = data["lines"].as_array() {
                    for line in log_lines {
                        let num = line["line_number"].as_u64().unwrap_or(0);
                        let content = line["content"].as_str().unwrap_or("");
                        let lower = content.to_lowercase();
                        if lower.contains("error") {
                            // Red for errors
                            println!("\x1b[31m{num:>6} | {content}\x1b[0m");
                        } else if lower.contains("warn") {
                            // Yellow for warnings
                            println!("\x1b[33m{num:>6} | {content}\x1b[0m");
                        } else {
                            println!("{num:>6} | {content}");
                        }
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
