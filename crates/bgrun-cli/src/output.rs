use anyhow::Result;
use bgrun_proto::{JobRecord, JobStatus};

/// Output mode for rendering command results.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputMode {
    Human,
    Json,
}

/// Detects the output mode based on whether stdout is a terminal.
pub fn output_mode() -> OutputMode {
    if is_terminal::IsTerminal::is_terminal(&std::io::stdout()) {
        OutputMode::Human
    } else {
        OutputMode::Json
    }
}

/// Prints a single job record.
pub fn print_job(record: &JobRecord, mode: OutputMode) -> Result<()> {
    match mode {
        OutputMode::Human => {
            println!("ID:       {}", record.id);
            println!("Name:     {}", record.name.as_deref().unwrap_or("-"));
            println!("Command:  {}", record.cmd.join(" "));
            println!(
                "Pid:      {}",
                record.pid.map_or("-".into(), |p| p.to_string())
            );
            println!("State:    {}", record.state);
            println!("Started:  {}", record.started_at);
            println!("Workspace: {}", record.workspace.as_deref().unwrap_or("-"));
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
            println!(
                "{:<24} {:<12} {:<8} {:<6} COMMAND",
                "ID", "NAME", "STATE", "PID"
            );
            println!("{}", "-".repeat(80));
            for record in records {
                let id_short = if record.id.len() > 8 {
                    &record.id[..8]
                } else {
                    &record.id
                };
                println!(
                    "{:<24} {:<12} {:<8} {:<6} {}",
                    id_short,
                    record.name.as_deref().unwrap_or("-"),
                    record.state.to_string(),
                    record.pid.map_or("-".into(), |p| p.to_string()),
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
            println!("State:      {}", status.state);
            println!(
                "Exit Code:  {}",
                status.exit_code.map_or("-".into(), |c| c.to_string())
            );
            println!("Ready At:   {}", status.ready_at.as_deref().unwrap_or("-"));
            println!("Restarts:   {}", status.restart_count);
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
