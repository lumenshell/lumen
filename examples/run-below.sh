#!/usr/bin/env bash
# Run a command in a pane below this one and wait for it. On success the pane
# closes itself after a moment; on failure it stays open and takes focus, so
# you can read what went wrong. Exits with the command's status.
#
#   run-below.sh make test
#   run-below.sh cargo build --release && echo shipped
#
# Needs shell integration (to know when the command finished).
set -euo pipefail

lumen=${LUMEN:-lumen}
[[ $# -gt 0 ]] || { echo "usage: run-below.sh COMMAND..." >&2; exit 2; }
cmd=$(printf '%q ' "$@")

# Listen before starting, so the finish can't be missed.
coproc WATCH { "$lumen" msg watch command-finished; }
trap 'kill "$WATCH_PID" 2>/dev/null || true' EXIT

pane=$("$lumen" msg split down --no-focus --run "$cmd")
echo "Running in pane $pane: $*"

while read -r event <&"${WATCH[0]}"; do
    [[ $(jq -r .pane <<<"$event") == "$pane" ]] || continue
    status=$(jq -r '.exit // 0' <<<"$event")
    seconds=$(jq -r '.duration_ms / 1000 | floor' <<<"$event")
    if [[ $status == 0 ]]; then
        echo "✓ done in ${seconds}s"
        sleep 1.5
        "$lumen" msg close "$pane"
    else
        echo "✗ exit $status after ${seconds}s; the pane is still open"
        "$lumen" msg focus "$pane"
    fi
    exit "$status"
done
echo "Lost the connection to Lumen" >&2
exit 1
