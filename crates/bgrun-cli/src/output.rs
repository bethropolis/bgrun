use anyhow::Result;
use bgrun_proto::{JobRecord, JobStatus};

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
            if let Some(port) = record.allocated_port {
                println!("Port:      {}", port);
            }
            if record.health_check.is_some() {
                println!("Health:    enabled (interval={}s, threshold={})",
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
            println!(
                "{:<24} {:<12} {:<8} {:<6} {:<8} COMMAND",
                "ID", "NAME", "STATE", "PID", "PORT"
            );
            println!("{}", "-".repeat(88));
            for record in records {
                let id_short = if record.id.len() > 8 {
                    &record.id[..8]
                } else {
                    &record.id
                };
                println!(
                    "{:<24} {:<12} {:<8} {:<6} {:<8} {}",
                    id_short,
                    record.name.as_deref().unwrap_or("-"),
                    record.state.to_string(),
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
