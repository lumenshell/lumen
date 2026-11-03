# Lumen

A fast, good-looking terminal for Linux and macOS, built with Rust,
[GPUI](https://gpui.rs) (Zed's GPU UI framework) and
[`alacritty_terminal`](https://crates.io/crates/alacritty_terminal).

## Features

- GPU-rendered terminal grid: truecolor, bold/italic, underline/undercurl,
  strikethrough, wide (CJK) glyphs, colour emoji, configurable scrollback
- Box-drawing, block and powerline characters drawn as shapes, so lines and
  prompt segments join without gaps
- Tabs in a custom title bar, with pane counts and inherited working directory.
  A tab shows the title its program sets until you rename it (double-click the
  tab, or Ctrl+, / ⌘,); **+** opens a tab and a tab's **×**
  closes it with all its panes. Drag the title bar to move the window and
  double-click it to maximize. Closing the last pane quits Lumen
- Split panes on a padded canvas: rounded frames, an accent focus halo, and
  dimmed unfocused panes; Focus Mode zooms the active pane
- Drag the gap between panes to resize a split; double-click it to even the split out
- Your layout comes back next launch: tabs, splits and their sizes, each pane's
  working directory, which pane had focus, tab names you gave, and the window's
  size, position and maximized state
- Command palette (fuzzy search over every action and theme)
- Find in scrollback, with optional case-sensitive and regex matching
- Keyboard select mode: move through the screen and scrollback with vi keys,
  select and copy without the mouse (see [Keyboard select mode](#keyboard-select-mode))
- `lumen info`: a neofetch-style summary of your system and this Lumen, beside
  a flickering flame drawn in your theme's colours
- Scriptable: `lumen msg` opens tabs and splits, types into panes, reads their
  screens and streams events such as "command finished with exit 1" (see
  [Scripting](#scripting-lumen-msg))
- Built-in plugins to turn on from the Plugin Gallery: jump to recent folders,
  search every command you've run (and rerun the last failure), switch git
  branches, SSH to a host, open project layouts, build status in the tab title
- Plugins in any language: Lumen runs them, restarts them if they crash, and
  lets them add palette commands, key bindings and pickers, limited to the
  permissions you approve. Python plugins get `lumen_plugin`, built in, with
  nothing to install (see [Plugins](#plugins) and the [plugin guide](docs/plugins.md))
- Shell integration for bash, zsh and fish: jump between commands, copy the last
  command's output, a red mark beside commands that failed, and new panes that
  open in the shell's directory (see [Shell integration](#shell-integration))
- A desktop notification when a long command finishes while you're looking
  elsewhere, and a green or red dot on its tab until you go back to it
- Themes that recolour both the terminal and the chrome: Rosé Pine, Rosé Pine
  Moon, Catppuccin Mocha, Tokyo Night, Rosé Pine Dawn (light)
- Mouse selection (drag, double-click word, triple-click line, Shift+click to
  extend), primary-selection copy and middle-click paste on Linux, bracketed paste
- The wheel scrolls history, or in full-screen programs such as `less` and
  `man` (which don't use the mouse) it moves through the text with arrow keys
- Clickable links: hold Ctrl (⌘ on macOS) to underline URLs and OSC 8 hyperlinks
  under the pointer, with the target shown at the bottom of the pane; click to
  open. Plain-text URLs with common schemes open; OSC 8 links, whose target is
  hidden behind other text, only open http, https and mailto
- Mouse reporting for vim, tmux, htop and friends: clicks, drags, motion and the
  wheel in X10, UTF-8 and SGR formats, plus focus in/out events. Hold Shift to
  select text locally instead
- Programs may set the clipboard (OSC 52, used by tmux and vim over SSH) but
  can't read it
- Bundled JetBrains Mono and Inter, so it looks the same everywhere
- A commented `config.toml` that applies changes as soon as you save it

## Build

```sh
# Debian/Ubuntu build dependencies (libnotify-bin is for notifications)
sudo apt install libxkbcommon-x11-dev libxcb1-dev libfreetype-dev libfontconfig-dev \
                 libwayland-dev libvulkan1 libnotify-bin

cargo run --release

# or install it, which also puts `lumen msg` on your PATH (~/.cargo/bin)
cargo install --locked --path .
```

`lumen` opens a window; `lumen msg ...` controls a running one (see
[Scripting](#scripting-lumen-msg)); `lumen --help` lists both.

On macOS you only need Xcode command-line tools.

## Shortcuts

| Action             | Linux                | macOS            |
|--------------------|----------------------|------------------|
| Command palette    | Ctrl+Shift+P         | ⌘⇧P              |
| New tab            | Ctrl+Shift+T         | ⌘T               |
| Split right / down | Ctrl+Shift+D / E     | ⌘D / ⌘⇧D         |
| Close pane         | Ctrl+Shift+W         | ⌘W               |
| Focus pane         | Alt+Shift+Arrows     | ⌘⌥Arrows         |
| Next pane          | Ctrl+Shift+N         | ⌘]               |
| Focus Mode (zoom)  | Ctrl+Shift+Enter     | ⌘⇧Enter          |
| Next tab           | Ctrl+Tab / Ctrl+PgDn | ⌘⇧]              |
| Previous tab       | Ctrl+Shift+Tab / Ctrl+PgUp | ⌘⇧[        |
| Go to tab N        | Ctrl+1…9 or Alt+1…9  | ⌘1…9             |
| Rename tab         | Ctrl+, or double-click the tab | ⌘, or double-click |
| Cycle theme        | Ctrl+Shift+Y         | ⌘⇧Y              |
| Font size          | Ctrl+= / Ctrl+- / 0  | ⌘= / ⌘- / ⌘0     |
| Copy / paste       | Ctrl+Shift+C / V, Shift+Insert | ⌘C / ⌘V |
| Scroll a page      | Shift+PgUp / PgDn    | Shift+PgUp / PgDn|
| Open link          | Ctrl+click           | ⌘+click          |
| Find in scrollback | Ctrl+Shift+F         | ⌘F               |
| Prev / next command | Ctrl+Shift+↑ / ↓    | ⌘↑ / ⌘↓          |
| Copy last output   | Ctrl+Shift+O         | ⌘⇧O              |
| Keyboard select mode | Ctrl+Shift+Space   | ⌘⇧Space          |
| Quit               | Ctrl+Shift+Q         | ⌘Q               |

In the find bar, Enter or ↑ goes to the older match and Shift+Enter or ↓ to
the newer one. Alt+C toggles case-sensitive matching (otherwise case is ignored
unless the query has a capital letter), Alt+R toggles regular expressions, and
Esc closes it.

The command jumps and Copy last output need [shell integration](#shell-integration).

The command palette searches every action above plus **Open Config File**, one
**Theme:** entry per theme, the commands plugins add, **Plugin Log:** (or
**Allow Plugin:**) for each plugin, **Plugin Gallery** and **Reload Plugins**, showing each
action's current shortcut. Type
to filter (Ctrl+U clears it), ↑/↓ or Tab to choose (the list scrolls), Enter to
run, Esc or a click outside to close.
Focus left/right/up/down and Go to tab N are keyboard-only.

Rename Tab (Ctrl+, / ⌘, or double-click a tab) asks for a name in the palette,
filled in with the name you gave it, if any. Enter renames it, Ctrl+U clears the
line, Esc keeps the old name, and an empty name goes back to the title the program sets. Names are saved with
the session; `lumen msg set-title` sets the same name from a script.

Every shortcut can be changed in the config (see [Key bindings](#key-bindings))
except Shift+PgUp/PgDn, Ctrl+click and the keys inside the find bar, palette
and keyboard select mode.

## Keyboard select mode

Ctrl+Shift+Space (⌘⇧Space) puts a pane into select mode: a second, accent
coloured cursor you move with vi keys over the screen and scrollback, while a
hint at the bottom of the pane shows what the next keys do. Nothing you type
reaches the shell until you leave.

| Keys | Move |
|---|---|
| `h` `j` `k` `l` or arrows | Left, down, up, right |
| `w` `b` `e` | Next word, previous word, end of word (`W` `B` `E`: words split only at spaces) |
| `0` `^` `$` (or Home, End) | Start of line, first non-blank, end of line |
| `H` `M` `L` | Top, middle, bottom of the screen |
| `gg` `G` | Oldest line of history, last line of output |
| `Ctrl+U` `Ctrl+D`, `Ctrl+B` `Ctrl+F` (or PgUp, PgDn) | Half a page, a page, up or down |
| `{` `}`, `%` | Previous or next blank line, matching bracket |
| `[` `]` | Previous or next command's prompt (needs [shell integration](#shell-integration)) |
| `/` `?`, then `n` `N` | Find (down or up) with the find bar, then the next or previous match |

A number before a move repeats it: `5j`, `3w`, `2[`.

| Keys | Select and copy |
|---|---|
| `v`, `V`, `Ctrl+V` | Start a selection of characters, whole lines or a block; the same key again ends it |
| `y` or Enter | Copy the selection and leave |
| `Y` | Copy the line under the cursor and leave |
| Esc | Drop the selection; with none, leave (`q`, `i` and Ctrl+C also leave) |

Clicking moves the cursor there, and a mouse selection can be copied with `y`
too. Copying also fills the primary selection on Linux, so middle-click pastes
it. Leaving scrolls back to the bottom.

## Config

Settings live in `~/.config/lumen/config.toml`, created with comments on first
run (open it from the command palette with **Open Config File**). Changes apply
as soon as you save; no restart needed.

```toml
theme = "Rosé Pine"            # or "Rosé Pine Moon", "Catppuccin Mocha", "Tokyo Night", "Rosé Pine Dawn"
font-family = "JetBrains Mono" # any installed monospace font; JetBrains Mono is built in
font-size = 14
line-height = 1.35             # multiple of the font size
padding = 12                   # pixels between a pane's edge and its text
scrollback = 10000             # lines of history per pane
cursor-style = "block"         # "block", "beam" or "underline", until a program picks one
shell = "/usr/bin/fish --login" # program for new panes; defaults to $SHELL
shell-integration = true       # mark prompts and commands in bash, zsh and fish
notify-after = 10              # seconds a command must run to notify when it ends; 0 = off
socket-api = true              # let scripts control Lumen with `lumen msg`
plugins = ["recent-dirs"]      # built-in plugins to run (see Plugins); the Plugin Gallery edits this
```

Delete a line to go back to its default. Mistakes (a typo'd key, a bad value, a
font that isn't installed) are listed in a banner at the bottom of the window
while the rest of the file still applies; a file that can't be parsed at all
keeps the previous settings. Switching theme or zooming the font from inside
Lumen edits just those keys and keeps your comments.

`shell` is split on spaces, so an argument can't contain a space (wrap it in a
script instead). `shell` and `shell-integration` apply to new panes, and
turning `socket-api` on takes a restart (turning it off applies at once);
everything else applies to open ones too. Older
versions used `~/.config/lumen/config`; its values are carried over the first
time `config.toml` is created.

## Key bindings

Change shortcuts in a `[keys]` table at the end of `config.toml`. Each line
binds a key to an action by name, and applies as soon as you save:

```toml
[keys]
ctrl-alt-t = "new-tab"      # an extra key for New Tab; Ctrl+Shift+T still works
ctrl-tab = "none"           # free Ctrl+Tab for the desktop or the shell
"ctrl-=" = "increase-font"  # keys with symbols need quotes
```

A key you list replaces whatever it did by default; `"none"` frees it, so it
goes to the program in the terminal instead. An action's other default keys
keep working, so to move one, bind the new key and set the old one to
`"none"`. Keys are case-insensitive and written like `ctrl-shift-t`, `alt-1`,
`cmd-k` (macOS), `f5`, `shift-insert` or `ctrl-pageup`. Multi-key sequences
such as `ctrl-a n` aren't supported yet. Typos are listed in the banner, with a
suggestion when an action name is close.

Plugin commands can be bound too, as `plugin:<plugin>:<command id>`, e.g.
`alt-g = "plugin:git-branch:switch"`; that also overrides a key the plugin
asked for.

Actions: `new-tab`, `close-pane`, `split-right`, `split-down`, `next-tab`,
`prev-tab`, `tab-1` … `tab-9`, `rename-tab`, `next-pane`, `focus-left`, `focus-right`,
`focus-up`, `focus-down`, `toggle-zoom`, `command-palette`, `next-theme`,
`increase-font`, `decrease-font`, `reset-font`, `copy`, `paste`, `find`,
`prev-command`, `next-command`, `copy-last-output`, `select-mode`, `quit`.

## Shell integration

When a pane runs bash, zsh or fish, Lumen loads a small script into it that
marks where each prompt, command and its output begins, and reports the
shell's directory. That gives you:

- **Jump between commands** with Ctrl+Shift+↑ / ↓ (⌘↑ / ⌘↓). The prompt you
  jumped to stays highlighted until you type; jumping past the newest one
  returns to the bottom.
- **Copy the last command's output** with Ctrl+Shift+O (⌘⇧O). It's selected
  too, so you can see what was copied.
- **Failure marks**: a bar in the left margin beside each prompt, red if the
  command run there failed and grey if it succeeded.
- **Notifications for long commands**: when a command that ran for at least
  `notify-after` seconds (10 by default) finishes and you're not looking at
  its pane (another pane or tab is focused, or Lumen isn't the active window),
  you get a desktop notification such as "✗ make test — Failed with exit 2
  after 3m 12s · ~/proj". If its pane isn't focused, its tab also gets a dot,
  green for success and red for failure, until you visit it. On Linux this
  uses `notify-send` (`sudo apt install libnotify-bin`); on macOS,
  Notification Center. Set `notify-after = 0` to turn it off.
- **New panes open where you are**, using the directory the shell reports. This
  works on macOS too; without it, Lumen reads it from `/proc` on Linux only.

Your own startup files (`~/.bashrc`, `~/.zshrc`, `~/.config/fish`) still load
as usual. The scripts are copied to `~/.local/share/lumen/shell-integration`
(`$XDG_DATA_HOME`, or `~/Library/Application Support/lumen/shell-integration`
on macOS) and loaded with `--rcfile`
for bash, `ZDOTDIR` for zsh and `XDG_DATA_DIRS` for fish.

It doesn't load for other shells, or for bash started as a login shell
(`bash -l`), because `--rcfile` doesn't apply there. To use it in those cases,
or on a remote machine over SSH, source the script yourself: add
`source /path/to/lumen.bash` to the end of `~/.bashrc`, source `lumen.zsh` from
`~/.zshrc`, or copy `lumen.fish` into `~/.config/fish/conf.d/`. The scripts are
in [`shell-integration/`](shell-integration). Directories reported by a remote
host are ignored, so new panes still open locally.

Set `shell-integration = false` in the config to turn it off.

## System info (`lumen info`)

`lumen info` prints a flame in your theme's colours beside your OS, kernel,
uptime, shell, CPU, GPU, memory and public IP, and Lumen's own details: version and
window size, theme, font, whether shell integration is on, and which plugins
are set up. Below them are the terminal's 16 colours. It takes a few
milliseconds and needs no other programs (GPU names come from `lspci` or
NVIDIA's driver when they're there).

The public IP comes from asking [api.ipify.org](https://www.ipify.org) (over
plain HTTP, and shown only if the answer is an IP address). The rest prints
at once and the IP fills in when it arrives, or reads "unavailable" when
offline. `--no-ip` skips it, so nothing leaves your machine.

In a terminal the flame burns until you press any key (the key still reaches
your shell; Ctrl+C works too). `--brief` burns for two seconds and then
settles, which suits your shell's startup: add `lumen info --brief` to your
`~/.bashrc`, `~/.zshrc` or `config.fish` to see it in every new pane.
`--still` doesn't animate at all, and it doesn't animate when the output
wouldn't fit on screen. Piped or redirected,
or with `NO_COLOR=1`, it prints plain text.

## Scripting (`lumen msg`)

Lumen can be driven from scripts, shell aliases and plugins. `lumen msg`
is the same `lumen` binary, so it needs to be on your PATH (`cargo install
--locked --path .` does that). Inside a Lumen pane, `lumen msg` talks to that Lumen and acts on that pane by default;
from anywhere else it finds the most recently started Lumen. From Python,
`lumen_plugin` does the same with less ceremony; see
[Scripts outside Lumen](docs/plugins.md#scripts-outside-lumen).

```sh
lumen msg list                          # tabs and panes with their ids
p=$(lumen msg split right --run 'npm run dev')   # prints the new pane's id
lumen msg split down --pane "$p" --cwd ~/api --run 'cargo watch -x test' --no-focus
lumen msg send-text --pane "$p" 'q\n'   # type into a pane (\n is Enter)
lumen msg read-screen --pane "$p" --lines 20
lumen msg set-title "Servers"           # this pane's tab; no title resets it
lumen msg action toggle-zoom            # any action from Key bindings
lumen msg notify "Deploy done" "web-3 is live"
lumen msg watch command-finished        # events as JSON lines, until Ctrl+C
```

| Command | Does |
|---|---|
| `list [--json]` | Tabs and panes: ids, titles, directories, sizes, last exit status |
| `new-tab [--cwd DIR] [--run CMD]` | Open a tab; prints its pane id |
| `split [right\|down] [--pane ID] [--cwd DIR] [--run CMD] [--no-focus]` | Split a pane; prints the new pane id |
| `focus [ID]`, `close [ID]` | Show and focus, or close, a pane |
| `send-text [--pane ID] TEXT` | Type text; `\n`, `\r`, `\t`, `\e` and `\\` are understood |
| `read-screen [--pane ID] [--lines N]` | Print the screen, or the last N lines including history |
| `set-title [--pane ID] [TITLE]` | Name a tab; without TITLE the program's title is shown again |
| `notify TITLE [BODY]` | Desktop notification (`notify-send` on Linux) |
| `action NAME [--pane ID]` | Run an action such as `split-right` or `find` |
| `watch [EVENT...]` | Stream events (all of them, or the ones named) |
| `call METHOD [JSON]` | Send any request and print the raw result |

`--run` types the command into the new pane's shell, so the shell stays open
after it finishes. A new pane starts in the directory of the pane it was split
from (or the current one, for tabs) unless `--cwd` says otherwise. Tab titles
set this way aren't kept between sessions.

More complete scripts (a project layout, run-and-wait, git branch tab titles,
broadcasting, grabbing another pane's output, and a Python client using the
socket directly) are in [`examples/`](examples/).

For example, a notification whenever a command fails in any pane (needs
[`jq`](https://jqlang.org)):

```sh
lumen msg watch command-finished |
  jq --unbuffered -r 'select(.exit != 0) | "\(.command): exit \(.exit)"' |
  while read -r msg; do lumen msg notify "Command failed" "$msg"; done
```

### Events

| Event | Fields | When |
|---|---|---|
| `pane-opened` / `pane-closed` | `pane` | A pane starts or closes |
| `focus-changed` | `pane` | Another pane gets keyboard focus |
| `title-changed` | `pane`, `title` | A program changes its title |
| `cwd-changed` | `pane`, `cwd` | The shell changes directory\* |
| `bell` | `pane` | A program rings the bell |
| `command-started` | `pane` | A command starts running\* |
| `command-finished` | `pane`, `command`, `exit`, `duration_ms` | A command finishes: what was typed, its exit status and how long it ran\* |

\* Needs [shell integration](#shell-integration).

`focus-changed` is sent when the window next redraws, so it can trail the
change slightly, and not at all while the window is minimized.

### Protocol

`lumen msg` is a thin client; anything that can open a Unix socket can do the
same. Each Lumen listens on `lumen-<pid>.sock` in `$XDG_RUNTIME_DIR/lumen`
(else `$TMPDIR/lumen-<uid>`), and every pane gets its path in `LUMEN_SOCKET`
and its own id in `LUMEN_PANE`. Messages are [JSON-RPC 2.0](https://www.jsonrpc.org/specification),
one per line:

```sh
echo '{"jsonrpc":"2.0","id":1,"method":"split","params":{"direction":"down","run":"htop"}}' |
  socat - UNIX-CONNECT:"$LUMEN_SOCKET"
# {"jsonrpc":"2.0","id":1,"result":{"pane":4294967299}}
```

Methods: `hello` (protocol version, Lumen version, actions and events),
`show-picker` (`items`: a list of `{id, label, detail}`, `placeholder`, and
`message`, text shown under the input; answers
`{"id": ...}` with the choice, or `null` if closed),
`list`, `new-tab` (`cwd`, `run`), `split` (`pane`, `direction`, `cwd`, `run`,
`focus`), `focus`, `close`, `send-text` (`pane`, `text`), `read-screen`
(`pane`, `lines`), `set-title` (`pane`, `title`), `notify` (`title`, `body`),
`run-action` (`action`, `pane`) and `subscribe` (`events`, a list; omit for
all). `pane` is optional everywhere and means the focused pane. After
`subscribe`, the connection also receives events as notifications:
`{"jsonrpc":"2.0","method":"event","params":{"event":"bell","pane":4294967298}}`.
Errors use the standard JSON-RPC codes, with `-32000` for requests that can't
be carried out (such as an unknown pane). The protocol version (`hello`) goes
up only when a change could break existing clients.

The socket and its directory are readable only by you. Anything running as
you can use it to type into panes and read their screens, so turn it off with
`socket-api = false` if that's a concern.

## Plugins

### Built-in plugins

Lumen comes with plugins that are off until you turn them on: choose **Plugin
Gallery** in the palette, pick one to see what it does and what it's allowed
to do, and choose **Turn on**. Choosing it there is your approval; it starts at
once and whenever Lumen opens. They're listed under `plugins` in config.toml,
so they travel with your dotfiles (a name added there by hand still asks for
approval the first time).

| Plugin | What it does | Permissions |
|---|---|---|
| **Recent Directories** (`recent-dirs`) | Remembers the folders your shells visit. **Go to Recent Directory** (Ctrl+Shift+J, ⌘⇧J) lists them, most used and most recent first, and `cd`s the pane to the one you pick (or opens it in a new tab if the pane is busy). | `input`, `panes` |
| **Command History** (`command-history`) | Keeps every command from every pane and session with its exit status, duration and folder. **Search Command History** (Ctrl+Shift+H, ⌘⇧H) types the one you pick without pressing Enter; **Rerun Last Failed Command** (Ctrl+Shift+R, ⌘⇧R) shows the last failure and its folder and, once you confirm, runs it again there; **Clear Command History** forgets it all. Commands starting with a space aren't kept. | `screen`, `input` |
| **Switch Git Branch** (`git-branch`) | Alt+B lists the branches of the pane's repository, newest first, and switches to the one you pick. | `input` |
| **SSH to Host** (`ssh-hosts`) | Ctrl+Shift+S (⌘⇧S) lists the hosts in `~/.ssh/config` (and files it `Include`s) and opens the one you pick in a new tab named after it. | `panes` |
| **Project Layouts** (`project-layout`) | Ctrl+Shift+L (⌘⇧L) finds `.lumen-layout.toml` in the pane's folder or above, shows you every command it would run, and opens its tab and splits ([format](plugins/project-layout/main.py)). | `panes` |
| **Build Status** (`build-status`) | When a build or test (`cargo test`, `make`, `npm test`, `pytest`, …) finishes, the tab title shows how it went, like `✓ cargo test · 12s`, until the next command. Leaves tabs you've named alone. | `screen`, `panes` |

All of them need [shell integration](#shell-integration) to know folders and
commands. Their data (history, folders) is kept in
`~/.local/state/lumen/plugin-data/`, readable only by you. A folder of your own
in the plugins folder with the same name runs instead of the built-in one, so
you can copy one from [`plugins/`](plugins/) and change it.

### Writing plugins

A plugin is a program in its own folder under `~/.config/lumen/plugins/`
(`$XDG_CONFIG_HOME/lumen/plugins`), written in any language. For Python,
Lumen comes with `lumen_plugin`, which needs no installing; see the
**[plugin guide](docs/plugins.md)** to write one in a few lines:

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

The rest of this section is the protocol underneath, for other languages.

Lumen starts every plugin when it opens, restarts one that exits (waiting 1, 2, 5, 10, 30 and then
60 seconds after quick crashes), and stops them all when it quits. Each runs
as its own process, so a slow or crashing plugin can't freeze Lumen.

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

A plugin talks to Lumen over its **stdin and stdout**, using the same
JSON-RPC 2.0 messages as the [socket API](#protocol), one per line. Anything it
writes to **stderr** goes to its log, which **Plugin Log: <name>** in the
palette opens in a new tab. It should exit when stdin closes. It gets
`LUMEN_PLUGIN` (its name), `LUMEN_PLUGIN_DIR` (its folder), `LUMEN_PLUGIN_DATA`
(a folder for its own data, kept across updates and readable only by you) in
its environment, and `PYTHONPATH` starting with the folder holding
`lumen_plugin`.

Besides the socket API's methods, plugins can:

- **Add palette commands**: `register-command` with `id`, `title` and
  optionally `keys` (e.g. `"alt-b"`). When you choose one, Lumen sends the
  plugin `{"jsonrpc":"2.0","method":"command","params":{"id":"switch","pane":4294967298}}`,
  where `pane` is the focused pane. Commands go away when the plugin stops.
- **Ask you to choose**: `show-picker` opens the palette with the plugin's
  items and answers with the one you pick (or `null` for Esc).

**Permissions.** Without any, a plugin can list panes, subscribe to events,
show notifications, add commands and show pickers. Anything more is declared
in `plugin.toml` and needs your approval, asked once in the palette the first
time Lumen sees the plugin (and again if its permissions or command change):

| Permission | Allows |
|---|---|
| `panes` | `new-tab`, `split`, `focus`, `close`, `set-title` |
| `input` | `send-text`, `run-action`: typing into panes, which can run any command |
| `screen` | `read-screen`, and the command text in `command-finished` events |

Your answers are kept in `~/.local/state/lumen/plugin-approvals.json`, outside
the plugins folder so a plugin can't approve itself. Choose **Allow Plugin:
<name>** in the palette to change your mind about one you refused; delete its
entry from that file to be asked again. A plugin missing a permission gets a
JSON-RPC error saying which one to add.

Permissions limit what a plugin can do *through Lumen*; they aren't a sandbox.
A plugin is a program running as you, so only install ones you trust, as you
would any other program.

When a plugin types into a shell with `send-text`, anything it didn't write
itself (a branch or file name, text from the screen) must be quoted for the
shell first, and must not contain control characters (a carriage return
presses Enter) or invisible ones such as right-to-left overrides, which make
a command look different from what it does. Git branch names, for one, can contain `$`, `;` and backticks.
Quote in a way fish also understands: POSIX single quotes aren't enough there,
since fish reads `\'` inside them as an escaped quote (`lumen_plugin.quote`
handles this; so does avoiding the shell, e.g. `new-tab`'s `cwd`).

Problems in a `plugin.toml` are listed in the banner at the bottom of the
window; fix them and choose **Reload Plugins** (which also picks up new and
changed plugins). To learn from: the built-in plugins in [`plugins/`](plugins/)
(all Python), and [`examples/plugins/`](examples/plugins/): `hello` (bash
speaking the raw protocol) and `snippets` (a snippet library).

## Session

The pane layout is saved to `~/.local/state/lumen/session.json`
(`$XDG_STATE_HOME`, or `~/Library/Application Support/lumen` on macOS) when you
quit or close the window, and every few seconds while Lumen runs. Tab names you
gave are kept. Shells start fresh in their saved directories; scrollback isn't kept. If you `exit` every
shell, the session is cleared and the next launch starts with one tab. Delete
the file to reset.

The window reopens at its saved size and position, maximized or fullscreen if it
was. If the saved spot would leave the title bar off every connected monitor
(say a monitor was unplugged), the window manager places it instead. On Wayland,
apps can't position their own windows, so only the size and maximized state
come back.

## Files and environment

| What           | Where (Linux)                                   | macOS                                               |
|----------------|-------------------------------------------------|-----------------------------------------------------|
| Settings       | `~/.config/lumen/config.toml` (`$XDG_CONFIG_HOME`) | same                                             |
| Session        | `~/.local/state/lumen/session.json` (`$XDG_STATE_HOME`) | `~/Library/Application Support/lumen/session.json` |
| Shell scripts  | `~/.local/share/lumen/shell-integration/` (`$XDG_DATA_HOME`) | `~/Library/Application Support/lumen/shell-integration/` |
| Socket         | `$XDG_RUNTIME_DIR/lumen/lumen-<pid>.sock`       | `$TMPDIR/lumen-<uid>/lumen-<pid>.sock`              |
| Plugins        | `~/.config/lumen/plugins/<name>/` (`$XDG_CONFIG_HOME`) | same                                         |
| Plugin logs and approvals | `~/.local/state/lumen/plugins/<name>.log`, `plugin-approvals.json` | `~/Library/Application Support/lumen/…` |
| Python package (`lumen python-path`) | `~/.local/state/lumen/python/lumen_plugin/` | `~/Library/Application Support/lumen/python/…` |
| Built-in plugins (written out by Lumen) and plugin data | `~/.local/state/lumen/builtin-plugins/`, `plugin-data/<name>/` | `~/Library/Application Support/lumen/…` |

Shells started by Lumen get `TERM=xterm-256color`, `COLORTERM=truecolor` and
`TERM_PROGRAM=lumen`, so scripts can tell they're running in Lumen, plus
`LUMEN_SOCKET` and `LUMEN_PANE` for [`lumen msg`](#scripting-lumen-msg) when the
socket API is on.

## Layout

| File                   | What it does                                              |
|------------------------|-----------------------------------------------------------|
| `src/main.rs`          | Actions, fonts, window creation                           |
| `src/keymap.rs`        | Default shortcuts and the `[keys]` table                  |
| `src/api.rs`           | Socket API server: JSON-RPC requests and the event stream |
| `src/cli.rs`           | The `lumen msg` command                                   |
| `src/info.rs`          | The `lumen info` command                                  |
| `src/plugins.rs`       | Plugin manifests, approvals, running and restarting them  |
| `src/workspace.rs`     | Title bar, tabs, pane tree, focus halo, palette overlay   |
| `src/terminal_view.rs` | Grid rendering, cursor, selection, mouse and key input    |
| `src/pty.rs`           | Shell process + `alacritty_terminal` state and event loop |
| `src/input.rs`         | Keystroke → escape-sequence encoding                      |
| `src/links.rs`         | URL and OSC 8 hyperlink detection                         |
| `src/search.rs`        | Find in scrollback: matching, navigation, highlights      |
| `src/vi.rs`            | Keyboard select mode: keys, motions, selections           |
| `src/glyphs.rs`        | Per-character glyph cache used to draw the grid           |
| `src/boxdraw.rs`       | Box-drawing, block and powerline characters as shapes     |
| `src/mouse.rs`         | Mouse event encoding for programs in mouse mode           |
| `src/palette.rs`       | Command palette                                           |
| `src/theme.rs`         | Theme palettes and 256-colour table                       |
| `src/settings.rs`      | Settings global and config file                           |
| `src/session.rs`       | Saved pane layout (JSON) and loading/sanitising it        |
| `src/shell_integration.rs` | Loading the shell scripts; turning their OSC 133/7 marks into hidden links |
| `src/marks.rs`         | Finding prompts, exit statuses and command output in the grid |
| `shell-integration/`   | The bash, zsh and fish scripts                            |
| `examples/`            | Scripts using `lumen msg` and the socket API, and example plugins |
| `python/`              | `lumen_plugin`, the Python package for plugins (built into Lumen), and its tests |
| `plugins/`             | The built-in plugins (built into Lumen; turned on in the Plugin Gallery), and tests |
| `docs/plugins.md`      | Guide and reference for writing plugins in Python         |

## Tests

```sh
cargo test
python3 -m unittest discover -s python/tests   # the Python package
python3 -m unittest discover -s plugins/tests  # the built-in plugins
```

## Next up

- Keyboard link hints
- In-process plugin hooks
- A quick-terminal dropdown
- IME input, cursor blink, background blur/opacity setting
- Ghostty theme file import
