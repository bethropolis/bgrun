# bgrun completions for zsh shell
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
        'schema:Print JSON Schema for a command'
        'clean:Remove all terminated jobs'
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
                status|kill|wait|tail|diff|send|stats|expect|attach)
                    local ids
                    ids=(${(f)"$(_call_program ids bgrun completions --active-ids 2>/dev/null | awk '{print $1}')"})
                    _values 'job id' $ids
                    ;;
                list|kill)
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
                        '--max-runtime=[Max runtime]:duration:'
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
