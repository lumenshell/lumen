# The plugin protocol

A plugin is a program in its own folder, written in any language. Your own
plugins live under `~/.config/lumen/plugins/` (`$XDG_CONFIG_HOME/lumen/plugins`)
and are named after their folder. Plugins installed with `lumen plugins` live
under `~/.local/share/lumen/installed-plugins/` (`$XDG_DATA_HOME`;
`~/Library/Application Support` on macOS) and are named `owner/name`, which is
also their `LUMEN_PLUGIN` and the name in `plugin:owner/name:id` key bindings.

For Python, use `lumen_plugin`, which comes with Lumen. The
[plugin guide](plugins.md) covers it. This page is the protocol underneath,
for other languages.

## Running

Lumen starts every plugin when it opens and stops them all when it quits.

If a plugin exits, Lumen restarts it. After quick crashes, it waits 1, 2, 5,
10, 30 and then 60 seconds before each restart.

Each plugin runs as its own process, so a slow or crashing plugin can't freeze
Lumen.

When a plugin is stopped (turned off, removed, replaced by a new version, or
Lumen quits), it loses its connection at once: its stdin closes, nothing more
it asks for is carried out, it's sent no more events, and its panes close. It
and every process it started are sent SIGTERM, and SIGKILL two seconds later
if they're still there. A program that should outlive the plugin, such as a
browser it opened, has to be started in a session of its own (`setsid`, or
`start_new_session=True` in Python).

## The manifest

```
~/.config/lumen/plugins/git-branch/
├── plugin.toml
└── main.py
```

```toml
# plugin.toml
description = "Switch git branches from the palette"
version = "1.0.0"
command = "python3 main.py"      # run in the plugin's folder; or a list: ["node", "my plugin.js"]
permissions = ["input"]           # any of "panes", "input", "screen"; leave out if none
```

### Settings

A plugin can have settings, which people set in their `config.toml` under
`[plugin.NAME]`. Describe each one in `plugin.toml`:

```toml
[settings.max-entries]
type = "integer"            # "string", "integer", "number", "boolean", "choice" or "list"
default = 500               # required, and must suit the type
min = 1                     # integer and number only, optional
max = 100000
description = "How many entries to keep"   # required

[settings.engine]
type = "choice"
choices = ["docker", "podman"]
default = "docker"
description = "Which container engine to use"
```

`number` takes whole numbers too. `list` is a list of strings. A mistake in
`[settings]` stops the plugin from loading, with the reason in the banner. A
wrong value in someone's `[plugin.NAME]` table is shown in their banner and
the default is used instead, so the plugin always gets valid values.

## Talking to Lumen

A plugin talks to Lumen over its stdin and stdout. It uses the same JSON-RPC
2.0 messages as the [socket API](scripting.md#protocol), one per line. It
should exit when stdin closes.

Anything it writes to stderr goes to its log. **Plugin Logs…** in the
palette opens it in a new tab.

Its environment has:

| Variable | Holds |
|---|---|
| `LUMEN_PLUGIN` | Its name |
| `LUMEN_PLUGIN_DIR` | Its folder |
| `LUMEN_PLUGIN_DATA` | A folder for its own data, kept across updates and readable only by you |
| `LUMEN_PLUGIN_SETTINGS` | Its settings as a JSON object, every one present, defaults filled in (`{}` with none) |
| `PYTHONPATH` | Starts with the folder holding `lumen_plugin` |

## What plugins can do

Everything in the socket API, plus:

- **Add palette commands.** Call `register-command` with `id`, `title` and
  optionally `keys` (such as `"alt-b"`). When you choose one, Lumen sends the
  plugin
  `{"jsonrpc":"2.0","method":"command","params":{"id":"switch","pane":4294967298}}`,
  where `pane` is the focused pane. Commands go away when the plugin stops.
- **Hear about new settings.** When someone saves `config.toml` with
  different values for the plugin, Lumen sends it
  `{"jsonrpc":"2.0","method":"event","params":{"event":"settings-changed","settings":{…}}}`
  with every setting, defaults filled in. The plugin isn't restarted, and
  doesn't need to subscribe: this goes only to the plugin concerned, and
  only when its values changed. A plugin restarted later gets the new values
  in `LUMEN_PLUGIN_SETTINGS`.
- **See how plugins are doing.** `plugin-status` lists every plugin with its
  `status` and whether it's in `trouble`. It needs no permission; the plugin
  manager uses it.
- **Ask you to choose.** `show-picker` opens the palette with the plugin's
  items, and answers with the one you pick (or `null` for Esc).
- **Ask you to type.** `show-prompt` opens the palette as a text input, and
  answers with what you typed (or `null` for Esc).
- **Draw panes of their own.** A [plugin pane](scripting.md#plugin-panes) runs
  no program. The plugin writes to it and gets its keys, for timers,
  dashboards, games or anything else a terminal can show.
- **Mark tabs.** `set-tab-style` puts a badge beside a tab's title and tints
  it, for a countdown, a build that broke or a production server.
- **Label splits.** `set-pane-style` names a pane with a short label and a
  colour, drawn above it while its tab is split, for a dev environment's
  servers or the task that failed.

## Permissions

Without any permissions, a plugin can list panes, subscribe to events, show
notifications, add commands and show pickers and prompts.

Anything more is declared in `plugin.toml` and needs your approval. For your
own plugins, Lumen asks once in the palette, the first time it sees the
plugin, and again if its permissions or command change. For plugins installed
with `lumen plugins`, you approve them when installing, and again only when an
update asks for a permission it didn't have; Lumen never asks about them as it
starts.

| Permission | Allows |
|---|---|
| `panes` | `new-tab`, `split`, `focus`, `close`, `set-title`, `set-tab-style`, `set-pane-style`, `move-tab`, `quick-terminal`, and drawing its own plugin panes (`write`) |
| `input` | `send-text`, `run-action`, and `run` on `new-tab` and `split`: typing into panes, which can run any command |
| `screen` | `read-screen`, and the command text in `command-finished` events |

A plugin missing a permission gets a JSON-RPC error saying which one to add.

Your answers are kept in `~/.local/state/lumen/plugin-approvals.json`, outside
the plugins folder, so a plugin can't approve itself.

- To change your mind about one you refused, choose **Allow Plugin: <name>**
  in the palette.
- To be asked again, delete its entry from that file.

Permissions limit what a plugin can do *through Lumen*. They aren't a sandbox.
A plugin is a program running as you, so only install ones you trust, as you
would any other program.

## Typing into shells safely

When a plugin types into a shell with `send-text`, anything it didn't write
itself (a branch or file name, text from the screen) must be:

- **quoted for the shell.** Git branch names, for one, can contain `$`, `;`
  and backticks.
- **free of control characters.** A carriage return presses Enter.
- **free of invisible characters**, such as right-to-left overrides, which
  make a command look different from what it does.

Quote in a way fish also understands. POSIX single quotes aren't enough there,
because fish reads `\'` inside them as an escaped quote. `lumen_plugin.quote`
handles this. So does avoiding the shell, for example with `new-tab`'s `cwd`.

## Debugging and reloading

Problems in a `plugin.toml` are listed in the banner at the bottom of the
window. Fix them and choose **Reload Plugins**, which also picks up new and
changed plugins.

## Examples

- The built-in plugins in [`plugins/`](../plugins/), all Python.
- [`examples/plugins/`](../examples/plugins/):
  - `hello`: bash speaking the raw protocol
  - `snippets`: a snippet library
  - `exit-colors`: tabs tinted by exit status
  - `todo`: a todo list in a plugin pane
  - `snake`: a game
