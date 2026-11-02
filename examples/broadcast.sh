#!/usr/bin/env bash
# Type the same command into every pane of the current tab, say to run
# `git pull` in several checkouts at once, or `uptime` on several SSH hosts.
#
#   broadcast.sh 'git pull'
#   broadcast.sh --no-enter 'partial command'   # type it, don't run it
set -euo pipefail

lumen=${LUMEN:-lumen}
enter=$'\r'
if [[ ${1-} == --no-enter ]]; then enter=''; shift; fi
[[ $# -eq 1 ]] || { echo "usage: broadcast.sh [--no-enter] TEXT" >&2; exit 2; }

panes=$("$lumen" msg list --json | jq -r '.tabs[] | select(.active) | .panes[].id')
for pane in $panes; do
    "$lumen" msg send-text --pane "$pane" "$1$enter"
done
echo "Sent to $(wc -w <<<"$panes") panes"
