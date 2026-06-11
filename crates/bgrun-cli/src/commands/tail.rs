use anyhow::Result;
use bgrun_proto::Command;
use std::time::Duration;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::output_mode;

/// Shows the last N lines of a job's log.
pub async fn tail(
    id: String,
    lines: usize,
    digest: bool,
    level: Option<String>,
    stream: Option<String>,
    strip_ansi: bool,
    follow: bool,
    json: bool,
) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let response = client
        .send::<serde_json::Value>(Command::Tail(bgrun_proto::TailArgs {
            id: id.clone(),
            lines,
            digest,
            level: level.clone(),
            strip_ansi,
            stream: stream.clone(),
            cursor: None,
            follow,
        }))
        .await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    if let Some(data) = response.data {
        match output_mode(json) {
            crate::output::OutputMode::Human => {
                if digest {
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
                        print_log_line(line);
                    }
                }
            }
            crate::output::OutputMode::Json => {
                println!("{}", serde_json::to_string(&data)?);
            }
        }

        // Follow mode: poll for new lines using cursor
        if follow {
            let mut cursor = data["cursor"].as_u64().unwrap_or(0);
            loop {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let mut client = DaemonClient::connect(&socket_path).await?;
                let resp = client
                    .send::<serde_json::Value>(Command::Tail(bgrun_proto::TailArgs {
                        id: id.clone(),
                        lines: 0,
                        digest: false,
                        level: level.clone(),
                        strip_ansi,
                        stream: stream.clone(),
                        cursor: Some(cursor),
                        follow: false,
                    }))
                    .await?;
                if !resp.ok {
                    break;
                }
                if let Some(d) = resp.data {
                    if let Some(new_lines) = d["lines"].as_array() {
                        if !new_lines.is_empty() {
                            for line in new_lines {
                                print_log_line(line);
                            }
                        }
                    }
                    if let Some(new_cursor) = d["cursor"].as_u64() {
                        cursor = new_cursor;
                    }
                }
            }
        }
    }

    Ok(())
}

fn print_log_line(line: &serde_json::Value) {
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
