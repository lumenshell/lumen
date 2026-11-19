# Built-in plugins

Lumen comes with four plugins. They're off until you turn them on.

1. Choose **Plugins** in the palette. It opens the plugin manager in a new
   tab.
2. Move to one to see what it does and what it's allowed to do.
3. Press Space to turn it on. If it needs permissions, the manager lists them
   and asks first.

Allowing it there is your approval. It starts at once, and again whenever
Lumen opens. Space turns it off again, and `s` changes its settings.

Turned-on plugins are listed under `plugins` in `config.toml`, so they travel
with your dotfiles. A name you add there by hand still asks for approval the
first time.

| Plugin | What it does | Permissions |
|---|---|---|
| **Recent Directories** (`recent-dirs`) | Remembers the folders your shells visit. **Go to Recent Directory** (Ctrl+Shift+J, ⌘⇧J) lists them, most used and most recent first, and `cd`s the pane to the one you pick. If the pane is busy, it opens a new tab there instead. | `input`, `panes` |
| **Command History** (`command-history`) | Keeps every command from every pane and session, with its exit status, duration and folder. **Search Command History** (Ctrl+Shift+H, ⌘⇧H) types the one you pick without pressing Enter. **Rerun Last Failed Command** (Ctrl+Shift+R, ⌘⇧R) shows the last failure and its folder, and runs it again there once you confirm. **Clear Command History** forgets it all. Commands starting with a space aren't kept. | `screen`, `input` |
| **Switch Git Branch** (`git-branch`) | Alt+B lists the branches of the pane's repository, newest first, and switches to the one you pick. | `input` |
| **SSH to Host** (`ssh-hosts`) | Ctrl+Shift+S (⌘⇧S) lists the hosts in `~/.ssh/config` (and files it `Include`s) and opens the one you pick in a new tab named after it. | `panes`, `input` |

Most of them need [shell integration](shell-integration.md) to know folders
and commands.

## Moved to the registry

Three plugins that used to come with Lumen now install from the
[plugin registry](plugins.md#managing-plugins), so they can be updated on
their own:

| Plugin | Install as | What it does |
|---|---|---|
| **Projects & Tasks** | `sn/project-layout` | Opens a project's tabs, splits and dev environment from `.lumen-layout.toml`, and runs its tasks |
| **Build Status** | `sn/build-status` | Shows how the last build or test went in the tab title, like `✓ cargo test · 12s` |
| **Pomodoro Timer** | `sn/pomodoro` | A big countdown in a strip under the pane, with the time on the tab |

If `config.toml` still lists one by its old name (`project-layout`,
`build-status`, `pomodoro`), Lumen reads it as the registry plugin, along with
its `[plugin.NAME]` settings and its saved data. Until it's installed, a
banner says so: open **Plugins** and press `i` on it, or run
`lumen plugins install sn/pomodoro`.

## Settings

Two of them have settings, set in `config.toml` (see
[plugin settings](config.md#plugin-settings)):

| Plugin | Setting | Default | What it does |
|---|---|---|---|
| Command History | `max-entries` | 10000 | How many commands to keep (100 to 1000000) |
| Recent Directories | `max-shown` | 200 | How many folders **Go to Recent Directory** lists (10 to 1000) |

```toml
[plugin.command-history]
max-entries = 50000

```

Changes apply when you save, without restarting the plugin.

Their data (history, folders) is kept in `~/.local/state/lumen/plugin-data/`,
readable only by you.

## Changing one

A folder of your own in the plugins folder with the same name runs instead of
the built-in one. So you can copy one from [`plugins/`](../plugins/) and change
it. See the [plugin guide](plugins.md) for how they're written.
