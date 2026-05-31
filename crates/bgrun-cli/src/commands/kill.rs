use anyhow::Result;
use bgrun_proto::{Command, KillArgs};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::output_mode;

/// Kills a job or all jobs in a workspace.
pub async fn kill(id: Option<String>, workspace: Option<String>) -> Result<()> {
    if id.is_none() && workspace.is_none() {
        anyhow::bail!("either --id or --workspace must be specified");
    }

    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    let args = KillArgs {
        id: id.clone(),
        workspace,
    };
    let response = client
        .send::<serde_json::Value>(Command::Kill(args))
        .await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    if output_mode() == crate::output::OutputMode::Json {
        if let Some(val) = response.data {
            println!("{}", serde_json::to_string(&val)?);
        }
    } else {
        if let Some(id) = id {
            println!("Killed job {id}");
        } else {
            println!("Killed");
        }
    }

    Ok(())
}
