# bash completion for gswitch
# Install: copy to /usr/share/bash-completion/completions/gswitch
#          or /etc/bash_completion.d/gswitch
#          or source this file from ~/.bashrc

_gswitch() {
    local cur prev words cword
    _init_completion -s || return

    # subcommands that do NOT need root (can be completed at any time)
    local no_root_subcmds="query switchable default ext-display runtime-pm cache-query"

    # all subcommands
    local subcmds="integrated passthrough hybrid nvidia query switchable power default ext-display runtime-pm reset cache-create cache-delete cache-query"

    # find if we already have a subcommand in COMP_WORDS
    local have_subcmd= i=
    for (( i = 1; i < COMP_CWORD; i++ )); do
        local w="${COMP_WORDS[i]}"
        # skip global options
        [[ "$w" == -* ]] && continue
        # check if it's a subcommand
        for sc in $subcmds; do
            [[ "$w" == "$sc" ]] && have_subcmd="$sc" && break 2
        done
    done

    # no subcommand yet → offer subcommands
    if [[ -z "$have_subcmd" ]]; then
        COMPREPLY=($(compgen -W "$subcmds" -- "$cur"))
        return
    fi

    # subcommand-specific completions
    case "$have_subcmd" in
        hybrid)
            # --rtd3 <level>
            if [[ "$cur" == -* ]]; then
                COMPREPLY=($(compgen -W "--rtd3" -- "$cur"))
            elif [[ "$prev" == "--rtd3" ]]; then
                COMPREPLY=($(compgen -W "0 1 2 3" -- "$cur"))
            fi
            ;;
        power)
            # subcommands: on / off / auto, no flags
            COMPREPLY=($(compgen -W "on off auto" -- "$cur"))
            ;;
        *)
            # for other subcommands, no further args needed
            COMPREPLY=()
            ;;
    esac
} &&
    complete -F _gswitch gswitch