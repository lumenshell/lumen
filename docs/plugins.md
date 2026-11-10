# Writing Lumen plugins in Python

A plugin is a program Lumen starts and talks to. It can add commands to the
palette (with their own keys), react to what happens in your panes, show
pickers and notifications, open tabs and splits, type into panes and read
them. Plugins can be written in any language (see [the protocol](plugin-protocol.md));
this guide covers `lumen_plugin`, the Python package that comes with Lumen.
The [built-in plugins](../plugins/) are written with it and make good
examples, from 40 lines (`git-branch`) to 200 (`command-history`).

- [Your first plugin](#your-first-plugin)
- [Commands](#commands)
- [Events](#events)
- [Talking to Lumen](#talking-to-lumen)
- [Permissions](#permissions)
- [Scripts outside Lumen](#scripts-outside-lumen)
- [Debugging](#debugging)
- [Reference](#reference)

## Your first plugin

Make a folder for it under `~/.config/lumen/plugins/`, with two files:

```toml
# ~/.config/lumen/plugins/where/plugin.toml
description = "Tell me where this pane is"
command = "python3 main.py"
```

```python
# ~/.config/lumen/plugins/where/main.py
from lumen_plugin import Plugin

app = Plugin()

@app.command("Where Am I", keys="alt-w")
async def where(pane):
    await app.notify("You are here", await pane.cwd() or "somewhere (no shell integration?)")

app.run()
```

Open the palette and choose **Reload Plugins**, then press Alt+W (or choose
**Where Am I** in the palette). That's the whole loop: change the code, Reload
Plugins, try it.

There's nothing to install. Lumen puts `lumen_plugin` on every plugin's
`PYTHONPATH`, and it always matches the Lumen that's running. It needs Python
3.8 or newer and only the standard library.

## Commands

`@app.command(title, keys=None, id=None)` adds a palette command. The function
gets the focused [`Pane`](#pane) when you run it:

```python
@app.command("Open Notes", keys="alt-n")
async def notes(pane):
    await app.new_tab(cwd="~/notes", run="nvim index.md")
```

- **keys** is one key, written as in `[keys]`: `"alt-n"`, `"ctrl-shift-k"`,
  `"cmd-shift-l"` (macOS). A key you've bound in `[keys]` yourself wins over a
  plugin's. `sys.platform == "darwin"` tells you which to pick.
- **id** defaults to the function's name. You can bind any plugin command in
  config.toml as `"plugin:<plugin>:<id>"`, e.g. `"alt-n" = "plugin:notes:notes"`.
- Commands go away when the plugin stops.

## Events

`@app.on(*events)` calls the function with an [`Event`](#event) whenever one of
`events` happens (any event, if you name none):

```python
@app.on("command-finished")
async def finished(event):
    if event["exit"] != 0 and event["duration_ms"] > 60_000:
        await app.notify("Long command failed", event.get("command") or "")
```

| Event | Fields | When |
|---|---|---|
| `pane-opened` / `pane-closed` | `pane` | A pane starts or closes |
| `focus-changed` | `pane` | Another pane gets keyboard focus |
| `title-changed` | `pane`, `title` | A program changes its title |
| `cwd-changed` | `pane`, `cwd` | The shell changes directory\* |
| `bell` | `pane` | A program rings the bell |
| `command-started` | `pane` | A command starts running\* |
| `command-finished` | `pane`, `command`†, `exit`, `duration_ms` | A command finishes\* |

\* Needs [shell integration](shell-integration.md). † Only with the
`screen` permission.

`@app.on_start` runs a function (no arguments) once the plugin is connected,
for setting things up.

Handlers can be `async def` (most are, since talking to Lumen is async) or
plain functions. Each runs on its own, so a handler waiting for you to pick
something doesn't hold up the others. If one raises, the error and traceback
go to the plugin's log and the plugin carries on.

## Talking to Lumen

Everything that talks to Lumen is awaited. A few examples:

```python
pane = await app.focused()                       # the pane with focus
tab = await app.new_tab(cwd="~/src", run="htop") # a new tab; returns its Pane
logs = await pane.split("down", run=["tail", "-f", path], focus=False)
await logs.set_title("logs")

choice = await app.pick([("prod", "Production", "eu-west-1"), ("dev", "Dev")],
                        placeholder="Deploy to…")  # "prod", "dev" or None
name = await app.prompt("Name", initial="my-")  # the text, or None for Esc or empty
if choice and await app.confirm(f"Deploy to {choice}?"):
    await pane.run(["./deploy.sh", choice])

text = await pane.read_screen(lines=20)          # needs `screen`
```

**Typing commands safely:** `pane.run(...)` types a command and presses Enter.
Give it a **list** and each word is quoted for the shell, so a branch or file
name can't run anything:

```python
await pane.run(["git", "switch", "--", branch])   # safe whatever `branch` is
await pane.run(f"git switch {branch}")            # NOT safe: `x;rm -rf ~` is a valid branch
```

The same goes for `run=` in `new_tab` and `split`. If you build a string
yourself, use `lumen_plugin.quote` on anything you didn't write. It's safe in
bash, zsh and fish; Python's `shlex.quote` isn't safe in fish, where `\'`
inside single quotes is an escaped quote.

Text with control characters (newlines, carriage returns, escape codes) or
invisible ones (right-to-left overrides, zero-width spaces, line separators)
is refused with `ValueError` by `run`, `quote` and `run=`: a carriage return
would press Enter partway through, and invisible characters can make a
command look different from what it does. `typeable(text)` tells you in
advance: check it before you show someone a command to confirm, so the
command they approve is the one that runs. If you mean to send keys, use
`pane.send_text`.

**Your plugin's files:** `app.dir` is the plugin's folder, for settings and
files that ship with it (the `snippets` example reads `snippets.toml` from
there). `app.data_dir` is a folder for what it collects (history, caches),
kept across updates and readable only by you. `app.name` is its name.

**Logging:** `print()` goes to the plugin's log, as does `app.log(...)`;
stdout belongs to the protocol, so the package moves `print` out of the way.

## Drawing your own pane

A plugin pane runs no program: you draw it, and get what's typed in it. Use
one for a timer, a dashboard, a game, anything a terminal can show. Ask for
one with `plugin_pane=True`:

```python
view = await pane.split("down", plugin_pane=True, focus=False, size=8)

@view.on_resize                       # also runs once it has its first size
async def redraw():
    await view.draw([f"{view.cols} × {view.rows}", "", "press q to close"])

@view.on_key
async def key(name):                  # "q", "space", "enter", "up", "ctrl-c", …
    if name == "q":
        await view.close()

@view.on_close
def gone():                           # closed by you, by Lumen, or above
    ...
```

`view.write(text)` shows text as if a program had printed it, so escape
sequences work: colours (`\x1b[31m`), moving the cursor (`\x1b[5;10H`),
clearing (`\x1b[2J`), hiding the cursor (`\x1b[?25l`), even mouse reports
(turn them on with `\x1b[?1000h` and read them in `on_input`, which gets
input exactly as sent). `view.draw(lines)` clears and redraws the whole pane
in one go, without flicker. `size` is the new pane's share of the space: a
fraction like `0.3`, or a number of rows (down) or columns (right).

A plugin pane belongs to your plugin: no one else can write to it, it closes
when your plugin stops, and it isn't brought back with the session. Its tab
is named after your plugin until you `set_title`. `new_tab(plugin_pane=True)`
gives a whole tab to it.

**Marking tabs:** `pane.set_tab_style(badge="🍅 24:59", color="red")` puts a
short badge beside the tab's title and tints the tab, which you can see from
any tab. Colours are `red`, `green`, `yellow`, `blue`, `magenta` and `cyan`
from your theme (so they suit it), or `"#rrggbb"`. Pass `None` to remove one;
what you leave out stays as it is. When tabs get narrow the title shortens
first, then the badge: give a `short_badge` such as `"24m"` to show instead,
or it's cut with "…". They're removed when your plugin stops, so a
plugin that crashes or is reloaded leaves no stale badge behind.

The built-in [Pomodoro Timer](../plugins/pomodoro/main.py) uses both, in
about 300 lines. Smaller ones are in [`examples/`](../examples/): a
[todo list](../examples/plugins/todo/main.py) that takes typed text, a
[snake game](../examples/plugins/snake/main.py) that animates,
[tabs tinted by exit status](../examples/plugins/exit-colors/main.py), and
[`watch_pane.py`](../examples/watch_pane.py), a script (not a plugin) with a
pane of its own.

## Permissions

Without any permissions a plugin can list panes, subscribe to events, show
notifications and pickers, and add commands. Anything more goes in
`plugin.toml`, and Lumen asks you to approve it once:

```toml
permissions = ["panes", "input"]
```

| Permission | Allows |
|---|---|
| `panes` | `new_tab`, `split`, `focus`, `close`, `set_title`, `set_tab_style`, `set_pane_style`, `move_tab`, and drawing your own plugin panes |
| `input` | `send_text`, `run`, `run_action`: typing into panes, which can run any command |
| `screen` | `read_screen`, and `command` in `command-finished` events |

A call without the permission raises `LumenError` with `code == -32001` and a
message naming the permission. Ask for what you use and no more: people see
the list when they approve your plugin.

Permissions limit what a plugin can do through Lumen; they aren't a sandbox.
A plugin is a program running as you.

## Scripts outside Lumen

The same package drives Lumen from a script in a pane, a cron job or another
program, through Lumen's [socket](scripting.md#protocol). Scripts don't get
commands (those are for plugins), and they aren't held to permissions (the
socket is yours alone; the package only connects through a folder that's
private to you, as Lumen makes it). Pass `run` an async function for a one-off script; it
ends when the function returns:

```python
# layout.py: PYTHONPATH="$(lumen python-path)" python3 layout.py
from lumen_plugin import Plugin

app = Plugin()

async def main():
    editor = await app.new_tab(cwd="~/src/api", run="nvim .")
    await editor.split("right", run="cargo watch -x test", focus=False)

app.run(main)
```

`lumen python-path` prints the folder holding Lumen's copy of the package. To
install it instead (in a virtualenv): `pip install ./python` from Lumen's
source. A script finds Lumen through `$LUMEN_SOCKET`, set in every pane, or
else uses the most recently started Lumen.

## Debugging

- **Plugin Log: <name>** in the palette opens the log in a new tab, following
  it as it grows. Errors in handlers show up there with tracebacks.
- A plugin that won't start (bad `plugin.toml`, missing program) is listed in
  the banner at the bottom of the window.
- A plugin that crashes is restarted after 1, 2, 5, 10, 30 and then 60
  seconds.
- To try a plugin by hand, run it with `LUMEN_PLUGIN=test python3 main.py`
  and type JSON-RPC at it; or point a script at the socket instead while
  you work out the logic.

## Reference

### Plugin

| | |
|---|---|
| `Plugin()` | The connection. Make one, add commands and handlers, call `run()`. |
| `@command(title, keys=None, id=None)` | Add a palette command; the function gets the focused `Pane` (or None). |
| `@on(*events)` | Handle events; the function gets an `Event`. No events means all. |
| `@on_start` | Run once connected. |
| `run(main=None)` | Connect and handle messages until Lumen goes away. With `main`, run it and stop when it returns. |
| `await call(method, **params)` | Any request; parameters that are None aren't sent. Returns the result. |
| `await request(method, params)` | The same with a dict, sent as it is (None as `null`). |
| `await hello()` | Lumen's version, protocol version, actions and events. |
| `await tabs()` | Every tab with its panes, as `list` returns them. |
| `await panes()` | Every pane as a dict (`id`, `title`, `cwd`, `cols`, `rows`, `last_exit`, `active`, `focused`, `plugin_pane`, `tab`, `window`). |
| `await focused()` | The focused `Pane`, or None. |
| `pane(id)` | A `Pane` for an id. |
| `await new_tab(cwd=None, run=None, plugin_pane=False)` | Open a tab (`~` works in `cwd`); returns its `Pane`, or a `PluginPane` with `plugin_pane=True`. `panes` |
| `await pick(items, placeholder=None, message=None)` | Palette picker; returns the chosen id or None. Items: strings, `(id, label)`, `(id, label, detail)` or dicts. `message` is wrapped text under the input. |
| `await prompt(placeholder=None, message=None, initial="")` | Text input in the palette; returns the trimmed text, or None for Esc or empty. |
| `await confirm(question, message=None, yes="Yes", no="No")` | Yes/no in the palette; Esc is no. |
| `await notify(title, body="")` | Desktop notification. |
| `await run_action(action)` | Run a Lumen action (a `[keys]` name) in the focused pane. `input` |
| `log(*args)` | Write to the plugin's log. |
| `name`, `dir`, `data_dir`, `is_plugin` | The plugin's name, its folder, a private folder for its data, and whether Lumen started it as a plugin. |

### Pane

| | |
|---|---|
| `id` | Its id, as in events and `lumen msg list`. |
| `await info()` | Its entry from `panes()`, or None if it has closed. |
| `await cwd()` | The shell's directory (needs shell integration), or None. |
| `await send_text(text)` | Type text as-is. `input` |
| `await run(command)` | Type a command and press Enter; a list is quoted word by word. `input` |
| `await read_screen(lines=None)` | The screen's text, or its last `lines` lines. `screen` |
| `await split(direction="right", cwd=None, run=None, focus=True, size=None, plugin_pane=False)` | Split it; returns the new `Pane` (a `PluginPane` with `plugin_pane=True`). `size`: a fraction, or rows/columns. `panes` |
| `await set_title(title=None)` | Name its tab; None goes back to the program's title. `panes` |
| `await set_pane_style(label=…, color=…)` | Label (up to 32 characters) drawn above the pane when its tab is split, and a tint (`TAB_COLORS` or `"#rrggbb"`); None removes, left out stays. They go when the plugin stops. `panes` |
| `await set_tab_style(badge=…, color=…, short_badge=None)` | Badge beside its tab's title (up to 24 characters) and a tint (`TAB_COLORS` or `"#rrggbb"`); None removes, left out stays. `short_badge` (up to 8 characters) shows instead when the tab is too narrow. `panes` |
| `await move_tab(position)` | Move its tab to `position` in its window, 1 being first. `panes` |
| `await focus()`, `await close()` | `panes` |
| `await run_action(action)` | Run a Lumen action in this pane. `input` |

### PluginPane

A `Pane` you draw (see [Drawing your own pane](#drawing-your-own-pane)), with:

| | |
|---|---|
| `cols`, `rows` | Its size, kept up to date. |
| `closed` | True once it has closed. |
| `await write(text)` | Show text, escape sequences and all (`\r\n` for a new line). |
| `await draw(lines)` | Clear it and show `lines` (a list, or a string), cursor hidden. |
| `@on_key` | The function gets each key typed, by name (see `keys`). |
| `@on_input` | The function gets input exactly as the terminal sent it. |
| `@on_resize` | The function runs when its size changes, and once it has one. |
| `@on_close` | The function runs once it has closed. |

`keys(text)` names the keys in a piece of input: letters and symbols as
themselves, and `space`, `enter`, `tab`, `shift-tab`, `backspace`, `escape`,
`up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `insert`,
`delete`, `ctrl-a` to `ctrl-z`. Sequences it doesn't know come back as they are.

### Event

`event.name` is the event, `event.pane` the `Pane` it's about (or None), and
the other fields read like a dict: `event["exit"]`, `event.get("command")`,
`event.data` for all of them.

### quote and typeable

`quote(word)` quotes one word for the shell (bash, zsh or fish) and raises
`ValueError` if it has control or invisible characters. Lists given to `run`
and `run=` are quoted with it. `typeable(text)` is False for text with those
characters.

### Errors

`LumenError` (with `.code`) when Lumen refuses a request: `-32001` for a
missing permission, `-32000` when it can't be done (say, the pane closed),
`-32602` for bad parameters. `ConnectionError` once Lumen has gone away;
handlers that hit it just stop.
