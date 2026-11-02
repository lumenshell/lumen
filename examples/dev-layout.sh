#!/usr/bin/env bash
# Open a project in a new tab laid out for work: an editor on the left, a dev
# server top right and a test watcher bottom right, with the tab named after
# the project.
#
#   dev-layout.sh [DIR]
#
# Change the commands with environment variables, e.g.
#   SERVER='npm run dev' TESTS='npm test -- --watch' dev-layout.sh ~/code/site
set -euo pipefail

lumen=${LUMEN:-lumen}
dir=$(cd "${1:-.}" && pwd)
editor=${EDITOR_CMD:-${EDITOR:-vim} .}
server=${SERVER:-}
tests=${TESTS:-}

# Guess sensible commands from the files in the project.
if [[ -z $server && -z $tests ]]; then
    if [[ -f $dir/Cargo.toml ]]; then
        server='cargo run'; tests='cargo test'
    elif [[ -f $dir/package.json ]]; then
        server='npm run dev'; tests='npm test'
    elif [[ -f $dir/manage.py ]]; then
        server='python manage.py runserver'; tests='python manage.py test'
    fi
fi

editor_pane=$("$lumen" msg new-tab --cwd "$dir" --run "$editor")
"$lumen" msg set-title --pane "$editor_pane" "$(basename "$dir")"

# Split without moving focus, so the editor keeps it.
server_pane=$("$lumen" msg split right --pane "$editor_pane" --no-focus ${server:+--run "$server"})
"$lumen" msg split down --pane "$server_pane" --no-focus ${tests:+--run "$tests"} >/dev/null

echo "Opened $(basename "$dir") in a new tab (editor pane $editor_pane)"
