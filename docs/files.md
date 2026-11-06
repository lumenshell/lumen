# Files, session and environment

## Session

Lumen saves every window's pane layout to `~/.local/state/lumen/session.json`
(`$XDG_STATE_HOME`, or `~/Library/Application Support/lumen` on macOS). It
saves when you quit, and every few seconds while it runs.

- Tab names you gave are kept.
- Shells start fresh in their saved folders. Scrollback isn't kept.
- Closing a window drops it from the session, except the last one, which comes
  back next launch.
- If you `exit` every shell, the session is cleared and the next launch starts
  with one tab.

Delete the file to reset.

Each window reopens at its saved size and position, maximized or fullscreen if
it was. If the saved spot would leave the title bar off every connected
monitor (say a monitor was unplugged), the window manager places it instead.
On Wayland, apps can't position their own windows, so only the size and
maximized state come back.

## Where Lumen keeps things

| What           | Where (Linux)                                   | macOS                                               |
|----------------|-------------------------------------------------|-----------------------------------------------------|
| Settings       | `~/.config/lumen/config.toml` (`$XDG_CONFIG_HOME`) | same                                             |
| Session        | `~/.local/state/lumen/session.json` (`$XDG_STATE_HOME`) | `~/Library/Application Support/lumen/session.json` |
| Shell scripts  | `~/.local/share/lumen/shell-integration/` (`$XDG_DATA_HOME`) | `~/Library/Application Support/lumen/shell-integration/` |
| Menu entry and icons (`scripts/install-linux.sh`) | `~/.local/share/applications/lumen.desktop`, `~/.local/share/icons/hicolor/*/apps/lumen.*` | in `Lumen.app` |
| Socket         | `$XDG_RUNTIME_DIR/lumen/lumen-<pid>.sock`       | `$TMPDIR/lumen-<uid>/lumen-<pid>.sock`              |
| Plugins        | `~/.config/lumen/plugins/<name>/` (`$XDG_CONFIG_HOME`) | same                                         |
| Plugin logs and approvals | `~/.local/state/lumen/plugins/<name>.log`, `plugin-approvals.json` | `~/Library/Application Support/lumen/…` |
| Python package (`lumen python-path`) | `~/.local/state/lumen/python/lumen_plugin/` | `~/Library/Application Support/lumen/python/…` |
| Built-in plugins (written out by Lumen) and plugin data | `~/.local/state/lumen/builtin-plugins/`, `plugin-data/<name>/` | `~/Library/Application Support/lumen/…` |

## Environment

Shells started by Lumen get:

| Variable | Value |
|---|---|
| `TERM` | `xterm-256color` |
| `COLORTERM` | `truecolor` |
| `TERM_PROGRAM` | `lumen`, so scripts can tell they're running in Lumen |
| `LUMEN_SOCKET`, `LUMEN_PANE` | For [`lumen msg`](scripting.md), when the socket API is on |
| `LUMEN_MARK_KEY` | For the [shell integration](shell-integration.md#marks-cant-be-faked) marks |
