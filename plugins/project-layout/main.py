"""Open Project Layout (Ctrl+Shift+L, ⌘⇧L on macOS): find .lumen-layout.toml
in the focused pane's directory or the nearest one above it, and open the tab
and splits it describes. Needs shell integration (for the directory) and
Python 3.11+ (for tomllib).

    # .lumen-layout.toml
    title = "api"                 # the tab's name (optional)

    [[pane]]                      # the first pane is the new tab
    run = "nvim ."

    [[pane]]
    split = "right"               # split the pane before it ("right" or "down")
    run = "cargo watch -x test"

    [[pane]]
    split = "down"
    of = 1                        # or split pane 1 (the first is 1)
    cwd = "frontend"              # relative to the layout file
    run = "npm run dev"

A layout runs commands, and a cloned repository can ship one, so you're
shown every command and asked before anything runs.
"""

import os
import shlex
import sys

try:
    import tomllib
except ImportError:  # Python < 3.11
    tomllib = None

from lumen_plugin import Plugin

app = Plugin()
KEYS = "cmd-shift-l" if sys.platform == "darwin" else "ctrl-shift-l"
NAME = ".lumen-layout.toml"


def find_layout(start):
    folder = os.path.abspath(start)
    while True:
        path = os.path.join(folder, NAME)
        if os.path.isfile(path):
            return path
        parent = os.path.dirname(folder)
        if parent == folder:
            return None
        folder = parent


def load(path):
    """The layout's title and panes, checked; raises ValueError if it's wrong."""
    with open(path, "rb") as f:
        data = tomllib.load(f)
    panes = data.get("pane")
    if not isinstance(panes, list) or not panes:
        raise ValueError("it needs at least one [[pane]]")
    base = os.path.dirname(path)
    out = []
    for i, p in enumerate(panes, 1):
        if i > 1 and p.get("split", "right") not in ("right", "down"):
            raise ValueError(f"pane {i}: split should be \"right\" or \"down\"")
        of = p.get("of", i - 1)
        if i > 1 and not (isinstance(of, int) and 1 <= of < i):
            raise ValueError(f"pane {i}: `of` should be the number of an earlier pane")
        cwd = os.path.normpath(os.path.join(base, os.path.expanduser(p.get("cwd", "."))))
        run = p.get("run")
        if run is not None and not isinstance(run, (str, list)):
            raise ValueError(f"pane {i}: run should be a string or a list")
        out.append({"split": p.get("split", "right"), "of": of, "cwd": cwd, "run": run})
    return data.get("title"), out


@app.command("Open Project Layout", keys=KEYS)
async def open_layout(pane):
    if tomllib is None:
        await app.notify("Open Project Layout", "This needs Python 3.11 or newer.")
        return
    cwd = await pane.cwd()
    path = cwd and find_layout(cwd)
    if not path:
        await app.notify("Open Project Layout", f"No {NAME} here or in any folder above.")
        return
    try:
        title, panes = load(path)
    except (ValueError, OSError, tomllib.TOMLDecodeError) as e:
        await app.notify("Open Project Layout", f"{path}: {e}")
        return

    commands = [p["run"] if isinstance(p["run"], str) else shlex.join(p["run"]) for p in panes if p["run"]]
    if commands:
        where = path.replace(os.path.expanduser("~"), "~", 1)
        listing = f"{where} runs: " + "  ·  ".join(f"`{c}`" for c in commands)
        question = f"Open this layout and run {len(commands)} command{'s' if len(commands) > 1 else ''}?"
        if not await app.confirm(question, message=listing, yes="Open and run", no="Cancel"):
            return

    opened = [await app.new_tab(cwd=panes[0]["cwd"], run=panes[0]["run"])]
    if title:
        await opened[0].set_title(str(title))
    for p in panes[1:]:
        opened.append(await opened[p["of"] - 1].split(p["split"], cwd=p["cwd"], run=p["run"], focus=False))
    await opened[0].focus()


app.run()
