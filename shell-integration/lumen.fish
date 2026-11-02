# Lumen shell integration for fish.
#
# Marks prompts and commands for the terminal (OSC 133) and reports the
# working directory (OSC 7). Lumen loads this automatically through
# XDG_DATA_DIRS; to use it elsewhere (e.g. over SSH), copy it to
# ~/.config/fish/conf.d/.

status is-interactive; or exit
set -q __lumen_loaded; and exit
set -g __lumen_loaded 1

# Don't pass Lumen's directory on to programs started from this shell.
if set -q LUMEN_FISH_DATA
    if set -l i (contains -i -- $LUMEN_FISH_DATA $XDG_DATA_DIRS)
        set -e XDG_DATA_DIRS[$i]
    end
    set -e LUMEN_FISH_DATA
end

function __lumen_postexec --on-event fish_postexec
    printf '\e]133;D;%s\a' $status
end

function __lumen_preexec --on-event fish_preexec
    printf '\e]133;C\a'
end

function __lumen_prompt --on-event fish_prompt
    printf '\e]133;A\a'
    # URL-escaped (keeps /), so a directory name can't inject escape sequences.
    printf '\e]7;file://%s%s\a' $hostname (string escape --style=url -- $PWD)
    # Mark the end of the prompt by wrapping fish_prompt, once the user's own
    # config has defined it.
    if not functions -q __lumen_user_prompt
        functions -c fish_prompt __lumen_user_prompt
        function fish_prompt
            __lumen_user_prompt
            printf '\e]133;B\a'
        end
    end
end
