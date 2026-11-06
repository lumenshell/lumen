# Shell integration

When a pane runs bash, zsh or fish, Lumen loads a small script into it. The
script marks where each prompt, command and its output begin, and reports the
shell's folder.

Set `shell-integration = false` in the config to turn it off.

## What you get

- **Jump between commands** with Ctrl+Shift+↑ / ↓ (⌘⇧↑ / ⌘⇧↓). The prompt you
  jumped to stays highlighted until you type. Jumping past the newest one
  returns to the bottom.
- **Copy the last command's output** with Ctrl+Shift+O (⌘⇧O). It's selected
  too, so you can see what was copied.
- **Failure marks.** A bar in the left margin beside each prompt, grey if the
  command succeeded and red if it failed. A failed command's prompt line is
  tinted red too. The scrollbar marks every command the same way, so you can
  find failures far back in the history and click to jump to them.
- **Notifications for long commands.** A command runs for at least
  `notify-after` seconds (10 by default) and finishes while you're looking
  elsewhere: at another pane or tab, or another app. You get a desktop
  notification such as "✗ make test — Failed with exit 2 after 3m 12s ·
  ~/proj". If its pane isn't focused, its tab also gets a dot, green for
  success and red for failure, until you visit it. On Linux this uses
  `notify-send` (`sudo apt install libnotify-bin`). On macOS it uses
  Notification Center. Set `notify-after = 0` to turn it off.
- **New panes open where you are**, using the folder the shell reports. This
  works on macOS too. Without it, Lumen can only read the folder from `/proc`,
  on Linux.

## How it loads

Your own startup files (`~/.bashrc`, `~/.zshrc`, `~/.config/fish`) still load
as usual.

The scripts are copied to `~/.local/share/lumen/shell-integration`
(`$XDG_DATA_HOME`, or `~/Library/Application Support/lumen/shell-integration`
on macOS). Lumen loads them with `--rcfile` for bash, `ZDOTDIR` for zsh and
`XDG_DATA_DIRS` for fish.

## Other shells, login shells and SSH

It doesn't load for other shells. It also doesn't load for bash started as a
login shell (`bash -l`), because `--rcfile` doesn't apply there.

In those cases, or on a remote machine over SSH, source the script yourself:

- bash: add `source /path/to/lumen.bash` to the end of `~/.bashrc`
- zsh: source `lumen.zsh` from `~/.zshrc`
- fish: copy `lumen.fish` into `~/.config/fish/conf.d/`

The scripts are in [`shell-integration/`](../shell-integration). Folders
reported by a remote host are ignored, so new panes still open locally.

## Marks can't be faked

What a command prints can't fake a mark. Otherwise `cat`ing a file or visiting
a hostile host could make up a prompt, a command and its exit status, and
plugins, Command History and notifications would believe it.

- Each pane gets a random key in `LUMEN_MARK_KEY`.
- The scripts add it to every mark.
- Once a pane's shell has used the key, marks without it are dropped.
- The shell's folder is only taken right after its own prompt mark, not from
  a background job printing at the prompt.

So a shell started inside another, such as one on a remote host over SSH,
isn't marked unless it has the key. For a host you trust, send it:

1. Add `SendEnv LUMEN_MARK_KEY` for the host in `~/.ssh/config`.
2. Add `AcceptEnv LUMEN_MARK_KEY` to the server's `sshd_config`.

A shell Lumen didn't set up (another shell, or `bash -l`) can still use plain
marks without a key.
