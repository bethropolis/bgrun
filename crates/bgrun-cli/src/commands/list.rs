use anyhow::Result;
use bgrun_proto::{Command, JobRecord};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::{output_mode, print_jobs};

/// Lists all jobs, optionally filtered by workspace.
pub async fn list(workspace: Option<String>) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let response = client
        .send::<Vec<JobRecord>>(Command::List { workspace })
        .await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    let records = response.data.unwrap_or_default();
    print_jobs(&records, output_mode())?;

    Ok(())
}
