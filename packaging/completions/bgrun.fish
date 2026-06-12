# bgrun completions for fish shell
# Install: bgrun completions --shell fish > ~/.config/fish/completions/bgrun.fish
# Or use install.sh which handles this automatically.

# Disable file path completion for bgrun
complete -c bgrun -f

# ── Dynamic subcommand discovery ──────────────────────────────────────
# Parse subcommands from `bgrun --help` so new commands work automatically.
function __bgrun_subcommands
    bgrun --help 2>/dev/null | command awk '
        BEGIN            { found = 0 }
        /^Commands:/     { found = 1; next }
        found && /^  [a-z]/ { print $1 }
        found && /^$/    { exit }
    '
end

# ── Dynamic flag discovery ──────────────────────────────────────────
# Parse long flags for a given subcommand from `bgrun <cmd> --help`.
function __bgrun_flags
    set -l cmd (commandline -op)
    if test (count $cmd) -ge 2
        bgrun $cmd[2] --help 2>/dev/null | command awk '
            /^Options:/   { found = 1; next }
            found && /^      --/ {
                gsub(/:.*$/, "", $1)
                print $1
            }
            found && /^  -h/ { exit }
        '
    end
end

# ── Dynamic job ID completion ───────────────────────────────────────
function __bgrun_active_ids
    bgrun completions --active-ids 2>/dev/null
end

# ── Dynamic workspace completion ────────────────────────────────────
function __bgrun_workspaces
    bgrun completions --workspaces 2>/dev/null
end

# ── Register top-level subcommands ──────────────────────────────────
complete -c bgrun -n "__fish_use_subcommand" -a "(__bgrun_subcommands)"

# ── Dynamic job ID completions for all commands that take a job ID ──
# Commands that take a job ID as their first positional argument:
#   status, kill, wait, tail, diff, send, stats, expect, attach
for cmd in status kill wait tail diff send stats expect attach
    complete -c bgrun -n "__fish_seen_subcommand_from $cmd" \
        -a "(__bgrun_active_ids)"
end

# ── Dynamic workspace completions for --workspace flag ──────────────
for cmd in list kill run run-group
    complete -c bgrun -n "__fish_seen_subcommand_from $cmd; and __fish_prev_arg_in --workspace" \
        -a "(__bgrun_workspaces)"
end

# ── Dynamic flag completions per subcommand ─────────────────────────
# For every subcommand, complete with its flags.
# This works because __bgrun_flags parses `bgrun <subcmd> --help`.
complete -c bgrun -n "not __fish_use_subcommand" \
    -a "(__bgrun_flags)"
