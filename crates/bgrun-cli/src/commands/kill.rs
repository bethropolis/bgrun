use anyhow::Result;
use bgrun_proto::{Command, KillArgs};
use inquire::Confirm;
use std::io::IsTerminal;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::output::output_mode;

/// Kills a job or all jobs in a workspace.
pub async fn kill(
    id: Option<String>,
    workspace: Option<String>,
    force: bool,
    json: bool,
) -> Result<()> {
    if id.is_none() && workspace.is_none() {
        anyhow::bail!("either --id or --workspace must be specified");
    }

    // Confirmation is skipped with --force or when stdin isn't a TTY (CI,
    // scripts, AI agents) — prompting there would crash with NotTTY.
    let interactive = std::io::stdin().is_terminal();
    if workspace.is_some()
        && !force
        && interactive
        && output_mode(json) == crate::output::OutputMode::Human
    {
        let confirm = Confirm::new("Kill all jobs in this workspace?")
            .with_default(false)
            .prompt()?;
        if !confirm {
            println!("Aborted.");
            return Ok(());
        }
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
        let err = response.error.unwrap_or_default();
        anyhow::bail!("kill: {err}");
    }

    if output_mode(json) == crate::output::OutputMode::Json {
        if let Some(val) = response.data {
            println!("{}", serde_json::to_string(&val)?);
        }
    } else if let Some(id) = id {
        println!("Killed job {id}");
    } else {
        println!("Killed");
    }

    Ok(())
}
