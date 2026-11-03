# Example plugins

Copy (or symlink) a folder into `~/.config/lumen/plugins/` and choose
**Reload Plugins** in the palette. See [Plugins](../../README.md#plugins) for how
plugins work.

| Plugin | Language | Permissions | What it does |
|---|---|---|---|
| [`hello`](hello/) | bash (+ `jq`) | none | Adds **Say Hello** (Alt+H), which shows a notification naming the pane you're in. The smallest complete plugin: register a command, react to it. |
| [`git-branch`](git-branch/) | Python 3 | `input` | Adds **Switch Git Branch** (Alt+B): lists the branches of the focused pane's repository, newest first, in a palette picker, and types `git switch <branch>` into the pane. Shows `register-command`, `list`, `show-picker` and `send-text`, with a small reusable client class. |

```sh
ln -s "$PWD/examples/plugins/git-branch" ~/.config/lumen/plugins/
```

`git-branch` asks for the `input` permission, so Lumen asks you to allow it
the first time. Their logs are under **Plugin Log: hello** and **Plugin Log:
git-branch** in the palette.
