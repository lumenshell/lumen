# Keyboard

- [Shortcuts](#shortcuts)
- [Find bar](#find-bar)
- [Command palette](#command-palette)
- [Renaming tabs](#renaming-tabs)
- [Keyboard select mode](#keyboard-select-mode)

To change a shortcut, see [Key bindings](config.md#key-bindings).

## Shortcuts

| Action             | Linux                | macOS            |
|--------------------|----------------------|------------------|
| Command palette    | Ctrl+Shift+P         | ⌘⇧P              |
| New tab            | Ctrl+T / Ctrl+Shift+T | ⌘T              |
| New window         | Ctrl+Shift+N         | ⌘N               |
| Split right / down | Ctrl+Shift+D / E     | ⌘D / ⌘⇧D         |
| Close pane         | Ctrl+Shift+W         | ⌘W               |
| Focus pane         | Ctrl+Arrows or Alt+Shift+Arrows | ⌘Arrows or ⌘⌥Arrows |
| Next pane          | Ctrl+Shift+]         | ⌘]               |
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
| Prev / next command | Ctrl+Shift+↑ / ↓    | ⌘⇧↑ / ⌘⇧↓        |
| Copy last output   | Ctrl+Shift+O         | ⌘⇧O              |
| Keyboard select mode | Ctrl+Shift+Space   | ⌘⇧Space          |
| Quit               | Ctrl+Shift+Q         | ⌘Q               |

The command jumps and Copy last output need
[shell integration](shell-integration.md).

Focus left, right, up and down and Go to tab N are keyboard-only. They aren't
in the palette.

You can change every shortcut except Shift+PgUp/PgDn, Ctrl+click and the keys
inside the find bar, the palette and keyboard select mode.

## Find bar

| Key | Does |
|---|---|
| Enter or ↑ | Older match |
| Shift+Enter or ↓ | Newer match |
| Alt+C | Toggle case-sensitive matching |
| Alt+R | Toggle regular expressions |
| Esc | Close |

Case is ignored unless the query has a capital letter, or you turn on
case-sensitive matching.

## Command palette

The palette lists Lumen's own commands under **Lumen** and the commands
plugins add under **Plugins**, each with its shortcut (a plugin command
without one shows which plugin it's from). While you type, matches stay under
their headings, best first, and the section with the best match comes first. Besides every action above, the Lumen section has:

- **Open Config File**
- **Theme…**, a list of every theme: each one is shown as you move to it,
  Enter keeps it and Esc goes back to the one you had
- **Tab Color…**, to tint the active tab in one of your theme's six colours
  (see [Colouring tabs](#colouring-tabs))
- **Plugin Logs…**, and **Allow Plugin:** for each plugin waiting for approval
- **Plugins** (with the number of updates waiting) and **Reload Plugins**

| Key | Does |
|---|---|
| Type | Filter the list |
| Ctrl+U | Clear the filter |
| ↑ / ↓ or Tab | Choose (the list scrolls) |
| Enter | Run |
| Esc, or a click outside | Close |

## Renaming tabs

Rename Tab (Ctrl+, / ⌘, or double-click a tab) asks for a name in the
palette, filled in with the name you gave it, if any.

- Enter renames it.
- Ctrl+U clears the line.
- Esc keeps the old name.
- An empty name goes back to the title the program sets.

Names are saved with the session. `lumen msg set-title` sets the same name
from a script.

## Colouring tabs

**Tab Color…** in the palette tints the active tab, to tell projects apart at
a glance. It lists your theme's red, green, yellow, blue, magenta and cyan,
each with a swatch, and No Colour.

- Moving through the list shows each colour on the tab.
- Enter keeps it. Esc puts back the colour the tab had.
- No Colour clears it, including one a script set.

A colour you pick is saved with the session and wins over one a script or
plugin sets with `lumen msg tab-style --color`.

## Keyboard select mode

Ctrl+Shift+Space (⌘⇧Space) puts a pane into select mode. A second cursor, in
your accent colour, moves with vi keys over the screen and scrollback. A hint
at the bottom of the pane shows what the next keys do. Nothing you type
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
| `[` `]` | Previous or next command's prompt (needs [shell integration](shell-integration.md)) |
| `/` `?`, then `n` `N` | Find (down or up) with the find bar, then the next or previous match |

A number before a move repeats it: `5j`, `3w`, `2[`.

| Keys | Select and copy |
|---|---|
| `v`, `V`, `Ctrl+V` | Start a selection of characters, whole lines or a block. The same key again ends it |
| `y` or Enter | Copy the selection and leave |
| `Y` | Copy the line under the cursor and leave |
| Esc | Drop the selection. With none, leave (`q`, `i` and Ctrl+C also leave) |

Clicking moves the cursor there, and `y` copies a mouse selection too. On
Linux, copying also fills the primary selection, so middle-click pastes it.
Leaving scrolls back to the bottom.

## Option as Alt (macOS)

On macOS, Option types special characters (Option+B is ∫) unless you set
`option-as-alt = true` in [Config](config.md). Then Option works as Alt does
on Linux: Option+B sends Alt+B, which shells use to move back a word.
