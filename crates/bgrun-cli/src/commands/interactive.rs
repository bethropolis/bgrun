use anyhow::Result;
use bgrun_proto::{Command, JobRecord};
use inquire::Select;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;

pub async fn start_menu() -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    loop {
        let options = vec![
            "List & Refresh Jobs",
            "View Job Status/Stats",
            "Attach to Interactive PTY",
            "Tail Job Logs",
            "Kill a Job",
            "Exit Menu",
        ];

        let select = Select::new("Select a bgrun action:", options).prompt();
        match select {
            Ok("List & Refresh Jobs") => {
                let _ = crate::commands::list::list(None, false).await;
            }
            Ok("View Job Status/Stats") => {
                if let Some(id) = select_active_job().await? {
                    let _ = crate::commands::status::status(id.clone(), false).await;
                    let _ = crate::commands::stats::stats(id, false).await;
                }
            }
            Ok("Attach to Interactive PTY") => {
                if let Some(id) = select_active_job().await? {
                    let _ = crate::commands::attach::attach_job(id, false).await;
                }
            }
            Ok("Tail Job Logs") => {
                if let Some(id) = select_active_job().await? {
                    let _ = crate::commands::tail::tail(id, 20, false, None, None, false, false, None, false).await;
                }
            }
            Ok("Kill a Job") => {
                if let Some(id) = select_active_job().await? {
                    let _ = crate::commands::kill::kill(Some(id), None, false).await;
                }
            }
            Ok("Exit Menu") | Err(_) => break,
            _ => {}
        }
        println!();
    }

    Ok(())
}

async fn select_active_job() -> Result<Option<String>> {
    let socket_path = bgrun_proto::paths::socket_path();
    let mut client = DaemonClient::connect(&socket_path).await?;

    let response = client
        .send::<Vec<JobRecord>>(Command::List { workspace: None })
        .await?;

    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }

    let records = response.data.unwrap_or_default();
    if records.is_empty() {
        println!("No active background jobs found.");
        return Ok(None);
    }

    let options: Vec<String> = records
        .iter()
        .map(|r| {
            let id_short = if r.id.len() > 8 { &r.id[..8] } else { &r.id };
            format!(
                "{} | {} [{}] | {}",
                id_short,
                r.name.as_deref().unwrap_or("unnamed"),
                r.state,
                r.cmd.join(" ")
            )
        })
        .collect();

    let ans = Select::new("Choose a process:", options).prompt();
    match ans {
        Ok(choice) => {
            let id = choice.split('|').next().unwrap_or_default().trim().to_string();
            Ok(Some(id))
        }
        Err(_) => Ok(None),
    }
}
