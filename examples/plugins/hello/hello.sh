#!/usr/bin/env bash
# A Lumen plugin in bash. Lumen runs it with JSON-RPC on stdin/stdout, one
# message per line; anything printed to stderr goes to its log (palette:
# "Plugin Log: hello"). Needs jq.
set -euo pipefail

send() { printf '%s\n' "$1"; }

# Add "Say Hello" to the palette, on Alt+H. No reply is needed, so it's sent
# as a notification (no "id").
send '{"jsonrpc":"2.0","method":"register-command","params":{"id":"hello","title":"Say Hello","keys":"alt-h"}}'
echo "registered Say Hello" >&2

# Lumen sends {"method":"command","params":{"id":...,"pane":...}} when a
# command is chosen. The loop ends when Lumen closes stdin (quit or reload).
while read -r line; do
    [[ $(jq -r '.method // empty' <<<"$line") == command ]] || continue
    pane=$(jq -r '.params.pane' <<<"$line")
    echo "Say Hello chosen in pane $pane" >&2
    send "$(jq -nc --arg body "You were in pane $pane." \
        '{jsonrpc:"2.0", method:"notify", params:{title:"Hello from a bash plugin", body:$body}}')"
done
