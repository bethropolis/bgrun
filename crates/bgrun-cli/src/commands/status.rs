use anyhow::Result;
use bgrun_proto::{Command, JobStatus};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::{output_mode, print_status};

/// Gets the status of a specific job by ID.
pub async fn status(id: String) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let response = client.send::<JobStatus>(Command::Status { id }).await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    if let Some(status) = response.data {
        print_status(&status, output_mode())?;
    }

    Ok(())
}
