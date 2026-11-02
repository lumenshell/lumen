#!/usr/bin/env bash
# Keep every tab named after the git repository and branch of its focused
# pane, e.g. "lumen ⎇ main", updating as you cd around. Tabs outside a repo
# go back to their normal title. Leave it running (e.g. in a spare pane, or
# `git-titles.sh &` from your shell's startup file).
#
# Needs shell integration (for the directory changes).
set -euo pipefail

lumen=${LUMEN:-lumen}

title_for() {
    local dir=$1 top branch
    top=$(git -C "$dir" rev-parse --show-toplevel 2>/dev/null) || return 1
    branch=$(git -C "$dir" symbolic-ref --short -q HEAD || git -C "$dir" rev-parse --short HEAD)
    printf '%s ⎇ %s' "$(basename "$top")" "$branch"
}

update() {
    local pane=$1 dir=$2 title
    if title=$(title_for "$dir"); then
        "$lumen" msg set-title --pane "$pane" "$title"
    else
        "$lumen" msg set-title --pane "$pane"
    fi
}

# Name the tabs that are already open, then follow along.
"$lumen" msg list --json |
    jq -r '.tabs[] | .panes[] | select(.active and .cwd != null) | "\(.id)\t\(.cwd)"' |
    while IFS=$'\t' read -r pane dir; do update "$pane" "$dir"; done

"$lumen" msg watch cwd-changed | while read -r event; do
    update "$(jq -r .pane <<<"$event")" "$(jq -r .cwd <<<"$event")"
done
