use anyhow::Result;
use bgrun_proto::Command;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::output_mode;

/// Waits for a pattern in a job's log output.
pub async fn expect(id: String, pattern: String, is_regex: bool, timeout: String) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let timeout_ms = parse_timeout_ms(&timeout)?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let response = client
        .send::<serde_json::Value>(Command::Expect {
            id: id.clone(),
            pattern,
            is_regex,
            timeout_ms,
        })
        .await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    if let Some(data) = response.data {
        match output_mode() {
            crate::output::OutputMode::Human => {
                if data["matched"].as_bool().unwrap_or(false) {
                    let line = data["line_number"].as_u64().unwrap_or(0);
                    let content = data["content"].as_str().unwrap_or("");
                    println!("Pattern matched at line {line}: {content}");
                } else {
                    println!("Pattern did not match before timeout");
                }
            }
            crate::output::OutputMode::Json => {
                println!("{}", serde_json::to_string(&data)?);
            }
        }
    }

    Ok(())
}

/// Parses a duration string like "5s", "30s", "2m", "1h" into milliseconds.
fn parse_timeout_ms(s: &str) -> Result<u64> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix("ms") {
        n.parse::<u64>()
            .map_err(|_| anyhow::anyhow!("invalid timeout: {s}"))
    } else if let Some(n) = s.strip_suffix('s') {
        n.parse::<u64>()
            .map(|n| n * 1_000)
            .map_err(|_| anyhow::anyhow!("invalid timeout: {s}"))
    } else if let Some(n) = s.strip_suffix('m') {
        n.parse::<u64>()
            .map(|n| n * 60_000)
            .map_err(|_| anyhow::anyhow!("invalid timeout: {s}"))
    } else if let Some(n) = s.strip_suffix('h') {
        n.parse::<u64>()
            .map(|n| n * 3_600_000)
            .map_err(|_| anyhow::anyhow!("invalid timeout: {s}"))
    } else {
        // treat bare number as seconds
        s.parse::<u64>()
            .map(|n| n * 1_000)
            .map_err(|_| anyhow::anyhow!("invalid timeout: {s}"))
    }
}
