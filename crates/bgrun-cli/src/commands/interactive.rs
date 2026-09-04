use anyhow::Result;
use bgrun_proto::{Command, JobRecord};
use inquire::{Confirm, CustomType, Select, Text};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;

// ── entry point ─────────────────────────────────────────────────────────────

pub async fn start_menu() -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    println!("bgrun interactive mode (ESC goes back, Ctrl+C exits)\n");

    // Optional workspace scope for the whole session.
    let mut workspace: Option<String> = match Text::new("Workspace filter (blank for all):")
        .with_help_message("Scope job browsing to one workspace; blank shows everything")
        .prompt_skippable()?
    {
        Some(w) if !w.trim().is_empty() => Some(w.trim().to_string()),
        _ => None,
    };

    loop {
        let records = fetch_jobs(workspace.clone()).await?;
        render_dashboard(&records, workspace.as_deref());

        let options = vec![
            "Browse jobs (pick one to manage)",
            "Start a new job",
            "Clean terminated jobs",
            "Change workspace filter",
            "Exit",
        ];

        let Some(choice) = Select::new("What next?", options)
            .with_page_size(8)
            .with_help_message("↑↓ navigate · Enter select · ESC back")
            .prompt_skippable()?
        else {
            break;
        };

        match choice {
            "Browse jobs (pick one to manage)" => {
                if let Some(record) = pick_job(&records)? {
                    job_menu(record).await?;
                }
            }
            "Start a new job" => {
                start_job_flow(workspace.clone()).await?;
            }
            "Clean terminated jobs" => {
                // force=true: already confirmed via menu intent; keep the
                // daemon-side confirm out of the way for a smooth flow.
                let _ = crate::commands::clean::clean(workspace.clone(), false, true).await;
            }
            "Change workspace filter" => {
                workspace = match Text::new("Workspace filter (blank for all):")
                    .prompt_skippable()?
                {
                    Some(w) if !w.trim().is_empty() => Some(w.trim().to_string()),
                    _ => None,
                };
            }
            _ => break,
        }
        println!();
    }

    println!("Bye!");
    Ok(())
}

// ── dashboard ───────────────────────────────────────────────────────────────

async fn fetch_jobs(workspace: Option<String>) -> Result<Vec<JobRecord>> {
    let socket_path = bgrun_proto::paths::socket_path();
    let mut client = DaemonClient::connect(&socket_path).await?;
    let response = client
        .send::<Vec<JobRecord>>(Command::List { workspace })
        .await?;
    if !response.ok {
        anyhow::bail!("{}", response.error.unwrap_or_default());
    }
    Ok(response.data.unwrap_or_default())
}

fn render_dashboard(records: &[JobRecord], workspace: Option<&str>) {
    let alive = records
        .iter()
        .filter(|r| r.state.to_string() != "Exited" && r.state.to_string() != "Crashed" && r.state.to_string() != "Killed")
        .count();
    let terminal = records.len().saturating_sub(alive);
    let scope = workspace.unwrap_or("all workspaces");
    println!("── {scope}: {} job(s) · {alive} alive · {terminal} terminated ──", records.len());
    if records.is_empty() {
        println!("No jobs here yet — start one from the menu below.");
    }
}

// ── job picking (index-mapped, no string splitting) ─────────────────────────

fn job_label(r: &JobRecord) -> String {
    let id_short = if r.id.len() > 8 { &r.id[..8] } else { &r.id };
    format!(
        "{} | {} [{}] | {}",
        id_short,
        r.name.as_deref().unwrap_or("unnamed"),
        r.state,
        r.cmd.join(" ")
    )
}

fn pick_job(records: &[JobRecord]) -> Result<Option<JobRecord>> {
    if records.is_empty() {
        println!("No jobs to pick from.");
        return Ok(None);
    }
    let labels: Vec<String> = records.iter().map(job_label).collect();
    let ans = Select::new("Choose a job:", labels)
        .with_page_size(10)
        .with_help_message("Type to filter · ↑↓ navigate · ESC back")
        .prompt_skippable()?;
    match ans {
        Some(choice) => {
            let idx = records.iter().position(|r| job_label(r) == choice);
            Ok(idx.map(|i| records[i].clone()))
        }
        None => Ok(None),
    }
}

// ── per-job submenu ─────────────────────────────────────────────────────────

async fn job_menu(record: JobRecord) -> Result<()> {
    let id = record.id.clone();
    let name = record.name.clone().unwrap_or_else(|| id.clone());
    loop {
        println!("\n── job: {name} [{}] ──", record.state);
        let options = vec![
            "Status + resource stats",
            "Tail logs",
            "Screen buffer (non-blocking peek)",
            "Diff (only new lines)",
            "Send stdin",
            "Wait until ready",
            "Expect log pattern",
            "Attach to PTY",
            "Stop job (graceful)",
            "Restart job (same definition)",
            "Kill job",
            "← Back to main menu",
        ];
        let Some(choice) = Select::new("Job action:", options)
            .with_page_size(12)
            .with_help_message("ESC back to main menu")
            .prompt_skippable()?
        else {
            break;
        };
        match choice {
            "Status + resource stats" => {
                let _ = crate::commands::status::status(id.clone(), false).await;
                let _ = crate::commands::stats::stats(id.clone(), false).await;
            }
            "Tail logs" => tail_flow(&id).await?,
            "Screen buffer (non-blocking peek)" => {
                let lines = prompt_lines(20)?;
                let _ = crate::commands::screen::screen(id.clone(), lines, false).await;
            }
            "Diff (only new lines)" => {
                let _ = crate::commands::diff::diff(id.clone(), None, None, false, None, false).await;
            }
            "Send stdin" => send_flow(&id).await?,
            "Wait until ready" => wait_flow(&id).await?,
            "Expect log pattern" => expect_flow(&id).await?,
            "Attach to PTY" => {
                let _ = crate::commands::attach::attach_job(id.clone(), false).await;
            }
            "Stop job (graceful)" => {
                let _ = crate::commands::stop::stop(id.clone(), "10s".into(), false).await;
                break;
            }
            "Restart job (same definition)" => {
                let confirm = Confirm::new(&format!("Restart job {name}?"))
                    .with_default(false)
                    .prompt_skippable()?;
                if confirm.unwrap_or(false) {
                    let _ =
                        crate::commands::restart::restart(id.clone(), "10s".into(), false).await;
                    // The restarted job has a new UUID; go back for a fresh list.
                    break;
                }
            }
            "Kill job" => {
                let confirm = Confirm::new(&format!("Kill job {name}?"))
                    .with_default(false)
                    .prompt_skippable()?;
                if confirm.unwrap_or(false) {
                    let _ = crate::commands::kill::kill(Some(id.clone()), None, false).await;
                    break;
                }
            }
            _ => break,
        }
        pause()?;
    }
    Ok(())
}

// ── prompt helpers ──────────────────────────────────────────────────────────

fn prompt_lines(default: usize) -> Result<usize> {
    match CustomType::<usize>::new("Lines:")
        .with_default(default)
        .with_help_message("How many log lines to show")
        .prompt_skippable()?
    {
        Some(n) => Ok(n),
        None => Ok(default),
    }
}

fn prompt_optional(message: &str, help: &str) -> Result<Option<String>> {
    match Text::new(message)
        .with_help_message(help)
        .prompt_skippable()?
    {
        Some(s) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        _ => Ok(None),
    }
}

fn pause() -> Result<()> {
    let _ = Text::new("Press Enter to continue…")
        .with_help_message("Enter continues · ESC goes back")
        .prompt_skippable()?;
    Ok(())
}

// ── flows ───────────────────────────────────────────────────────────────────

async fn tail_flow(id: &str) -> Result<()> {
    let lines = prompt_lines(20)?;
    let digest = Confirm::new("Show digest summary instead of raw lines?")
        .with_default(false)
        .prompt_skippable()?
        .unwrap_or(false);
    let level = prompt_optional(
        "Level filter (error/warn, blank for none):",
        "Only show lines at this level",
    )?;
    let _ = crate::commands::tail::tail(
        id.to_string(),
        lines,
        digest,
        level,
        None,
        false,
        false,
        None,
        false,
    )
    .await;
    Ok(())
}

async fn send_flow(id: &str) -> Result<()> {
    let Some(data) = Text::new("Text to send (blank for just Enter):")
        .prompt_skippable()?
    else {
        return Ok(());
    };
    let payload = if data.is_empty() {
        "\n".to_string()
    } else {
        let with_enter = Confirm::new("Append Enter (newline)?")
            .with_default(true)
            .prompt_skippable()?
            .unwrap_or(true);
        if with_enter {
            format!("{data}\n")
        } else {
            data
        }
    };
    let _ = crate::commands::send::send(id.to_string(), payload, false).await;
    Ok(())
}

async fn wait_flow(id: &str) -> Result<()> {
    let timeout = match Text::new("Timeout:")
        .with_default("60s")
        .with_help_message("e.g. 30s, 5m")
        .prompt_skippable()?
    {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => "60s".to_string(),
    };
    let _ = crate::commands::wait::wait(id.to_string(), timeout, false).await;
    Ok(())
}

async fn expect_flow(id: &str) -> Result<()> {
    let pattern = match Text::new("Pattern to wait for:")
        .prompt_skippable()?
    {
        Some(p) if !p.trim().is_empty() => p.trim().to_string(),
        _ => return Ok(()),
    };
    let is_regex = Confirm::new("Treat as regex?")
        .with_default(false)
        .prompt_skippable()?
        .unwrap_or(false);
    let timeout = match Text::new("Timeout:")
        .with_default("60s")
        .prompt_skippable()?
    {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => "60s".to_string(),
    };
    let _ = crate::commands::expect::expect(id.to_string(), pattern, is_regex, timeout, false).await;
    Ok(())
}

async fn start_job_flow(workspace: Option<String>) -> Result<()> {
    let cmd = match Text::new("Command to run:")
        .with_help_message("e.g. npm run dev · cargo run · python worker.py")
        .prompt_skippable()?
    {
        Some(c) if !c.trim().is_empty() => c.trim().to_string(),
        _ => {
            println!("Aborted: a command is required.");
            return Ok(());
        }
    };

    let name = prompt_optional("Job name (blank for none):", "Named jobs are idempotent")?;
    let ready = prompt_optional(
        "Ready pattern (blank for none):",
        "Job becomes Ready when a log line contains this",
    )?;
    let env_raw = prompt_optional(
        "Env vars, comma-separated KEY=VAL (blank for none):",
        "e.g. PORT=3000,RUST_LOG=debug",
    )?;
    let env: Vec<String> = env_raw
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let wait = Confirm::new("Wait until Ready after starting?")
        .with_default(ready.is_some())
        .prompt_skippable()?
        .unwrap_or(false);
    let wait_timeout = if wait {
        match Text::new("Wait timeout:")
            .with_default("60s")
            .prompt_skippable()?
        {
            Some(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => "60s".to_string(),
        }
    } else {
        "60s".to_string()
    };

    let flags = crate::commands::run::RunFlags {
        ready_when: ready,
        ready_when_regex: None,
        ready_when_port: None,
        ready_when_url: None,
        ready_when_file: None,
        after: None,
        pty: false,
        restart: None,
        backoff: None,
        max_retries: None,
        pty_cols: None,
        pty_rows: None,
        max_rss_mb: None,
        max_runtime_ms: None,
        allocate_port: None,
        health_check_url: None,
        health_check_port: None,
        health_interval: None,
        health_threshold: None,
        env,
        cwd: None,
        replace: false,
        wait,
        wait_timeout,
    };

    if let Err(e) = crate::commands::run::run(vec![cmd], name, workspace, flags, false).await {
        println!("Failed to start job: {e:#}");
    }
    Ok(())
}
