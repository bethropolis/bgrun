# bgrun completions for bash shell
# Install: bgrun completions --shell bash > /etc/bash_completion.d/bgrun
# Or:      bgrun completions --shell bash >> ~/.bashrc

_bgrun()
{
    local cur prev words cword
    _init_completion || return

    local commands="run list status kill wait tail diff run-group send stats expect attach schema clean help"

    if [[ $cword -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$commands" -- "$cur"))
        return
    fi

    case "${words[1]}" in
        status|kill|wait|tail|diff|send|stats|expect|attach)
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
            local opts="--name --workspace --ready-when --ready-when-regex --ready-when-port --ready-when-url --ready-when-file --after --pty --restart --backoff --cols --rows --max-rss --max-runtime"
            COMPREPLY=($(compgen -W "$opts" -- "$cur"))
            ;;
    esac
} &&
complete -F _bgrun bgrun
