use anyhow::Result;
use bgrun_proto::{Command, WaitResult};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::duration::parse_duration_ms;
use crate::output::output_mode;

/// Waits for a job to become ready or until timeout.
pub async fn wait(id: String, timeout: String, json: bool) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let timeout_ms = parse_duration_ms(&timeout)?;

    if output_mode(json) == crate::output::OutputMode::Human {
        eprintln!("Waiting for job {} (timeout: {})...", id, timeout);
    }

    let (cancel_tx, mut cancel_rx) = tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        let _ = cancel_tx.send(()).await;
    });

    let response = tokio::select! {
        result = client.send::<WaitResult>(Command::Wait {
            id: id.clone(),
            timeout_ms,
        }) => result?,
        _ = cancel_rx.recv() => {
            println!("\nCancelled.");
            return Ok(());
        }
    };

    if !response.ok {
        let err = response.error.unwrap_or_default();
        anyhow::bail!("wait: {err}");
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
