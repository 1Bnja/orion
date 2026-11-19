# Orion shell integration for zsh.
#
# Marks where each command starts and ends with OSC 133 sequences (a terminal standard).
# Orion reads them to build blocks:
#
#   ESC]133;A BEL   prompt starts
#   ESC]133;B BEL   prompt ends: what follows is the command you type
#   ESC]133;C BEL   you pressed Enter: what follows is the command output
#   ESC]133;D;n BEL command finished, with exit code n
#
# Also sets the title (OSC 0) with the current folder name: shown in the tab.
# Sets OSC 9;9 with the full path: Orion uses it to resolve relative paths on Ctrl+click.
#
# Loaded after your profile, so it wraps whatever prompt you already have.

__orion_first_prompt=1  # 1 = no prompt shown yet
__orion_ran_command=0   # 0 = no command ran since last prompt

function __orion_precmd {
    # $? must be first: any other code clears it.
    local code=$?
    local e=$'\e'
    local bel=$'\a'

    if (( ! __orion_first_prompt )); then
        # If a command ran: D with exit code. If Enter was pressed empty: D without code.
        if (( __orion_ran_command )); then
            printf '%s]133;D;%d%s' "$e" "$code" "$bel"
        else
            printf '%s]133;D%s' "$e" "$bel"
        fi
    fi
    __orion_first_prompt=0
    __orion_ran_command=0

    # Tab title (OSC 0): current folder name.
    local title=${PWD##*/}
    [[ -z $title ]] && title=$PWD  # filesystem root has no last segment
    printf '%s]0;%s%s' "$e" "$title" "$bel"
    # Full path (OSC 9;9): Orion uses this to resolve relative paths on Ctrl+click.
    printf '%s]9;9;%s%s' "$e" "$PWD" "$bel"

    # OSC 133 A: prompt starts.
    printf '%s]133;A%s' "$e" "$bel"
}

function __orion_preexec {
    __orion_ran_command=1
    local e=$'\e'
    local bel=$'\a'
    # OSC 133 C: command output starts.
    printf '%s]133;C%s' "$e" "$bel"
}

# Prepend so __orion_precmd runs first and captures $? before any other hook changes it.
precmd_functions=(__orion_precmd $precmd_functions[@])
autoload -Uz add-zsh-hook
add-zsh-hook preexec __orion_preexec

# OSC 133 B: prompt ends (command input starts).
# Appended to PS1 so it appears right after the prompt is drawn.
PS1="${PS1}"$'%{\e]133;B\a%}'

# Shift+Enter in Orion = new line without executing.
# Orion sends it as ESC+Enter (Meta+Enter), which is what code agents also use.
function __orion_add_newline {
    BUFFER="${BUFFER}
"
    CURSOR=${#BUFFER}
}
zle -N __orion_add_newline
bindkey $'\e\r' __orion_add_newline
