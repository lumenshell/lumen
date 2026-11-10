# Built-in plugins

Lumen comes with seven plugins. They're off until you turn them on.

1. Choose **Plugin Gallery** in the palette.
2. Pick one to see what it does and what it's allowed to do.
3. Choose **Turn on**.

Choosing it there is your approval. It starts at once, and again whenever
Lumen opens.

Turned-on plugins are listed under `plugins` in `config.toml`, so they travel
with your dotfiles. A name you add there by hand still asks for approval the
first time.

| Plugin | What it does | Permissions |
|---|---|---|
| **Recent Directories** (`recent-dirs`) | Remembers the folders your shells visit. **Go to Recent Directory** (Ctrl+Shift+J, ⌘⇧J) lists them, most used and most recent first, and `cd`s the pane to the one you pick. If the pane is busy, it opens a new tab there instead. | `input`, `panes` |
| **Command History** (`command-history`) | Keeps every command from every pane and session, with its exit status, duration and folder. **Search Command History** (Ctrl+Shift+H, ⌘⇧H) types the one you pick without pressing Enter. **Rerun Last Failed Command** (Ctrl+Shift+R, ⌘⇧R) shows the last failure and its folder, and runs it again there once you confirm. **Clear Command History** forgets it all. Commands starting with a space aren't kept. | `screen`, `input` |
| **Switch Git Branch** (`git-branch`) | Alt+B lists the branches of the pane's repository, newest first, and switches to the one you pick. | `input` |
| **SSH to Host** (`ssh-hosts`) | Ctrl+Shift+S (⌘⇧S) lists the hosts in `~/.ssh/config` (and files it `Include`s) and opens the one you pick in a new tab named after it. | `panes` |
| **Projects & Tasks** (`project-layout`) | **Open Project** (Ctrl+Shift+L, ⌘⇧L) finds `.lumen-layout.toml` in the pane's folder or above, shows you every command before it first runs, and again whenever the file changes, and opens its tabs and splits. Then it starts the project's dev environment in a new tab, each task in its own labelled split. The tab shows how many are running, how many failed, or that they're all done. With no dev environment yet, it lists what the project can run (compose services, package.json and composer scripts, make and just targets, Procfile entries, Cargo), and saves what you tick to the file. **Edit Dev Environment** changes that choice. **Run Task** (Ctrl+Shift+K, ⌘⇧K) runs one task in a split beside the pane. **Restart Task** (Ctrl+Alt+R, ⌘⌥R) stops the task in the focused split and runs it again. **Run Command…** runs a command you type in a split, **Save as Task** keeps it, and **Add Task** saves one directly. A task saved in the file or found in a Procfile is shown to you in full, with its folder, and confirmed before Run Task first runs it ([format](../plugins/project-layout/main.py)). | `panes`, `input` |
| **Build Status** (`build-status`) | When a build or test (`cargo test`, `make`, `npm test`, `pytest`, …) finishes, the tab title shows how it went, like `✓ cargo test · 12s`, until the next command. Leaves tabs you've named alone. | `screen`, `panes` |
| **Pomodoro Timer** (`pomodoro`) | **Start Pomodoro** asks how long to focus (25, 50 or 15 minutes), then opens a strip under the pane with a big countdown. The tab shows it too, tinted red while you focus and green on a break. Breaks start by themselves, and every fourth is a long one. The next session waits for you. In the timer: Space pauses, S skips, R restarts, +/- change a minute, Q closes. Counts today's sessions. | `panes` |

Most of them need [shell integration](shell-integration.md) to know folders
and commands. The Pomodoro Timer doesn't.

Their data (history, folders) is kept in `~/.local/state/lumen/plugin-data/`,
readable only by you.

## Changing one

A folder of your own in the plugins folder with the same name runs instead of
the built-in one. So you can copy one from [`plugins/`](../plugins/) and change
it. See the [plugin guide](plugins.md) for how they're written.
