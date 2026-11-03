# Example plugins

Copy (or symlink) a folder into `~/.config/lumen/plugins/` and choose
**Reload Plugins** in the palette. See the [plugin guide](../../docs/plugins.md)
for writing your own in Python, and [Plugins](../../README.md#plugins) for the
protocol underneath.

| Plugin | Language | Permissions | What it does |
|---|---|---|---|
| [`hello`](hello/) | bash (+ `jq`) | none | **Say Hello** (Alt+H) shows a notification naming the pane you're in. The smallest plugin there is, speaking the raw protocol: register a command, react to it. |
| [`snippets`](snippets/) | Python 3.11+ | `input` | **Insert Snippet** (Ctrl+Shift+X, ⌘⇧X) picks from [`snippets.toml`](snippets/snippets.toml) and types it into the pane without pressing Enter. Edit the file to add your own. |
| [`exit-colors`](exit-colors/) | Python | `panes` | Tints each tab green or red by how its last command went, with the exit status as a badge, cleared when the next command starts. Shows tab styles reacting to events, in 30 lines. Needs shell integration. |
| [`todo`](todo/) | Python | `panes` | **Todo List** (Alt+T) opens a todo list in a pane on the right, with what's left on the tab: ↑/↓ move, Space ticks, A adds, E edits, D deletes. A plugin pane with its own line editor for typing. |
| [`snake`](snake/) | Python | `panes` | **Play Snake** opens a game in a tab of its own, with your score on the tab and your best kept. A plugin pane that animates, redrawing the whole pane every step. |

```sh
mkdir -p ~/.config/lumen/plugins
cp -r examples/plugins/snippets ~/.config/lumen/plugins/
cp -r examples/plugins/snake ~/.config/lumen/plugins/
```

For bigger examples, read the built-in plugins in [`plugins/`](../../plugins/):
recent folders, command history, a git branch picker, an SSH host picker,
project layouts, build status and a pomodoro timer. Turn them on from **Plugin Gallery** in the
palette; to change one, copy its folder into your plugins folder (yours runs
instead of the built-in one).

A layout for this repository, to try Project Layouts with:

```toml
# .lumen-layout.toml
title = "lumen"

[[pane]]
run = "git status"

[[pane]]
split = "right"
run = "cargo watch -x check"
```
