<img src="assets/icons/linux/hicolor/128x128/apps/lumen.png" alt="Lumen's icon: a glowing prompt" width="96" height="96">

# Lumen

A GPU-rendered terminal for Linux and macOS. It has tabs, split panes, shell
integration, and plugins you can write in any language.

It's a native app, not a webview: no Electron, no browser engine underneath.
Every glyph is drawn straight to the GPU.

<img src="assets/screenshot.png" alt="Lumen with four tabs and three panes: a shell in a web shop project where npm test failed (its prompt marked in red) and then passed, a Vite dev server, and lumen campfire burning. The tabs carry badges: a green 128 on tests, a blue web-1.staging on deploy, and 3 to do on notes">

## Install

```sh
curl -fsSL https://lumenshell.dev/install | sh
```

It installs for your user, without sudo, on Linux (x86_64 and arm64) and
macOS. Run it again to update.

On macOS you can use Homebrew instead:

```sh
brew install --cask lumenshell/tap/lumen
```

To uninstall, pick a version, or build from source, see
[Installing Lumen](docs/install.md).

## What you get

- **Tabs and split panes.** Drag tabs to reorder them and drag the gaps to
  resize splits. Focus Mode zooms one pane. Every window, tab and split comes
  back next launch.
- **A quick terminal.** It drops down from the top of the screen on one key,
  from any app, and hides again with its shells still running.
  [More](docs/quick-terminal.md)
- **Your shell, understood.** Jump between commands, copy the last command's
  output, and spot failed commands by their red mark. Get a notification when
  a long command finishes while you're elsewhere.
  [More](docs/shell-integration.md)
- **One palette for everything.** Ctrl+Shift+P (⌘⇧P) fuzzy-searches every
  action, theme and plugin command.
- **Select and copy without the mouse.** Move over the screen and scrollback
  with vi keys. [More](docs/keyboard.md#keyboard-select-mode)
- **Scripts that drive it.** `lumen msg` opens tabs and splits, types into
  panes, reads their screens and streams events. [More](docs/scripting.md)
- **Plugins in any language.** Seven come built in, from command history to a
  pomodoro timer. [More](docs/builtin-plugins.md)
- **Themes for the whole window.** 24 built in, from Rosé Pine, Catppuccin
  and Tokyo Night to Gruvbox, Nord and Solarized, 8 of them light. Each
  recolours the terminal and the chrome around it. Add your own, or use any
  Ghostty theme file. [More](docs/config.md#themes)

It also handles CJK input methods, the kitty keyboard protocol, clickable
links, mouse reporting for vim and tmux, and colour emoji. See
[everything Lumen does](docs/features.md).

## Screenshots

**Tabs with badges and colours**, set by plugins and scripts: a pomodoro
countdown, a staging server, what's left to do.

<img src="assets/screenshots/tabs.png" alt="The tab bar: shop with three panes, tests with a green 128 badge, deploy with a blue web-1.staging badge, and notes badged 3 to do">

<table>
<tr>
<td width="50%"><b>Command palette</b>: every action, theme and plugin command, fuzzy-searched, with its shortcut.<br><br><img src="assets/screenshots/palette.png" alt="The command palette over a git log, filtered to split: Split Right and Split Down with their shortcuts"></td>
<td width="50%"><b>Find in scrollback</b>, with every match highlighted.<br><br><img src="assets/screenshots/find.png" alt="A git log with file stats and the find bar searching for checkout: 6 of 6 matches highlighted"></td>
</tr>
<tr>
<td><b>Keyboard select mode</b>: move with vi keys and select by character, line or block.<br><br><img src="assets/screenshots/select.png" alt="A failed test run with two lines selected in keyboard select mode, and a hint saying y copies and Esc cancels"></td>
<td><b>Pomodoro Timer</b>, a built-in plugin: a big countdown in a strip under your work, with the time on the tab.<br><br><img src="assets/screenshots/pomodoro.png" alt="A git log above a pomodoro pane counting down from 25 minutes, with the countdown on the tab"></td>
</tr>
<tr>
<td><b>Plugin panes</b>: a todo list drawn by a plugin, beside your shell, with what's left on the tab.<br><br><img src="assets/screenshots/todo.png" alt="A git log beside a todo list pane: one item ticked off and three left, with 3 to do on the tab"></td>
<td><b>Plugin Gallery</b>: the built-in plugins, off until you turn them on.<br><br><img src="assets/screenshots/gallery.png" alt="The Plugin Gallery listing Recent Directories, Command History, Switch Git Branch, SSH to Host, Projects &amp; Tasks, Build Status and Pomodoro Timer, which is on"></td>
</tr>
<tr>
<td><b><code>watch_pane.py</code></b>, an example script: <code>watch</code> in a pane of its own, with changes highlighted.<br><br><img src="assets/screenshots/watch.png" alt="A shell above a watch pane rerunning a status script every second, with the changed time highlighted"></td>
<td><b><code>lumen campfire</code></b>: a pixel-art fire in a pane, for cold coding sessions.<br><br><img src="assets/screenshots/campfire.png" alt="A shell with a passing test run beside a pane with a pixel-art campfire, its sparks drifting up"></td>
</tr>
<tr>
<td colspan="2"><b>Rosé Pine Dawn</b>, the light theme: every theme recolours the window around the terminal too.<br><br><img src="assets/screenshots/light.png" alt="The main layout in a light theme: a shell with test runs beside a Vite dev server, and tabs with badges"></td>
</tr>
</table>

## The keys to know

| Action              | Linux                | macOS            |
|---------------------|----------------------|------------------|
| Command palette     | Ctrl+Shift+P         | ⌘⇧P              |
| New tab             | Ctrl+T               | ⌘T               |
| Split right / down  | Ctrl+Shift+D / E     | ⌘D / ⌘⇧D         |
| Close pane          | Ctrl+Shift+W         | ⌘W               |
| Move between panes  | Ctrl+Arrows          | ⌘Arrows          |
| Find in scrollback  | Ctrl+Shift+F         | ⌘F               |
| Prev / next command | Ctrl+Shift+↑ / ↓     | ⌘⇧↑ / ⌘⇧↓        |

The rest are in [Keyboard](docs/keyboard.md). You can change any of them in
the config.

## Config

Settings live in `~/.config/lumen/config.toml`. Lumen writes it, with
comments, the first time it runs, and applies your changes the moment you
save.

```toml
theme = "Tokyo Night"
font-size = 14
background-opacity = 0.9

[keys]
ctrl-t = "none"   # give Ctrl+T back to the shell
```

Every setting and key binding is in [Config](docs/config.md).

## Scripting

Inside a Lumen pane, `lumen msg` acts on that pane:

```sh
p=$(lumen msg split right --run 'npm run dev')   # prints the new pane's id
lumen msg tab-style --badge prod --color red
lumen msg watch command-finished                 # events as JSON lines
```

[Scripting](docs/scripting.md) covers every command, the events and the
JSON-RPC protocol. [`examples/`](examples/) has ready-made scripts.

## Plugins

Turn on the built-in plugins from **Plugin Gallery** in the palette. Each one
shows what it does and what it's allowed to do before you approve it.

To write your own in Python, use `lumen_plugin`. It comes with Lumen, so
there's nothing to install:

```python
from lumen_plugin import Plugin

app = Plugin()

@app.command("Switch Git Branch", keys="alt-b")
async def switch(pane):
    branch = await app.pick(["main", "dev"], placeholder="Switch to…")
    if branch:
        await pane.run(["git", "switch", "--", branch])   # a list is quoted for the shell

app.run()
```

- [Built-in plugins](docs/builtin-plugins.md): what each one does
- [Plugin guide](docs/plugins.md): writing plugins in Python
- [Plugin protocol](docs/plugin-protocol.md): writing them in any other language

## Two extras

`lumen info` shows your system beside a flickering flame in your theme's
colours. `lumen campfire` fills a pane with a crackling fire, for cold coding
sessions. See [Commands](docs/commands.md).

## Documentation

| Guide | Covers |
|---|---|
| [Installing Lumen](docs/install.md) | Installer options, Homebrew, release files, building from source |
| [Features](docs/features.md) | Everything Lumen does |
| [Keyboard](docs/keyboard.md) | All shortcuts, the find bar, the palette, keyboard select mode |
| [Config](docs/config.md) | Every setting and key binding |
| [Quick terminal](docs/quick-terminal.md) | Setting it up on X11, Wayland and macOS |
| [Shell integration](docs/shell-integration.md) | What it adds, how it loads, using it over SSH |
| [Commands](docs/commands.md) | `lumen info` and `lumen campfire` |
| [Scripting](docs/scripting.md) | `lumen msg`, events and the socket protocol |
| [Built-in plugins](docs/builtin-plugins.md) | The seven plugins in the Plugin Gallery |
| [Plugin guide](docs/plugins.md) | Writing plugins in Python |
| [Plugin protocol](docs/plugin-protocol.md) | Writing plugins in any language, and permissions |
| [Files and session](docs/files.md) | Where Lumen keeps things, and its environment variables |
| [Working on Lumen](docs/development.md) | Building, tests, source layout, releasing |

## License

Lumen is available under either the [MIT License](LICENSE-MIT) or the
[Apache License 2.0](LICENSE-APACHE), at your option. Unless you say otherwise,
anything you contribute is offered under the same terms.

The bundled fonts, JetBrains Mono and Inter, keep their own license, the SIL
Open Font License ([JetBrains Mono](assets/fonts/JetBrainsMono-OFL.txt),
[Inter](assets/fonts/Inter-LICENSE.txt)).
