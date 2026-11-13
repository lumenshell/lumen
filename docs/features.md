# Everything Lumen does

The full list, grouped by area. The [README](../README.md) has the highlights.

## Drawing the terminal

- GPU-rendered grid: truecolor, bold and italic, underline and undercurl,
  strikethrough, wide (CJK) glyphs, colour emoji, configurable scrollback.
- Box-drawing, block and powerline characters are drawn as shapes, so lines
  and prompt segments join without gaps.
- 24 built-in themes, 8 of them light, recolour both the terminal and the
  window around it. Add your own, or drop in any Ghostty theme file, and pick
  a light and a dark one to follow the system ([Themes](config.md#themes)).
- JetBrains Mono and Inter are bundled, so it looks the same everywhere.
- A blinking cursor, or a steady one (`cursor-blink`).
- A see-through, blurred background where the desktop supports it
  (`background-opacity`).

## Tabs, panes and windows

- Tabs live in a custom title bar, showing pane counts. A new tab starts in
  the current folder.
- A tab shows the title its program sets until you rename it: double-click
  the tab, or press Ctrl+, (⌘,).
- **+** opens a tab. A tab's **×** closes it with all its panes. Drag a tab
  along the bar to reorder it.
- Drag the title bar to move the window. Double-click it to maximize.
- Split panes sit on a padded canvas with rounded frames. The focused pane
  gets an accent halo and the others dim (`unfocused-dim`; `pane-gap` sets the
  space between them). Focus Mode zooms the active pane. With `bell = "flash"`
  a bell flashes its pane, and marks its tab yellow if you're elsewhere.
- Drag the gap between panes to resize a split. Double-click the gap to even
  it out.
- Open as many windows as you like (Ctrl+Shift+N, ⌘N). A new window opens in
  the current pane's folder. Closing a window's last pane closes the window,
  and Lumen quits when its last window closes.
- A [quick terminal](quick-terminal.md) drops down from the top of the screen
  on a key you press from any app. The same key hides it, and its shells keep
  running.
- Your layout comes back next launch: every window with its tabs, splits and
  their sizes, each pane's folder, which pane had focus, the tab names you
  gave, and the window's size, position and maximized state (see
  [Files and session](files.md)).

## Typing

- Input methods for Chinese, Japanese and Korean, compose keys and dead keys.
  The text you're composing shows at the cursor.
- The kitty keyboard protocol, so programs such as Neovim and Helix can tell
  Ctrl+I from Tab and Esc from Ctrl+[, and see key releases.
- Every shortcut can be changed in the config (see
  [Key bindings](config.md#key-bindings)).

## Finding and selecting

- The command palette fuzzy-searches every action and theme.
- Find in scrollback, with optional case-sensitive and regex matching.
- [Keyboard select mode](keyboard.md#keyboard-select-mode): move through the
  screen and scrollback with vi keys, then select and copy without the mouse.
- Mouse selection: drag, double-click for a word, triple-click for a line,
  Shift+click to extend. On Linux, selecting copies to the primary selection
  and middle-click pastes. `copy-on-select = true` also copies to the
  clipboard, and `right-click` can paste or copy. `word-separators` sets what
  ends a word. Bracketed paste.
- A scrollbar appears while you scroll, with a mark for every command (red for
  the ones that failed). Click a mark to jump to that command, or drag the
  bar. It fades out when you stop. Point at a pane's right edge to bring it
  back. `scrollbar = false` turns it off.
- The wheel scrolls history, `scroll-speed` lines per notch (3 by default). In full-screen programs that don't use the
  mouse, such as `less` and `man`, it moves through the text with arrow keys.

## Links and the mouse

- Hold Ctrl (⌘ on macOS; change it with `link-modifier`) to underline URLs and OSC 8 hyperlinks under the
  pointer, with the target shown at the bottom of the pane. Click to open.
  Plain-text URLs with common schemes open. OSC 8 links hide their target
  behind other text, so only http, https and mailto ones open.
- Mouse reporting for vim, tmux, htop and friends: clicks, drags, motion and
  the wheel in X10, UTF-8 and SGR formats, plus focus in and out events. Hold
  Shift to select text locally instead.
- Programs can set the clipboard (OSC 52, used by tmux and vim over SSH) but
  can't read it.

## Shell integration

For bash, zsh and fish. See [Shell integration](shell-integration.md).

- Jump between commands and copy the last command's output.
- A red mark beside commands that failed.
- New panes open in the shell's folder.
- A desktop notification when a long command finishes while you're looking
  elsewhere, and a green or red dot on its tab until you go back to it.

## Commands and scripting

- `lumen info`: a neofetch-style summary of your system and this Lumen, beside
  a flickering flame in your theme's colours (see [Commands](commands.md)).
- `lumen campfire`: a crackling campfire that fills the pane, for cold coding
  sessions (see [Commands](commands.md#campfire)).
- `lumen msg` opens tabs and splits, types into panes, reads their screens and
  streams events such as "command finished with exit 1" (see
  [Scripting](scripting.md)).

## Plugins

- [Built-in plugins](builtin-plugins.md) you turn on from the Plugin Gallery:
  jump to recent folders, search every command you've run (and rerun the last
  failure), switch git branches, SSH to a host, open project layouts, see
  build status in the tab title, and a pomodoro timer.
- Plugins in any language. Lumen runs them and restarts them if they crash.
  They can add palette commands, key bindings and pickers, draw panes of their
  own, and put badges and colours on tabs, limited to the permissions you
  approve.
- Python plugins get `lumen_plugin` built in, with nothing to install (see the
  [plugin guide](plugins.md)).

## Config

- A commented `config.toml` that applies changes as soon as you save it (see
  [Config](config.md)).
