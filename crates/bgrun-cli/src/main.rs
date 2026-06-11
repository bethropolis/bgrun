use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod autostart;
mod client;
pub mod commands;
pub mod output;

#[derive(Parser)]
#[command(
    name = "bgrun",
    about = "Background process runner for AI agent workflows"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a command in the background
    Run {
        /// Command to run (trailing arguments)
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,

        /// Optional name for the job
        #[arg(long)]
        name: Option<String>,

        /// Optional workspace tag
        #[arg(long)]
        workspace: Option<String>,

        /// Readiness: match a log pattern (substring)
        #[arg(long)]
        ready_when: Option<String>,

        /// Readiness: match a log pattern (regex)
        #[arg(long)]
        ready_when_regex: Option<String>,

        /// Readiness: poll a TCP port
        #[arg(long)]
        ready_when_port: Option<u16>,

        /// Readiness: poll an HTTP URL (2xx = ready)
        #[arg(long)]
        ready_when_url: Option<String>,

        /// Readiness: wait for a file to exist
        #[arg(long)]
        ready_when_file: Option<String>,

        /// Start after a named job is ready
        #[arg(long)]
        after: Option<String>,

        /// Allocate a pseudo-terminal for the child process
        #[arg(long)]
        pty: bool,

        /// Restart policy: "on-crash"
        #[arg(long)]
        restart: Option<String>,

        /// Backoff duration for restart (e.g. "2s", "5m")
        #[arg(long)]
        backoff: Option<String>,

        /// PTY columns (default 80)
        #[arg(long)]
        cols: Option<u16>,

        /// PTY rows (default 24)
        #[arg(long)]
        rows: Option<u16>,
    },

    /// List running jobs
    List {
        /// Filter by workspace
        #[arg(long)]
        workspace: Option<String>,
    },

    /// Get status of a job
    Status {
        /// Job ID
        id: String,
    },

    /// Kill a job
    Kill {
        /// Job ID
        id: Option<String>,

        /// Kill all jobs in a workspace
        #[arg(long)]
        workspace: Option<String>,
    },

    /// Wait for a job to become ready
    Wait {
        /// Job ID
        id: String,

        /// Timeout (e.g. "5s", "30s", "2m", default "60s")
        #[arg(long, default_value = "60s")]
        timeout: String,
    },

    /// Show the last N lines of a job's log
    Tail {
        /// Job ID
        id: String,

        /// Number of lines to show
        #[arg(long, default_value_t = 20)]
        lines: usize,

        /// Show digest summary instead of raw lines
        #[arg(long)]
        digest: bool,

        /// Filter by level (e.g. "error", "warn")
        #[arg(long)]
        level: Option<String>,

        /// Strip ANSI escape codes from output
        #[arg(long)]
        strip_ansi: bool,
    },

    /// Show log lines since the last diff call
    Diff {
        /// Job ID
        id: String,

        /// Number of lines to show (unlimited if not set)
        #[arg(long)]
        lines: Option<usize>,

        /// Strip ANSI escape codes from output
        #[arg(long)]
        strip_ansi: bool,
    },

    /// Run multiple named jobs in parallel
    RunGroup {
        /// Job names to run
        #[arg(required = true)]
        names: Vec<String>,
    },

    /// Send data to a job's stdin
    Send {
        /// Job ID
        id: String,

        /// Data to send
        data: String,
    },

    /// Show resource stats for a running job
    Stats {
        /// Job ID
        id: String,
    },

    /// Wait for a pattern in a job's log output
    Expect {
        /// Job ID
        id: String,

        /// Pattern to wait for
        pattern: String,

        /// Treat pattern as a regex
        #[arg(long)]
        regex: bool,

        /// Timeout (e.g. "5s", "30s", "2m", default "60s")
        #[arg(long, default_value = "60s")]
        timeout: String,
    },

    /// Attach to a PTY job's interactive terminal
    Attach {
        /// Job ID
        id: String,
    },

    /// Manage embedded skills
    Skill {
        #[command(subcommand)]
        command: SkillCommands,
    },
}

#[derive(Subcommand)]
enum SkillCommands {
    /// Install the embedded skill to a target directory
    Install {
        /// Target directory (e.g. ~/.config/opencode/skills/bgrun)
        path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            cmd,
            name,
            workspace,
            ready_when,
            ready_when_regex,
            ready_when_port,
            ready_when_url,
            ready_when_file,
            after,
            pty,
            restart,
            backoff,
            cols,
            rows,
        } => {
            let flags = commands::run::RunFlags {
                ready_when,
                ready_when_regex,
                ready_when_port,
                ready_when_url,
                ready_when_file,
                after,
                pty,
                restart,
                backoff,
                pty_cols: cols,
                pty_rows: rows,
            };
            commands::run::run(cmd, name, workspace, flags).await?;
        }
        Commands::List { workspace } => {
            commands::list::list(workspace).await?;
        }
        Commands::Status { id } => {
            commands::status::status(id).await?;
        }
        Commands::Kill { id, workspace } => {
            commands::kill::kill(id, workspace).await?;
        }
        Commands::Wait { id, timeout } => {
            commands::wait::wait(id, timeout).await?;
        }
        Commands::Tail {
            id,
            lines,
            digest,
            level,
            strip_ansi,
        } => {
            commands::tail::tail(id, lines, digest, level, strip_ansi).await?;
        }
        Commands::Diff {
            id,
            lines,
            strip_ansi,
        } => {
            commands::diff::diff(id, lines, strip_ansi).await?;
        }
        Commands::RunGroup { names } => {
            commands::run_group::run_group(names).await?;
        }
        Commands::Send { id, data } => {
            commands::send::send(id, data).await?;
        }
        Commands::Stats { id } => {
            commands::stats::stats(id).await?;
        }
        Commands::Attach { id } => {
            commands::attach::attach_job(id).await?;
        }
        Commands::Expect {
            id,
            pattern,
            regex,
            timeout,
        } => {
            commands::expect::expect(id, pattern, regex, timeout).await?;
        }
        Commands::Skill { command } => match command {
            SkillCommands::Install { path } => {
                commands::skill::install(path)?;
            }
        },
    }

    Ok(())
}
