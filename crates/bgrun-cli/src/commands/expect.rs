use anyhow::Result;
use bgrun_proto::Command;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::duration::BgrunDuration;
use crate::output::output_mode;

/// Waits for a pattern in a job's log output.
pub async fn expect(id: String, pattern: String, is_regex: bool, timeout: String, json: bool) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let timeout_ms = timeout.parse::<BgrunDuration>()?.0;

    let mut client = DaemonClient::connect(&socket_path).await?;

    if output_mode(json) == crate::output::OutputMode::Human {
        eprintln!("Waiting for pattern in job {} (timeout: {})...", id, timeout);
    }

    let (cancel_tx, mut cancel_rx) = tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        let _ = cancel_tx.send(()).await;
    });

    let response = tokio::select! {
        result = client.send::<serde_json::Value>(Command::Expect {
            id: id.clone(),
            pattern,
            is_regex,
            timeout_ms,
        }) => result?,
        _ = cancel_rx.recv() => {
            println!("\nCancelled.");
            return Ok(());
        }
    };

    if !response.ok {
        let err = response.error.unwrap_or_default();
        anyhow::bail!("expect: {err}");
    }

    if let Some(data) = response.data {
        match output_mode(json) {
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


