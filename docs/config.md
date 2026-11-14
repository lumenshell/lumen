# Config

Settings live in `~/.config/lumen/config.toml`. Lumen creates it, with
comments, the first time it runs. Open it from the command palette with
**Open Config File**.

Changes apply as soon as you save. No restart needed.

```toml
theme = "Rosé Pine"            # any theme below, or "light:Rosé Pine Dawn, dark:Rosé Pine"
accent-color = "#c4a7e7"       # replace the theme's accent colour (unset: the theme's own)
font-family = "JetBrains Mono" # any installed monospace font; JetBrains Mono is built in
font-size = 14
line-height = 1.35             # multiple of the font size
padding = 12                   # pixels between a pane's edge and its text
scrollback = 10000             # lines of history per pane
scrollbar = true               # a scrollbar with command marks while you scroll; false: none
cursor-style = "block"         # "block", "beam" or "underline", until a program picks one
cursor-blink = true            # false: the cursor never blinks, even if a program asks
cursor-blink-interval = 530    # milliseconds on, then off, when it blinks (200 to 2000)
bold-is-bright = true          # bold text in the first eight colours uses their bright versions
ligatures = true               # join operators like -> and != when the font has ligatures
background-opacity = 1         # 0.3 to 1; below 1 the desktop shows through (needs a compositor on Linux)
shell = "/usr/bin/fish --login" # program for new panes; defaults to $SHELL
shell-integration = true       # mark prompts and commands in bash, zsh and fish
quick-terminal-key = "f12"     # (X11) a key Lumen catches from any app for the quick terminal; "" for none
quick-terminal-height = 0.45   # its height, as a share of the screen
quick-terminal-autohide = false # hide it when you click into another window
notify-after = 10              # seconds a command must run to notify when it ends; 0 = off
socket-api = true              # let scripts control Lumen with `lumen msg`
copy-on-select = false         # selecting also copies to the clipboard
word-separators = ",│`|:\"'()[]{}<>"  # characters that end a word on double-click; spaces always do
scroll-speed = 3               # lines the mouse wheel scrolls per notch (1 to 20)
right-click = "none"           # "none", "paste", or "copy-or-paste" (copy a selection, else paste)
link-modifier = "ctrl"         # key to hold when clicking links: "ctrl", "cmd", "alt" or "none"; "cmd" on macOS
option-as-alt = false          # (macOS) Option sends Alt to programs instead of typing special characters
kitty-keyboard = true          # let programs turn on the kitty keyboard protocol
bell = "off"                   # "off", or "flash": flash the pane, and mark its tab if you're elsewhere
window-width = 1100            # size of a new window when there's no saved one, in pixels
window-height = 720
restore-session = true         # bring back windows, tabs and splits next launch; false saves nothing
working-directory = "inherit"  # where new tabs, splits and windows start: "inherit", "home" or a folder
# unfocused-dim = 0.35         # dim panes you're not in, 0 to 0.5; unset follows the theme (0.25 on light ones)
pane-gap = 8                   # pixels between split panes (0 to 24)
ui-font-family = "Inter"       # font for tabs, the palette and banners; Inter is built in
plugins = ["recent-dirs"]      # built-in plugins to run; the Plugin Gallery edits this
```

Delete a line to go back to its default.

## Mistakes

A typo'd key, a bad value or a font that isn't installed is listed in a banner
at the bottom of the window. The rest of the file still applies. If the file
can't be parsed at all, Lumen keeps the previous settings.

Switching theme or zooming the font from inside Lumen edits just those keys
and keeps your comments.

## Themes

Each theme colours the terminal and the window around it: title bar, tabs,
palette and the gaps between panes.

**Built in (24).** Dark: Rosé Pine, Rosé Pine Moon, Catppuccin Mocha,
Catppuccin Macchiato, Catppuccin Frappé, Tokyo Night, Tokyo Night Storm, Nord,
Gruvbox Dark, Kanagawa, Everforest Dark, GitHub Dark, Flexoki Dark, Solarized
Dark, Dracula, One Dark. Light: Rosé Pine Dawn, Catppuccin Latte, Tokyo Night
Day, Gruvbox Light, Everforest Light, GitHub Light, Flexoki Light, Solarized
Light. Pick one with `theme = "Name"`, from the palette (**Theme:** entries), or
cycle with **Cycle Theme** (Ctrl+Shift+Y, ⌘⇧Y).

**Follow the system's light or dark** with a pair:

```toml
theme = "light:Rosé Pine Dawn, dark:Rosé Pine"
```

Lumen switches when the system does. On Linux that's the desktop's dark-style
setting (the freedesktop `color-scheme`, which GNOME, KDE and Cinnamon set);
on a desktop that doesn't set it, the light one is used. Picking a theme from
the palette replaces the pair with that one theme.

**Your own themes** go in `~/.config/lumen/themes/`, one file per theme, named
after it: `Midnight.toml` is "Midnight". Lumen picks up new and changed files
as you save them, and a file named like a built-in theme replaces it. Two
formats work:

- Lumen's own, in TOML. Only `background` and `foreground` are required. Left
  out, the palette is Rosé Pine's, and the window colours are worked out from
  the rest.

  ```toml
  background = "#191724"
  foreground = "#e0def4"
  cursor = "#ebbcba"
  selection = "#403d52"
  palette = ["#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
             "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4"]

  [chrome]          # the window around the panes; all optional
  shell = "#12101b" # behind the panes and the title bar
  raised = "#26233a" # active tab, palette
  border = "#2a2738"
  subtext = "#908caa"
  muted = "#6e6a86"
  accent = "#c4a7e7"
  ```

- [Ghostty](https://ghostty.org) theme files, unchanged (no extension, or
  `.conf`): `background`, `foreground`, `cursor-color`,
  `selection-background` and `palette = N=#rrggbb` lines. Ghostty ships
  hundreds of them. A file with no extension is named in full, dots and all:
  `Catppuccin.Mocha` is "Catppuccin.Mocha".

A file with a mistake is named in the banner, and the other themes still load.
Notes like a `README` are left alone, and if two files give the same theme
name, the first (by file name) is used and the other is named in the banner.

`accent-color = "#rrggbb"` replaces the accent (the focused pane's border,
highlights in the palette) of whichever theme is in use.

The built-in themes come from their projects' published palettes, all under
the MIT licence: [Rosé Pine](https://rosepinetheme.com),
[Catppuccin](https://catppuccin.com),
[Tokyo Night](https://github.com/folke/tokyonight.nvim),
[Nord](https://www.nordtheme.com), [Gruvbox](https://github.com/morhetz/gruvbox),
[Kanagawa](https://github.com/rebelot/kanagawa.nvim),
[Everforest](https://github.com/sainnhe/everforest),
[GitHub](https://github.com/primer/github-vscode-theme),
[Flexoki](https://stephango.com/flexoki),
[Solarized](https://ethanschoonover.com/solarized),
[Dracula](https://draculatheme.com) and
[One Dark](https://github.com/atom/atom/tree/master/packages/one-dark-syntax).

## When settings apply

- `shell` and `shell-integration` apply to new panes.
- Turning `socket-api` on takes a restart. Turning it off applies at once.
- `restore-session`, `window-width` and `window-height` apply at the next
  launch. With `restore-session = false` the session file is left as it is,
  so turning it back on brings back the last saved session.
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
