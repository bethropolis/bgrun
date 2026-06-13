use anyhow::Result;
use bgrun_proto::Command;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

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
    filter_regex: Option<String>,
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
            filter_regex: filter_regex.clone(),
        }))
        .await?;

    if !response.ok {
        let err = response.error.unwrap_or_default();
        anyhow::bail!("tail: {err}");
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

        // Follow mode: use StreamLogs for non-polling log stream
        if follow {
            stream_logs_follow(&socket_path, &id, json).await?;
        }
    }

    Ok(())
}

/// Connects to the daemon, sends a StreamLogs command, and prints live
/// LogLine entries as they arrive (non-polling).
async fn stream_logs_follow(
    socket_path: &std::path::Path,
    job_id: &str,
    json: bool,
) -> Result<()> {
    let stream = UnixStream::connect(socket_path).await?;
    let (reader, mut writer) = tokio::io::split(stream);
    let mut buf_reader = BufReader::new(reader);

    // Send StreamLogs command
    let request = bgrun_proto::Request {
        id: uuid::Uuid::new_v4().to_string(),
        command: Command::StreamLogs {
            id: job_id.to_string(),
        },
    };
    let cmd_json = serde_json::to_string(&request)?;
    writer.write_all(cmd_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    // Read the initial control response
    let mut buf = String::new();
    buf_reader.read_line(&mut buf).await?;
    let control: serde_json::Value = serde_json::from_str(buf.trim())?;
    if !control["ok"].as_bool().unwrap_or(false) {
        let err = control["error"].as_str().unwrap_or("stream failed");
        anyhow::bail!("tail --follow: {err}");
    }

    // Stream LogLine entries
    let mut line_buf = String::new();
    loop {
        line_buf.clear();
        match buf_reader.read_line(&mut line_buf).await {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line_buf.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if json {
                    println!("{}", trimmed);
                } else {
                    match serde_json::from_str::<serde_json::Value>(trimmed) {
                        Ok(val) => print_log_line(&val),
                        Err(_) => {}
                    }
                }
            }
            Err(_) => break,
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
