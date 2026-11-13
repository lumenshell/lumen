#!/usr/bin/env bash
# Rough performance check for a Lumen build, on a private X display.
#
#   scripts/perf.sh path/to/lumen
#
# Prints three numbers to compare between builds (lower is better):
#   cat-50mb-ms         time for a pane to show a 50 MB file
#   idle-cpu-ticks      CPU ticks Lumen uses in 10 s, idle, three tabs open
#   palette-x20-ticks   CPU ticks to open and close the palette 20 times
# Run it a few times and compare medians: single runs are noisy.
set -euo pipefail

BIN=$(realpath "${1:?usage: scripts/perf.sh path/to/lumen}")
for tool in Xvfb metacity python3; do
    command -v "$tool" >/dev/null || { echo "perf.sh needs $tool" >&2; exit 1; }
done

T=$(mktemp -d)
mkdir -p "$T/cfg/lumen" "$T/state" "$T/data" "$T/run" "$T/home"
chmod 700 "$T/run"
DISPLAY_NUM=:97
PIDS=()
cleanup() { kill "${PIDS[@]}" 2>/dev/null || true; wait 2>/dev/null || true; rm -rf "$T"; }
trap cleanup EXIT

Xvfb $DISPLAY_NUM -screen 0 1600x1000x24 >/dev/null 2>&1 & PIDS+=($!)
sleep 1
export DISPLAY=$DISPLAY_NUM HOME="$T/home" XDG_CONFIG_HOME="$T/cfg" XDG_STATE_HOME="$T/state" \
    XDG_DATA_HOME="$T/data" XDG_RUNTIME_DIR="$T/run"
unset LUMEN_SOCKET LUMEN_PANE
metacity --replace >/dev/null 2>&1 & PIDS+=($!)
sleep 1
"$BIN" >"$T/lumen.log" 2>&1 & LP=$!; PIDS+=($LP)
for _ in $(seq 50); do ls "$T"/run/lumen/*.sock >/dev/null 2>&1 && break; sleep 0.2; done
export LUMEN_SOCKET=$(ls "$T"/run/lumen/*.sock | head -1)
sleep 2
m() { "$BIN" msg "$@"; }
ticks() { awk '{print $14 + $15}' "/proc/$LP/stat"; }
now_ms() { date +%s%3N; }

# A 50 MB file of ordinary text lines.
python3 -c "
import sys
line = ''.join(chr(33 + (i * 7) % 90) for i in range(119)) + '\n'
with open(sys.argv[1], 'w') as f:
    f.write(line * (50 * 1024 * 1024 // len(line)))
" "$T/big.txt"

PANE=$(m list --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["tabs"][0]["panes"][0]["id"])')
m watch command-finished > "$T/events" 2>&1 & PIDS+=($!)
sleep 0.5
START=$(now_ms)
m send-text --pane "$PANE" "cat $T/big.txt
"
until grep -q '"command-finished"' "$T/events" 2>/dev/null; do sleep 0.01; done
echo "cat-50mb-ms $(( $(now_ms) - START ))"

m send-text --pane "$PANE" "clear
"
m new-tab >/dev/null; m new-tab >/dev/null
sleep 3
A=$(ticks); sleep 10; B=$(ticks)
echo "idle-cpu-ticks $(( B - A ))"

A=$(ticks)
for _ in $(seq 20); do
    m action command-palette; sleep 0.1
    m action command-palette; sleep 0.1
done
B=$(ticks)
echo "palette-x20-ticks $(( B - A ))"
