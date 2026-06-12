use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use std::io::IsTerminal;
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
    /// Output in JSON format (default: human-readable)
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Commands>,
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

        /// Max RSS in MB before the job is killed
        #[arg(long)]
        max_rss: Option<u64>,

        /// Max runtime before the job is killed (e.g. "30s", "5m")
        #[arg(long)]
        max_runtime: Option<String>,

        /// Allocate a free port and set it as the given env var name (e.g. "PORT")
        #[arg(long)]
        allocate_port: Option<String>,

        /// Health check URL to poll (e.g. http://localhost:8080/health)
        #[arg(long)]
        health_check_url: Option<String>,

        /// Health check TCP port to probe
        #[arg(long)]
        health_check_port: Option<u16>,

        /// Health check polling interval in seconds (default 10)
        #[arg(long)]
        health_interval: Option<u64>,

        /// Consecutive health check failures before killing (default 3)
        #[arg(long)]
        health_threshold: Option<u32>,
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

        /// Filter by stream source (stdout, stderr, pty)
        #[arg(long)]
        stream: Option<String>,

        /// Strip ANSI escape codes from output
        #[arg(long)]
        strip_ansi: bool,

        /// Follow new log lines in real time (polls every 200ms)
        #[arg(long)]
        follow: bool,

        /// Filter log lines by regex pattern
        #[arg(long)]
        filter_regex: Option<String>,
    },

    /// Show log lines since the last diff call
    Diff {
        /// Job ID
        id: String,

        /// Number of lines to show (unlimited if not set)
        #[arg(long)]
        lines: Option<usize>,

        /// Filter by stream source (stdout, stderr, pty)
        #[arg(long)]
        stream: Option<String>,

        /// Strip ANSI escape codes from output
        #[arg(long)]
        strip_ansi: bool,

        /// Filter log lines by regex pattern
        #[arg(long)]
        filter_regex: Option<String>,
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

    /// Print JSON Schema for a command's arguments
    Schema {
        /// Command name (run, kill, tail, or leave blank for unified enum)
        command: Option<String>,
    },

    /// Hidden completion utility used by shell extensions
    #[command(hide = true)]
    Completions {
        /// Print active short IDs with state descriptions
        #[arg(long)]
        active_ids: bool,

        /// Print unique active workspaces
        #[arg(long)]
        workspaces: bool,

        /// Generate completion script for a shell (fish, bash, zsh)
        #[arg(long)]
        shell: Option<String>,

        /// Generate and print the CLI man page (troff format)
        #[arg(long)]
        man: bool,
    },

    /// Remove all terminated (crashed/exited/killed) jobs
    Clean {
        /// Only clean jobs in this workspace
        #[arg(long)]
        workspace: Option<String>,
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
    let json = cli.json;

    match cli.command {
        Some(Commands::Run {
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
            max_rss,
            max_runtime,
            allocate_port,
            health_check_url,
            health_check_port,
            health_interval,
            health_threshold,
        }) => {
            let max_runtime_ms = max_runtime
                .as_ref()
                .and_then(|s| commands::run::parse_duration_ms(s));
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
                max_rss_mb: max_rss,
                max_runtime_ms,
                allocate_port,
                health_check_url,
                health_check_port,
                health_interval,
                health_threshold,
            };
            commands::run::run(cmd, name, workspace, flags, json).await?;
        }
        Some(Commands::List { workspace }) => {
            commands::list::list(workspace, json).await?;
        }
        Some(Commands::Status { id }) => {
            commands::status::status(id, json).await?;
        }
        Some(Commands::Kill { id, workspace }) => {
            commands::kill::kill(id, workspace, json).await?;
        }
        Some(Commands::Wait { id, timeout }) => {
            commands::wait::wait(id, timeout, json).await?;
        }
        Some(Commands::Tail {
            id,
            lines,
            digest,
            level,
            stream,
            strip_ansi,
            follow,
            filter_regex,
        }) => {
            commands::tail::tail(id, lines, digest, level, stream, strip_ansi, follow, filter_regex, json).await?;
        }
        Some(Commands::Diff {
            id,
            lines,
            stream,
            strip_ansi,
            filter_regex,
        }) => {
            commands::diff::diff(id, lines, stream, strip_ansi, filter_regex, json).await?;
        }
        Some(Commands::RunGroup { names }) => {
            commands::run_group::run_group(names, json).await?;
        }
        Some(Commands::Send { id, data }) => {
            commands::send::send(id, data, json).await?;
        }
        Some(Commands::Stats { id }) => {
            commands::stats::stats(id, json).await?;
        }
        Some(Commands::Attach { id }) => {
            commands::attach::attach_job(id).await?;
        }
        Some(Commands::Expect {
            id,
            pattern,
            regex,
            timeout,
        }) => {
            commands::expect::expect(id, pattern, regex, timeout, json).await?;
        }
        Some(Commands::Schema { command }) => {
            commands::schema::print_schema(command.as_deref())?;
        }
        Some(Commands::Completions { active_ids, workspaces, shell, man }) => {
            commands::completions::completions(active_ids, workspaces, shell, man).await?;
        }
        Some(Commands::Clean { workspace }) => {
            commands::clean::clean(workspace, json).await?;
        }
        Some(Commands::Skill { command }) => match command {
            SkillCommands::Install { path } => {
                commands::skill::install(path)?;
            }
        },
        None => {
            if std::io::stdout().is_terminal() {
                commands::interactive::start_menu().await?;
            } else {
                // Non-interactive: show help
                let mut cmd = Cli::command();
                cmd.print_help()?;
                println!();
            }
        }
    }

    Ok(())
}
