use anyhow::Result;
use bgrun_proto::Command;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::duration::BgrunDuration;
use crate::output::output_mode;

/// Gracefully stops a job: SIGTERM (escalating to SIGKILL after the timeout)
/// if alive, and always marks it Killed so pending policy restarts stand
/// down. Unlike `kill`, stopping a terminal-state job succeeds.
pub async fn stop(id: String, timeout: String, json: bool) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let timeout_ms = timeout.parse::<BgrunDuration>()?.0;

    let response = client
        .send::<serde_json::Value>(Command::Stop {
            id: id.clone(),
            timeout_ms,
        })
        .await?;

    if !response.ok {
        let err = response.error.unwrap_or_default();
        anyhow::bail!("stop: {err}");
    }

    if output_mode(json) == crate::output::OutputMode::Json {
        if let Some(val) = response.data {
            println!("{}", serde_json::to_string(&val)?);
        }
    } else {
        println!("Stopped job {id}");
    }

    Ok(())
}
