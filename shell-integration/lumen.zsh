# Lumen shell integration for zsh.
#
# Marks prompts and commands for the terminal (OSC 133) and reports the
# working directory (OSC 7). Lumen loads this automatically; to use it
# elsewhere (e.g. over SSH), source it from ~/.zshrc.

[[ -o interactive ]] || return 0
(( ${+__lumen_loaded} )) && return 0
typeset -g __lumen_loaded=1 __lumen_ran=0

autoload -Uz add-zsh-hook add-zle-hook-widget

__lumen_precmd() {
    local st=$?
    (( __lumen_ran )) && printf '\e]133;D;%s\a' $st
    __lumen_ran=0
    local p=${PWD//\%/%25}
    printf '\e]7;file://%s%s\a' $HOST ${p// /%20}
    printf '\e]133;A\a'
}

__lumen_preexec() {
    __lumen_ran=1
    printf '\e]133;C\a'
}

# The line editor starts once the prompt is drawn: that's where input begins.
__lumen_line_init() {
    printf '\e]133;B\a'
}

add-zsh-hook precmd __lumen_precmd
add-zsh-hook preexec __lumen_preexec
add-zle-hook-widget line-init __lumen_line_init
