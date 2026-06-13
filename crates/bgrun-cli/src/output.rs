use anyhow::Result;
use bgrun_proto::{JobRecord, JobStatus};
use crossterm::style::Stylize;
use std::io::IsTerminal;

/// Output mode for rendering command results.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputMode {
    Human,
    Json,
}

/// Returns the output mode. Defaults to Human; callers pass `--json` flag to force Json.
pub fn output_mode(force_json: bool) -> OutputMode {
    if force_json {
        OutputMode::Json
    } else {
        OutputMode::Human
    }
}

fn use_color() -> bool {
    std::io::stdout().is_terminal()
}

fn color_state(s: &str) -> String {
    match s {
        "Ready" | "Exited" => s.green().to_string(),
        "Running" => s.yellow().to_string(),
        "Crashed" | "Killed" => s.red().to_string(),
        _ => s.to_string(),
    }
}

fn maybe_bold(label: &str) -> String {
    if use_color() {
        label.bold().to_string()
    } else {
        label.to_string()
    }
}

fn maybe_color_state(s: &str) -> String {
    if use_color() {
        color_state(s)
    } else {
        s.to_string()
    }
}

/// Prints a single job record.
pub fn print_job(record: &JobRecord, mode: OutputMode) -> Result<()> {
    match mode {
        OutputMode::Human => {
            let b = |s: &str| maybe_bold(s);
            println!("{}:       {}", b("ID"), record.id);
            println!("{}:     {}", b("Name"), record.name.as_deref().unwrap_or("-"));
            println!("{}:  {}", b("Command"), record.cmd.join(" "));
            println!(
                "{}:      {}",
                b("Pid"),
                record.pid.map_or("-".into(), |p| p.to_string())
            );
            println!("{}:    {}", b("State"), maybe_color_state(&record.state.to_string()));
            println!("{}:  {}", b("Started"), record.started_at);
            println!("{}: {}", b("Workspace"), record.workspace.as_deref().unwrap_or("-"));
            if let Some(port) = record.allocated_port {
                println!("{}:      {}", b("Port"), port);
            }
            if record.health_check.is_some() {
                println!("{}:    enabled (interval={}s, threshold={})",
                    b("Health"),
                    record.health_interval_secs.unwrap_or(10),
                    record.health_threshold.unwrap_or(3));
            }
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string(record)?);
        }
    }
    Ok(())
}

/// Prints a list of job records.
pub fn print_jobs(records: &[JobRecord], mode: OutputMode) -> Result<()> {
    match mode {
        OutputMode::Human => {
            if records.is_empty() {
                println!("No jobs.");
                return Ok(());
            }
            let color = use_color();
            if color {
                println!(
                    "{} {:<22} {} {:<12} {} {:<8} {} {:<5} {} {:<8} {}",
                    "ID".bold(), "",
                    "NAME".bold(), "",
                    "STATE".bold(), "",
                    "PID".bold(), "",
                    "PORT".bold(), "",
                    "COMMAND".bold(),
                );
                let rule: String = std::iter::repeat('\u{2500}').take(90).collect();
                println!("{}", rule.dim());
            } else {
                println!(
                    "  {:<22} {:<12} {:<8} {:<5} {:<8} COMMAND",
                    "ID", "NAME", "STATE", "PID", "PORT"
                );
                println!("  {}", "-".repeat(85));
            }
            for record in records {
                let id_short = if record.id.len() > 8 {
                    &record.id[..8]
                } else {
                    &record.id
                };
                let state = if color {
                    color_state(&record.state.to_string())
                } else {
                    record.state.to_string()
                };
                println!(
                    "  {:<22} {:<12} {:<8} {:<5} {:<8} {}",
                    id_short,
                    record.name.as_deref().unwrap_or("-"),
                    state,
                    record.pid.map_or("-".into(), |p| p.to_string()),
                    record.allocated_port.map_or("-".into(), |p| p.to_string()),
                    record.cmd.join(" "),
                );
            }
        }
        OutputMode::Json => {
            for record in records {
                println!("{}", serde_json::to_string(record)?);
            }
        }
    }
    Ok(())
}

/// Prints job status information.
pub fn print_status(status: &JobStatus, mode: OutputMode) -> Result<()> {
    match mode {
        OutputMode::Human => {
            let b = |s: &str| maybe_bold(s);
            println!("{}:      {}", b("State"), maybe_color_state(&status.state.to_string()));
            println!(
                "{}:  {}",
                b("Exit Code"),
                status.exit_code.map_or("-".into(), |c| c.to_string())
            );
            println!("{}:   {}", b("Ready At"), status.ready_at.as_deref().unwrap_or("-"));
            println!("{}:   {}", b("Restarts"), status.restart_count);
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string(status)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_mode_values_are_distinct() {
        assert_ne!(OutputMode::Human, OutputMode::Json);
    }
}
