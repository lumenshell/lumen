# Lumen shell integration for zsh.
#
# Marks prompts and commands for the terminal (OSC 133) and reports the
# working directory (OSC 7). Lumen loads this automatically; to use it
# elsewhere (e.g. over SSH), source it from ~/.zshrc.

[[ -o interactive ]] || return 0
(( ${+__lumen_loaded} )) && return 0
typeset -g __lumen_loaded=1 __lumen_ran=0
# Lumen gives each pane a key and only believes marks that carry it, so a
# program's output (or a file you cat) can't fake a prompt or a command.
typeset -g __lumen_k=${LUMEN_MARK_KEY:+;k=$LUMEN_MARK_KEY}

autoload -Uz add-zsh-hook add-zle-hook-widget

# Percent-encode control characters, spaces and % in $1 into REPLY, so a
# directory name can't end the OSC 7 early and inject escape sequences.
__lumen_urlencode() {
    emulate -L zsh
    local LC_ALL=C s=$1 c i
    REPLY=
    for (( i = 1; i <= ${#s}; i++ )); do
        c=$s[i]
        case $c in
            [[:cntrl:]]|' '|%) printf -v c '%%%02X' "'$c" ;;
        esac
        REPLY+=$c
    done
}

__lumen_precmd() {
    local st=$? REPLY
    (( __lumen_ran )) && printf '\e]133;D;%s%s\a' $st "$__lumen_k"
    __lumen_ran=0
    printf '\e]133;A%s\a' "$__lumen_k"
    __lumen_urlencode $PWD
    printf '\e]7;file://%s%s\a' $HOST $REPLY
}

__lumen_preexec() {
    __lumen_ran=1
    printf '\e]133;C%s\a' "$__lumen_k"
}

# The line editor starts once the prompt is drawn: that's where input begins.
__lumen_line_init() {
    printf '\e]133;B%s\a' "$__lumen_k"
}

add-zsh-hook precmd __lumen_precmd
add-zsh-hook preexec __lumen_preexec
add-zle-hook-widget line-init __lumen_line_init
