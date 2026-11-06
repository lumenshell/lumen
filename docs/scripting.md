# Scripting with `lumen msg`

Scripts, shell aliases and plugins can drive Lumen.

- [Commands](#commands)
- [Events](#events)
- [Protocol](#protocol)
- [Plugin panes](#plugin-panes)
- [Security](#security)

`lumen msg` is the same `lumen` binary, so it needs to be on your PATH. The
installer, Homebrew and `cargo install` all put it there.

Inside a Lumen pane, `lumen msg` talks to that Lumen and acts on that pane by
default. From anywhere else, it finds the most recently started Lumen.

From Python, `lumen_plugin` does the same with less ceremony. See
[Scripts outside Lumen](plugins.md#scripts-outside-lumen).

```sh
lumen msg list                          # tabs and panes with their ids
p=$(lumen msg split right --run 'npm run dev')   # prints the new pane's id
lumen msg split down --pane "$p" --cwd ~/api --run 'cargo watch -x test' --no-focus
lumen msg send-text --pane "$p" 'q\n'   # type into a pane (\n is Enter)
lumen msg read-screen --pane "$p" --lines 20
lumen msg set-title "Servers"           # this pane's tab; no title resets it
lumen msg tab-style --badge prod --color red   # mark the tab; --clear removes it
lumen msg action toggle-zoom            # any action from Key bindings
lumen msg notify "Deploy done" "web-3 is live"
lumen msg watch command-finished        # events as JSON lines, until Ctrl+C
```

## Commands

| Command | Does |
|---|---|
| `list [--json]` | Every window's tabs and panes: ids, titles, folders, sizes, last exit status |
| `new-tab [--cwd DIR] [--run CMD]` | Open a tab. Prints its pane id |
| `split [right\|down] [--pane ID] [--cwd DIR] [--run CMD] [--size N] [--no-focus]` | Split a pane. Prints the new pane id. `--size` is the new pane's share: a fraction (`0.3`) or rows/columns (`12`) |
| `focus [ID]`, `close [ID]` | Show and focus, or close, a pane |
| `send-text [--pane ID] TEXT` | Type text. `\n`, `\r`, `\t`, `\e` and `\\` are understood |
| `read-screen [--pane ID] [--lines N]` | Print the screen, or the last N lines including history |
| `set-title [--pane ID] [TITLE]` | Name a tab. Without TITLE, the program's title shows again |
| `move-tab [--pane ID] N` | Move the pane's tab to position N in its window (1 is first) |
| `tab-style [--pane ID] [--badge TEXT] [--short TEXT] [--color COLOR] [--clear]` | Put a short badge beside the tab's title, tint the tab, or `--clear` both. Colours are `red`, `green`, `yellow`, `blue`, `magenta`, `cyan` from your theme, or `#rrggbb`. `--short` (up to 8 characters) shows instead of the badge when the tab is too narrow |
| `notify TITLE [BODY]` | Desktop notification (`notify-send` on Linux) |
| `quick-terminal` | Show the [quick terminal](quick-terminal.md), or hide it if it's in front |
| `action NAME [--pane ID]` | Run an action such as `split-right` or `find` (see [Actions](config.md#actions)) |
| `watch [EVENT...]` | Stream events: all of them, or the ones named |
| `call METHOD [JSON]` | Send any request and print the raw result |

`--run` types the command into the new pane's shell, so the shell stays open
after it finishes.

A new pane starts in the folder of the pane it was split from (or the current
one, for tabs) unless `--cwd` says otherwise.

Tab titles and badges set this way aren't kept between sessions.

### Examples

A notification whenever a command fails in any pane (needs
[`jq`](https://jqlang.org)):

```sh
lumen msg watch command-finished |
  jq --unbuffered -r 'select(.exit != 0) | "\(.command): exit \(.exit)"' |
  while read -r msg; do lumen msg notify "Command failed" "$msg"; done
```

[`examples/`](../examples/) has more complete scripts: a project layout,
run-and-wait, git branch tab titles, broadcasting, grabbing another pane's
output, and a Python client using the socket directly.

## Events

| Event | Fields | When |
|---|---|---|
| `pane-opened` / `pane-closed` | `pane` | A pane starts or closes |
| `focus-changed` | `pane` | Another pane gets keyboard focus |
| `title-changed` | `pane`, `title` | A program changes its title |
| `cwd-changed` | `pane`, `cwd` | The shell changes folder\* |
| `bell` | `pane` | A program rings the bell |
| `command-started` | `pane` | A command starts running\* |
| `command-finished` | `pane`, `command`, `exit`, `duration_ms` | A command finishes: what was typed, its exit status and how long it ran\* |

\* Needs [shell integration](shell-integration.md).

`focus-changed` is sent when the window next redraws. So it can trail the
change slightly, and isn't sent at all while the window is minimized.

## Protocol

`lumen msg` is a thin client. Anything that can open a Unix socket can do the
same.

Each Lumen listens on `lumen-<pid>.sock` in `$XDG_RUNTIME_DIR/lumen` (else
`$TMPDIR/lumen-<uid>`). Every pane gets the socket's path in `LUMEN_SOCKET`
and its own id in `LUMEN_PANE`.

Messages are [JSON-RPC 2.0](https://www.jsonrpc.org/specification), one per
line:

```sh
echo '{"jsonrpc":"2.0","id":1,"method":"split","params":{"direction":"down","run":"htop"}}' |
  socat - UNIX-CONNECT:"$LUMEN_SOCKET"
# {"jsonrpc":"2.0","id":1,"result":{"pane":4294967299}}
```

### Methods

`pane` is optional everywhere and means the focused pane, in the active
window.

| Method | Params | Notes |
|---|---|---|
| `hello` | | Answers with the protocol version, Lumen version, actions and events |
| `list` | | See below |
| `new-tab` | `cwd`, `run`, `plugin_pane` | |
| `split` | `pane`, `direction`, `cwd`, `run`, `focus`, `size`, `plugin_pane` | |
| `focus`, `close` | `pane` | |
| `send-text` | `pane`, `text` | |
| `read-screen` | `pane`, `lines` | |
| `set-title` | `pane`, `title` | |
| `set-tab-style` | `pane`, `badge`, `short_badge`, `color` | A field left out stays, `null` clears it, and a new `badge` drops the old `short_badge`. A plugin's styles go when it stops. A socket client's stay |
| `move-tab` | `pane`, `index` | `index` counts from 0 |
| `write` | `pane`, `text` | [Plugin panes](#plugin-panes) only |
| `notify` | `title`, `body` | |
| `run-action` | `action`, `pane` | |
| `quick-terminal` | | Answers `{"shown": true}` or `false` |
| `show-picker` | `items`, `placeholder`, `message` | `items` is a list of `{id, label, detail}`. `message` is text shown under the input. Answers `{"id": ...}` with the choice, or `null` if closed |
| `subscribe` | `events` | A list. Leave it out for all events |

`list` covers every window. Each tab has a `window` (0 for the first window
opened), `window_active` and `quick` (it's in the quick terminal). Its `index`
counts within its window. Panes a plugin draws have `"plugin_pane": true`,
and tabs show their `badge` and `color`.

After `subscribe`, the connection also receives events as notifications:

```json
{"jsonrpc":"2.0","method":"event","params":{"event":"bell","pane":4294967298}}
```

Errors use the standard JSON-RPC codes, with `-32000` for requests that can't
be carried out, such as an unknown pane.

The protocol version (`hello`) goes up only when a change could break existing
clients.

## Plugin panes

`"plugin_pane": true` on `new-tab` or `split` opens a pane that runs no
program. The connection that opened it draws it with `write`: text with escape
sequences, as a program would print. Nobody else may write to it.

The connection is sent what happens to the pane as `pane` notifications:

- `"input"`, for what's typed. Keys arrive as a terminal sends them, so ↑ is
  `\u001b[A`:
  `{"method":"pane","params":{"event":"input","pane":…,"text":"q"}}`
- `"resize"`, with `cols` and `rows`. Also sent once when the pane first has a
  size.
- `"closed"`

The pane closes when the connection does, and isn't kept in the session. Its
tab is named after the plugin (`script` for a socket client) until it sets a
title.

## Security

The socket and its folder are readable only by you. But anything running as
you can use it to type into panes and read their screens. If that's a concern,
turn it off with `socket-api = false`.
