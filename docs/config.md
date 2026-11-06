# Config

Settings live in `~/.config/lumen/config.toml`. Lumen creates it, with
comments, the first time it runs. Open it from the command palette with
**Open Config File**.

Changes apply as soon as you save. No restart needed.

```toml
theme = "Rosé Pine"            # or "Rosé Pine Moon", "Catppuccin Mocha", "Tokyo Night", "Rosé Pine Dawn"
font-family = "JetBrains Mono" # any installed monospace font; JetBrains Mono is built in
font-size = 14
line-height = 1.35             # multiple of the font size
padding = 12                   # pixels between a pane's edge and its text
scrollback = 10000             # lines of history per pane
scrollbar = true               # a scrollbar with command marks while you scroll; false: none
cursor-style = "block"         # "block", "beam" or "underline", until a program picks one
cursor-blink = true            # false: the cursor never blinks, even if a program asks
background-opacity = 1         # 0.3 to 1; below 1 the desktop shows through (needs a compositor on Linux)
shell = "/usr/bin/fish --login" # program for new panes; defaults to $SHELL
shell-integration = true       # mark prompts and commands in bash, zsh and fish
quick-terminal-key = "f12"     # (X11) a key Lumen catches from any app for the quick terminal; "" for none
quick-terminal-height = 0.45   # its height, as a share of the screen
quick-terminal-autohide = false # hide it when you click into another window
notify-after = 10              # seconds a command must run to notify when it ends; 0 = off
socket-api = true              # let scripts control Lumen with `lumen msg`
plugins = ["recent-dirs"]      # built-in plugins to run; the Plugin Gallery edits this
```

Delete a line to go back to its default.

## Mistakes

A typo'd key, a bad value or a font that isn't installed is listed in a banner
at the bottom of the window. The rest of the file still applies. If the file
can't be parsed at all, Lumen keeps the previous settings.

Switching theme or zooming the font from inside Lumen edits just those keys
and keeps your comments.

## When settings apply

- `shell` and `shell-integration` apply to new panes.
- Turning `socket-api` on takes a restart. Turning it off applies at once.
- Everything else applies to open panes too.

`shell` is split on spaces, so an argument can't contain a space. Wrap it in a
script instead.

Older versions used `~/.config/lumen/config`. Its values are carried over the
first time `config.toml` is created.

## Key bindings

Change shortcuts in a `[keys]` table at the end of `config.toml`. Each line
binds a key to an action by name, and applies as soon as you save:

```toml
[keys]
ctrl-alt-t = "new-tab"      # an extra key for New Tab; Ctrl+Shift+T still works
ctrl-tab = "none"           # free Ctrl+Tab for the desktop or the shell
ctrl-t = "none"             # give Ctrl+T back to the shell (bash swaps letters, fzf finds files)
ctrl-left = "none"          # give Ctrl+←/→ back to the shell for jumping by word
ctrl-right = "none"         # (Alt+Shift+arrows still move between panes)
"ctrl-=" = "increase-font"  # keys with symbols need quotes
```

How it works:

- A key you list replaces whatever it did by default.
- `"none"` frees a key, so it goes to the program in the terminal instead.
- An action's other default keys keep working. To move an action to a new key,
  bind the new key and set the old one to `"none"`.
- Typos are listed in the banner, with a suggestion when an action name is
  close.

Keys are case-insensitive and written like `ctrl-shift-t`, `alt-1`, `cmd-k`
(macOS), `f5`, `shift-insert` or `ctrl-pageup`. On Linux, Shift plus a symbol
matches the symbol it types on a US layout, so `ctrl-shift-]` and `ctrl-}`
are the same key. Multi-key sequences such as `ctrl-a n` aren't supported yet.

Plugin commands can be bound too, as `plugin:<plugin>:<command id>`, for
example `alt-g = "plugin:git-branch:switch"`. That also overrides a key the
plugin asked for.

### Actions

`new-tab`, `new-window`, `quick-terminal`, `close-pane`, `split-right`,
`split-down`, `next-tab`, `prev-tab`, `tab-1` … `tab-9`, `rename-tab`,
`next-pane`, `focus-left`, `focus-right`, `focus-up`, `focus-down`,
`toggle-zoom`, `command-palette`, `next-theme`, `increase-font`,
`decrease-font`, `reset-font`, `copy`, `paste`, `find`, `prev-command`,
`next-command`, `copy-last-output`, `select-mode`, `quit`.

The default keys for each are in [Keyboard](keyboard.md#shortcuts).
