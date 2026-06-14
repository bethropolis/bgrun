use anyhow::Result;
use bgrun_proto::{Command, JobRecord};

use crate::client::DaemonClient;

/// Handles the `bgrun completions` subcommand.
pub async fn completions(
    active_ids: bool,
    workspaces: bool,
    shell: Option<String>,
    man: bool,
) -> Result<()> {
    if man {
        print_man_page()?;
        return Ok(());
    }

    if let Some(shell_name) = shell {
        match shell_name.as_str() {
            "fish" => print_fish_completions(),
            "bash" => print_bash_completions(),
            "zsh" => print_zsh_completions(),
            other => anyhow::bail!("unsupported shell: '{}'. Supported: fish, bash, zsh", other),
        }
        return Ok(());
    }

    // Dynamic data generation (original behavior)
    let socket_path = bgrun_proto::paths::socket_path();
    if let Ok(mut client) = DaemonClient::connect(&socket_path).await {
        if let Ok(response) = client
            .send::<Vec<JobRecord>>(Command::List { workspace: None })
            .await
        {
            if let Some(records) = response.data {
                if active_ids {
                    for r in &records {
                        let id_short = if r.id.len() > 8 { &r.id[..8] } else { &r.id };
                        let name = r.name.as_deref().unwrap_or("unnamed");
                        println!("{}\t{} ({})", id_short, name, r.state);
                    }
                } else if workspaces {
                    let mut unique_ws = std::collections::HashSet::new();
                    for r in &records {
                        if let Some(ref ws) = r.workspace {
                            unique_ws.insert(ws.clone());
                        }
                    }
                    for ws in unique_ws {
                        println!("{}", ws);
                    }
                }
            }
        }
    }

    Ok(())
}

fn print_man_page() -> Result<()> {
    use clap::CommandFactory;
    use std::io::Write;

    let cmd = crate::Cli::command();
    let man = clap_mangen::Man::new(cmd);
    let mut buf = Vec::new();
    man.render(&mut buf)?;
    std::io::stdout().write_all(&buf)?;
    Ok(())
}

fn print_fish_completions() {
    println!(r#"# bgrun completions for fish shell
# Install: bgrun completions --shell fish > ~/.config/fish/completions/bgrun.fish

complete -c bgrun -f

# Commands
complete -c bgrun -n "__fish_use_subcommand" -a "run" -d "Run a command in the background"
complete -c bgrun -n "__fish_use_subcommand" -a "list" -d "List running jobs"
complete -c bgrun -n "__fish_use_subcommand" -a "status" -d "Get status of a job"
complete -c bgrun -n "__fish_use_subcommand" -a "kill" -d "Kill a job"
complete -c bgrun -n "__fish_use_subcommand" -a "wait" -d "Wait for a job to become ready"
complete -c bgrun -n "__fish_use_subcommand" -a "tail" -d "Show the last N lines of a job's log"
complete -c bgrun -n "__fish_use_subcommand" -a "diff" -d "Show log lines since the last diff call"
complete -c bgrun -n "__fish_use_subcommand" -a "run-group" -d "Run multiple named jobs in parallel"
complete -c bgrun -n "__fish_use_subcommand" -a "send" -d "Send data to a job's stdin"
complete -c bgrun -n "__fish_use_subcommand" -a "stats" -d "Show resource stats for a running job"
complete -c bgrun -n "__fish_use_subcommand" -a "expect" -d "Wait for a pattern in a job's log output"
complete -c bgrun -n "__fish_use_subcommand" -a "attach" -d "Attach to a PTY job's interactive terminal"
complete -c bgrun -n "__fish_use_subcommand" -a "screen" -d "Show last N lines from in-memory buffer"
complete -c bgrun -n "__fish_use_subcommand" -a "schema" -d "Print JSON Schema for a command's arguments"
complete -c bgrun -n "__fish_use_subcommand" -a "clean" -d "Remove all terminated jobs"
complete -c bgrun -n "__fish_use_subcommand" -a "skill" -d "Manage embedded skills"
complete -c bgrun -n "__fish_use_subcommand" -a "help" -d "Print help"

# Dynamic job IDs for commands that accept a job ID
complete -c bgrun -n "__fish_seen_subcommand_from status kill wait tail diff send stats attach expect screen" -a "(bgrun completions --active-ids)"

# Dynamic workspaces for list, kill, and clean
complete -c bgrun -n "__fish_seen_subcommand_from list kill; and __fish_prev_arg_in --workspace" -a "(bgrun completions --workspaces)"
complete -c bgrun -n "__fish_seen_subcommand_from clean; and __fish_prev_arg_in --workspace" -a "(bgrun completions --workspaces)"

# Run command flags
complete -c bgrun -n "__fish_seen_subcommand_from run" -l name -d "Optional name for the job"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l workspace -d "Optional workspace tag"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l ready-when -d "Log pattern readiness"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l ready-when-regex -d "Regex log readiness"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l ready-when-port -d "TCP port readiness"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l ready-when-url -d "HTTP URL readiness"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l ready-when-file -d "File existence readiness"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l after -d "Start after a named job"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l pty -d "Allocate a PTY for the child"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l restart -d "Restart policy"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l backoff -d "Backoff duration"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l cols -d "PTY columns"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l rows -d "PTY rows"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l max-rss -d "Max RSS in MB"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l max-runtime -d "Max runtime"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l allocate-port -d "Allocate free port as env var"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l health-check-url -d "Health check HTTP URL"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l health-check-port -d "Health check TCP port"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l health-interval -d "Health check interval in secs"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l health-threshold -d "Health check failure threshold"

# Tail flags
complete -c bgrun -n "__fish_seen_subcommand_from tail" -l lines -d "Number of lines to show"
complete -c bgrun -n "__fish_seen_subcommand_from tail" -l digest -d "Show digest summary"
complete -c bgrun -n "__fish_seen_subcommand_from tail" -l level -d "Filter by level (error/warn)"
complete -c bgrun -n "__fish_seen_subcommand_from tail" -l stream -d "Filter by stream (stdout/stderr/pty)"
complete -c bgrun -n "__fish_seen_subcommand_from tail" -l strip-ansi -d "Strip ANSI escape codes"
complete -c bgrun -n "__fish_seen_subcommand_from tail" -l follow -d "Follow new log lines"
complete -c bgrun -n "__fish_seen_subcommand_from tail" -l filter-regex -d "Filter by regex pattern"

# Diff flags
complete -c bgrun -n "__fish_seen_subcommand_from diff" -l lines -d "Number of lines to show"
complete -c bgrun -n "__fish_seen_subcommand_from diff" -l stream -d "Filter by stream (stdout/stderr/pty)"
complete -c bgrun -n "__fish_seen_subcommand_from diff" -l strip-ansi -d "Strip ANSI escape codes"
complete -c bgrun -n "__fish_seen_subcommand_from diff" -l filter-regex -d "Filter by regex pattern"

# Send flags
complete -c bgrun -n "__fish_seen_subcommand_from send" -l newline -d "Append newline to data"
complete -c bgrun -n "__fish_seen_subcommand_from send" -l enter -d "Send an Enter (newline)"

# Screen flags
complete -c bgrun -n "__fish_seen_subcommand_from screen" -l lines -d "Number of lines to show"

# Clean flags
complete -c bgrun -n "__fish_seen_subcommand_from clean" -l workspace -d "Workspace to clean"
complete -c bgrun -n "__fish_seen_subcommand_from clean" -s f -l force -d "Skip confirmation"

# Status flags
complete -c bgrun -n "__fish_seen_subcommand_from status" -s n -l name -d "Job name"

# Kill flags
complete -c bgrun -n "__fish_seen_subcommand_from kill" -s n -l name -d "Job name"
complete -c bgrun -n "__fish_seen_subcommand_from kill" -l workspace -d "Workspace to kill"

# Global flags
complete -c bgrun -l json -d "Output in JSON format"
complete -c bgrun -l help -d "Print help" -s h
"#);
}

fn print_bash_completions() {
    let script = r#"# bgrun completions for bash shell
# Install: bgrun completions --shell bash > /etc/bash_completion.d/bgrun
# Or:      bgrun completions --shell bash >> ~/.bashrc

_bgrun()
{
    local cur prev words cword
    _init_completion || return

    local commands="run list status kill wait tail diff run-group send stats expect attach screen schema clean skill help"

    if [[ $cword -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$commands" -- "$cur"))
        return
    fi

    case "${words[1]}" in
        status|kill|wait|tail|diff|send|stats|expect|attach|screen)
            local ids
            ids=$(bgrun completions --active-ids 2>/dev/null | awk '{print $1}')
            COMPREPLY=($(compgen -W "$ids" -- "$cur"))
            ;;
        list|kill)
            case "$prev" in
                --workspace)
                    local ws
                    ws=$(bgrun completions --workspaces 2>/dev/null)
                    COMPREPLY=($(compgen -W "$ws" -- "$cur"))
                    ;;
                *)
                    COMPREPLY=($(compgen -W "--workspace" -- "$cur"))
                    ;;
            esac
            ;;
        run)
            local opts="--name --workspace --ready-when --ready-when-regex --ready-when-port --ready-when-url --ready-when-file --after --pty --restart --backoff --cols --rows --max-rss --max-runtime --allocate-port --health-check-url --health-check-port --health-interval --health-threshold"
            COMPREPLY=($(compgen -W "$opts" -- "$cur"))
            ;;
        clean)
            local opts="--workspace --force"
            COMPREPLY=($(compgen -W "$opts" -- "$cur"))
            ;;
    esac
} &&
complete -F _bgrun bgrun
"#;
    print!("{}", script);
}

fn print_zsh_completions() {
    let script = r#"# bgrun completions for zsh shell
# Install: bgrun completions --shell zsh > /usr/local/share/zsh/site-functions/_bgrun
# Or add to ~/.zshrc:  source <(bgrun completions --shell zsh)

#compdef bgrun

_bgrun() {
    local -a commands
    commands=(
        'run:Run a command in the background'
        'list:List running jobs'
        'status:Get status of a job'
        'kill:Kill a job'
        'wait:Wait for a job to become ready'
        'tail:Show the last N lines of a job log'
        'diff:Show log lines since the last diff call'
        'run-group:Run multiple named jobs in parallel'
        'send:Send data to a stdin'
        'stats:Show resource stats for a running job'
        'expect:Wait for a pattern in log output'
        'attach:Attach to a PTY job terminal'
        'screen:Show last N lines from in-memory buffer'
        'schema:Print JSON Schema for a command'
        'clean:Remove all terminated jobs'
        'skill:Manage embedded skills'
        'help:Print help'
    )

    _arguments -C \
        '--json[Output in JSON format]' \
        '(-h --help)'{-h,--help}'[Print help]' \
        '1: :->command' \
        '*:: :->args'

    case "$state" in
        command)
            _describe 'command' commands
            ;;
        args)
            case "$words[1]" in
                status|kill|wait|tail|diff|send|stats|expect|attach|screen)
                    local ids
                    ids=(${(f)"$(_call_program ids bgrun completions --active-ids 2>/dev/null | awk '{print $1}')"})
                    _values 'job id' $ids
                    ;;
                list|kill|clean)
                    _arguments '--workspace[Filter by workspace]:workspace:->workspaces'
                    ;;
                run)
                    _arguments \
                        '--name=[Job name]:name:' \
                        '--workspace=[Workspace tag]:workspace:' \
                        '--ready-when=[Log pattern readiness]:pattern:' \
                        '--ready-when-regex=[Regex log readiness]:pattern:' \
                        '--ready-when-port=[TCP port readiness]:port:' \
                        '--ready-when-url=[HTTP URL readiness]:url:' \
                        '--ready-when-file=[File existence readiness]:file:' \
                        '--after=[Start after a named job]:name:' \
                        '--pty[Allocate a PTY]' \
                        '--restart=[Restart policy]:policy:(on-crash)' \
                        '--backoff=[Backoff duration]:duration:' \
                        '--cols=[PTY columns]:number:' \
                        '--rows=[PTY rows]:number:' \
                        '--max-rss=[Max RSS in MB]:mb:' \
                        '--max-runtime=[Max runtime]:duration:' \
                        '--allocate-port=[Allocate free port as env var]:name:' \
                        '--health-check-url=[Health check HTTP URL]:url:' \
                        '--health-check-port=[Health check TCP port]:port:' \
                        '--health-interval=[Health check interval in secs]:seconds:' \
                        '--health-threshold=[Health check failure threshold]:count:'
                    ;;
            esac
            ;;
    esac

    case "$state" in
        workspaces)
            local ws
            ws=(${(f)"$(_call_program ws bgrun completions --workspaces 2>/dev/null)"})
            _values 'workspace' $ws
            ;;
    esac
}

_bgrun "$@"
"#;
    print!("{}", script);
}
