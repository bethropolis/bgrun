# bgrun completions for fish shell
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
complete -c bgrun -n "__fish_seen_subcommand_from run" -l restart -d "Restart policy (never/on-crash/on-failure/always)"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l backoff -d "Backoff duration"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l max-retries -d "Max consecutive restart attempts"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l cols -d "PTY columns"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l rows -d "PTY rows"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l max-rss -d "Max RSS in MB"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l max-runtime -d "Max runtime"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l allocate-port -d "Allocate free port as env var"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l health-check-url -d "Health check HTTP URL"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l health-check-port -d "Health check TCP port"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l health-interval -d "Health check interval in secs"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l health-threshold -d "Health check failure threshold"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l env -s e -d "Env var KEY=VAL (repeatable)"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l cwd -s C -d "Working directory"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l replace -d "Kill existing same-name job first"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l wait -d "Block until Ready after starting"
complete -c bgrun -n "__fish_seen_subcommand_from run" -l wait-timeout -d "Timeout for --wait"

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

