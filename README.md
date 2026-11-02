# Lumen

A fast, good-looking terminal for Linux and macOS, built with Rust,
[GPUI](https://gpui.rs) (Zed's GPU UI framework) and
[`alacritty_terminal`](https://crates.io/crates/alacritty_terminal).

## Features (MVP)

- GPU-rendered terminal grid: truecolor, bold/italic, underline/undercurl,
  strikethrough, wide (CJK) glyphs, colour emoji, 10k lines of scrollback
- Tabs in a custom title bar, with pane counts and inherited working directory
- Split panes on a padded canvas: rounded frames, an accent focus halo, and
  dimmed unfocused panes; Focus Mode zooms the active pane
- Drag the gap between panes to resize a split; double-click it to even the split out
- Your layout comes back next launch: tabs, splits and their sizes, each pane's
  working directory, which pane had focus, and the window's size, position and
  maximized state
- Command palette (fuzzy search over every action and theme)
- Themes that recolour both the terminal and the chrome: Rosé Pine, Rosé Pine
  Moon, Catppuccin Mocha, Tokyo Night, Rosé Pine Dawn (light)
- Mouse selection (drag, double-click word, triple-click line), primary-selection
  copy and middle-click paste on Linux, bracketed paste
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
- Settings persist to `~/.config/lumen/config`

## Build

```sh
# Debian/Ubuntu build dependencies
sudo apt install libxkbcommon-x11-dev libxcb1-dev libfreetype-dev libfontconfig-dev \
                 libwayland-dev libvulkan1

cargo run --release
```

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
| Next / prev tab    | Ctrl+Tab / Ctrl+PgDn | ⌘⇧] / ⌘⇧[        |
| Go to tab N        | Alt+1…9              | ⌘1…9             |
| Cycle theme        | Ctrl+Shift+Y         | ⌘⇧Y              |
| Font size          | Ctrl+= / Ctrl+- / 0  | ⌘= / ⌘- / ⌘0     |
| Copy / paste       | Ctrl+Shift+C / V     | ⌘C / ⌘V          |
| Scroll a page      | Shift+PgUp / PgDn    | Shift+PgUp / PgDn|
| Open link          | Ctrl+click           | ⌘+click          |
| Find in scrollback | Ctrl+Shift+F         | ⌘F               |

In the find bar, Enter or ↑ goes to the older match and Shift+Enter or ↓ to
the newer one. Alt+C toggles case-sensitive matching (otherwise case is ignored
unless the query has a capital letter), Alt+R toggles regular expressions, and
Esc closes it.

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
```

Delete a line to go back to its default. Mistakes (a typo'd key, a bad value, a
font that isn't installed) are listed in a banner at the bottom of the window
while the rest of the file still applies; a file that can't be parsed at all
keeps the previous settings. Switching theme or zooming the font from inside
Lumen edits just those keys and keeps your comments.

`shell` applies to new panes; everything else applies to open ones too. Older
versions used `~/.config/lumen/config`; its values are carried over the first
time `config.toml` is created.

## Session

The pane layout is saved to `~/.local/state/lumen/session.json`
(`$XDG_STATE_HOME`, or `~/Library/Application Support/lumen` on macOS) when you
quit or close the window, and every few seconds while Lumen runs. Shells start
fresh in their saved directories; scrollback isn't kept. If you `exit` every
shell, the session is cleared and the next launch starts with one tab. Delete
the file to reset.

The window reopens at its saved size and position, maximized or fullscreen if it
was. If the saved spot would leave the title bar off every connected monitor
(say a monitor was unplugged), the window manager places it instead. On Wayland,
apps can't position their own windows, so only the size and maximized state
come back.

## Layout

| File                   | What it does                                              |
|------------------------|-----------------------------------------------------------|
| `src/main.rs`          | Actions, keybindings, fonts, window creation              |
| `src/workspace.rs`     | Title bar, tabs, pane tree, focus halo, palette overlay   |
| `src/terminal_view.rs` | Grid rendering, cursor, selection, mouse and key input    |
| `src/pty.rs`           | Shell process + `alacritty_terminal` state and event loop |
| `src/input.rs`         | Keystroke → escape-sequence encoding                      |
| `src/links.rs`         | URL and OSC 8 hyperlink detection                         |
| `src/search.rs`        | Find in scrollback: matching, navigation, highlights      |
| `src/glyphs.rs`        | Per-character glyph cache used to draw the grid           |
| `src/boxdraw.rs`       | Box-drawing, block and powerline characters as shapes     |
| `src/mouse.rs`         | Mouse event encoding for programs in mouse mode           |
| `src/palette.rs`       | Command palette                                           |
| `src/theme.rs`         | Theme palettes and 256-colour table                       |
| `src/settings.rs`      | Settings global and config file                           |
| `src/session.rs`       | Saved pane layout (JSON) and loading/sanitising it        |

## Next up

- Drawers, workspaces
- IME input, cursor blink, background blur/opacity setting
- Ghostty theme file import
