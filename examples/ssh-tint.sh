#!/usr/bin/env bash
# ssh, with the tab marked while you're connected: a badge naming the host,
# tinted red for production hosts and blue for the rest. The tab goes back to
# normal when ssh exits.
#
#   ssh-tint.sh db-1.prod.example.com
#   alias ssh=ssh-tint.sh          # in your shell's rc file
#
# Production hosts are the ones matching PROD_HOSTS, a shell pattern
# (default: *prod*). Outside Lumen it's plain ssh.
set -uo pipefail

lumen=${LUMEN:-lumen}
[[ -n ${LUMEN_PANE:-} ]] || exec ssh "$@"

# ssh -G works out the real host name from your arguments and ~/.ssh/config.
host=$(ssh -G "$@" 2>/dev/null | awk '/^hostname / { print $2; exit }')
[[ -n $host ]] || exec ssh "$@"

if [[ $host == ${PROD_HOSTS:-*prod*} ]]; then color=red; else color=blue; fi
"$lumen" msg tab-style --badge "${host:0:24}" --color "$color" 2>/dev/null
trap '"$lumen" msg tab-style --clear 2>/dev/null' EXIT

ssh "$@"
