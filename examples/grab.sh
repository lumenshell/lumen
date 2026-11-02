#!/usr/bin/env bash
# Copy what another pane is showing, e.g. a stack trace from the server pane,
# without leaving the one you're in. Prints it, and puts it on the clipboard
# when a clipboard tool is available.
#
#   grab.sh PANE [LINES]    # LINES includes history, e.g. 200
#
# Without PANE it lists the panes to choose from.
set -euo pipefail

lumen=${LUMEN:-lumen}
if [[ $# -eq 0 ]]; then
    "$lumen" msg list
    echo "usage: grab.sh PANE [LINES]" >&2
    exit 2
fi

text=$("$lumen" msg read-screen --pane "$1" ${2:+--lines "$2"})
printf '%s\n' "$text"

for tool in "wl-copy" "xclip -selection clipboard" "xsel --clipboard --input" "pbcopy"; do
    if command -v "${tool%% *}" >/dev/null; then
        printf '%s' "$text" | $tool && echo "(copied $(wc -l <<<"$text") lines)" >&2
        break
    fi
done
