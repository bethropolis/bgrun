use anyhow::Result;
use bgrun_proto::{Command, JobRecord};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::duration::BgrunDuration;
use crate::output::{output_mode, print_job};

/// Restarts a job: stops the live process (grace period `timeout`) and
/// re-spawns it from its stored definition (command, env, cwd, readiness,
/// restart policy). Returns the new job record.
pub async fn restart(id: String, timeout: String, json: bool) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let timeout_ms = timeout.parse::<BgrunDuration>()?.0;

    let response = client
        .send::<JobRecord>(Command::Restart {
            id: id.clone(),
            timeout_ms,
        })
        .await?;

    if !response.ok {
        let err = response.error.unwrap_or_default();
        anyhow::bail!("restart: {err}");
    }

    let record = response
        .data
        .ok_or_else(|| anyhow::anyhow!("restart: empty response from daemon"))?;
    print_job(&record, output_mode(json))?;

    Ok(())
}
