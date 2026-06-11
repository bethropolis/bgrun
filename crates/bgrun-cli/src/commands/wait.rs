use anyhow::Result;
use bgrun_proto::{Command, WaitResult};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::output_mode;

/// Parses a duration string like "5s", "30s", "2m" into milliseconds.
fn parse_duration_ms(s: &str) -> Result<u64> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('s') {
        Ok(n.parse::<u64>()? * 1_000)
    } else if let Some(n) = s.strip_suffix('m') {
        Ok(n.parse::<u64>()? * 60_000)
    } else if let Some(n) = s.strip_suffix('h') {
        Ok(n.parse::<u64>()? * 3_600_000)
    } else {
        // Assume seconds if no suffix
        Ok(s.parse::<u64>()? * 1_000)
    }
}

/// Waits for a job to become ready or until timeout.
pub async fn wait(id: String, timeout: String, json: bool) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let timeout_ms = parse_duration_ms(&timeout)?;

    if output_mode(json) == crate::output::OutputMode::Human {
        eprintln!("Waiting for job {} (timeout: {})...", id, timeout);
    }

    let response = client
        .send::<WaitResult>(Command::Wait {
            id: id.clone(),
            timeout_ms,
        })
        .await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    if let Some(result) = response.data {
        match output_mode(json) {
            crate::output::OutputMode::Human => {
                if result.ready {
                    println!("Job {} is ready ({}ms)", id, result.elapsed_ms);
                } else if let Some(ref s) = result.state {
                    let ec = result
                        .exit_code
                        .map(|c| format!(", exit_code={}", c))
                        .unwrap_or_default();
                    println!(
                        "Job {} reached terminal state {}{} ({}ms)",
                        id, s, ec, result.elapsed_ms
                    );
                } else {
                    println!(
                        "Job {} did not become ready within {}ms",
                        id, result.elapsed_ms
                    );
                }
            }
            crate::output::OutputMode::Json => {
                println!("{}", serde_json::to_string(&result)?);
            }
        }
    }

    Ok(())
}
