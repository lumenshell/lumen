# Scripting examples

Small, working scripts built on [`lumen msg`](../README.md#scripting-lumen-msg)
and the socket API. Copy them somewhere on your PATH and adapt them. They need
`lumen` on your PATH (`cargo install --path .`), or set `LUMEN` to the binary,
and most use [`jq`](https://jqlang.org) to read JSON.

| Script | What it does | Shows off |
|---|---|---|
| [`dev-layout.sh`](dev-layout.sh) `[DIR]` | Opens a project in a new tab: editor left, dev server top right, tests bottom right, tab named after the project. Guesses the commands for Rust, Node and Django projects; override with `SERVER`, `TESTS`, `EDITOR_CMD` | `new-tab`, `split --no-focus --run`, chaining pane ids, `set-title` |
| [`run-below.sh`](run-below.sh) `COMMAND...` | Runs a command in a pane below, waits for it, closes the pane if it worked and focuses it if it didn't. Exits with the command's status, so `run-below.sh make && deploy` works | `watch` events, waiting on one pane, `close`, `focus` |
| [`git-titles.sh`](git-titles.sh) | Keeps each tab titled with its repository and branch (`lumen ⎇ main`) as you `cd` around. Leave it running | `cwd-changed` events, `list --json`, `set-title` |
| [`broadcast.sh`](broadcast.sh) `[--no-enter] TEXT` | Types the same command into every pane of the current tab | `list --json`, `send-text` |
| [`grab.sh`](grab.sh) `PANE [LINES]` | Prints another pane's screen (or its last LINES lines with history) and copies it to the clipboard | `read-screen` |
| [`pane_log.py`](pane_log.py) | A live log of panes opening and closing and every command that finishes, with status, time and directory. Talks to the socket directly in Python, no `lumen msg` needed | The raw JSON-RPC protocol, `subscribe`, a second connection for requests |

For plugins (which Lumen starts and supervises, and which can add palette
commands and pickers), see [`plugins/`](plugins/).

`run-below.sh`, `git-titles.sh` and `pane_log.py` rely on
[shell integration](../README.md#shell-integration) for command and directory
events.
