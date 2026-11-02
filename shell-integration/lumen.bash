# Lumen shell integration for bash.
#
# Marks prompts and commands for the terminal (OSC 133) and reports the
# working directory (OSC 7). Lumen loads this automatically with --rcfile;
# to use it elsewhere (e.g. over SSH), source it at the end of ~/.bashrc.

# Started by Lumen with --rcfile: load the usual startup files first.
if [[ -n ${LUMEN_BASH_RCFILE-} ]]; then
    unset LUMEN_BASH_RCFILE
    [[ -f /etc/bash.bashrc ]] && source /etc/bash.bashrc
    [[ -f ~/.bashrc ]] && source ~/.bashrc
fi

[[ $- == *i* ]] || return 0
[[ -n ${__lumen_loaded-} ]] && return 0
__lumen_loaded=1

__lumen_ran=0

# Percent-encode control characters, spaces and % in $1 into REPLY, so a
# directory name can't end the OSC 7 early and inject escape sequences.
__lumen_urlencode() {
    local LC_ALL=C s=$1 c i
    REPLY=
    for (( i = 0; i < ${#s}; i++ )); do
        c=${s:i:1}
        case $c in
            [[:cntrl:]]|' '|%) printf -v c '%%%02X' "'$c" ;;
        esac
        REPLY+=$c
    done
}

__lumen_precmd() {
    local st=$? REPLY
    if (( __lumen_ran )); then
        printf '\e]133;D;%s\a' "$st"
    fi
    __lumen_ran=0
    printf '\e]133;A\a'
    __lumen_urlencode "$PWD"
    printf '\e]7;file://%s%s\a' "${HOSTNAME-}" "$REPLY"
}

# Runs after any other prompt commands, so themes that rebuild PS1 each time
# (starship and friends) still get the end-of-prompt mark.
__lumen_ps1() {
    [[ $PS1 == *'133;B'* ]] || PS1=$PS1'\[\e]133;B\a\]'
}

# PS0 is printed after a command is read, just before it runs (not for an
# empty Enter). The array subscript is evaluated in this shell, so it can
# record that a command ran without a DEBUG trap (which themes may own).
__lumen_ps0='${__lumen_noop[__lumen_ran=1]}'$'\e]133;C\a'
[[ ${PS0-} == *__lumen_ran* ]] || PS0=${PS0-}$__lumen_ps0

if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
    PROMPT_COMMAND=(__lumen_precmd "${PROMPT_COMMAND[@]}" __lumen_ps1)
else
    PROMPT_COMMAND="__lumen_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}; __lumen_ps1"
fi
