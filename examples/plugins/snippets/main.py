"""Insert Snippet (Ctrl+Shift+X, ⌘⇧X on macOS): pick one of your snippets
from snippets.toml (in this folder) and type it into the pane, without
pressing Enter. Python 3.11+ (for tomllib).
"""

import os
import sys

try:
    import tomllib
except ImportError:  # Python < 3.11
    tomllib = None

from lumen_plugin import Plugin

app = Plugin()
KEYS = "cmd-shift-x" if sys.platform == "darwin" else "ctrl-shift-x"
FILE = os.path.join(app.dir, "snippets.toml")


@app.command("Insert Snippet", keys=KEYS)
async def insert(pane):
    if tomllib is None:
        await app.notify("Insert Snippet", "This needs Python 3.11 or newer.")
        return
    try:
        with open(FILE, "rb") as f:
            snippets = tomllib.load(f).get("snippet", [])
    except (OSError, tomllib.TOMLDecodeError) as e:
        await app.notify("Insert Snippet", f"Couldn't read {FILE}: {e}")
        return
    items = [(i, s["name"], s["text"].replace("\n", " ⏎ "))
             for i, s in enumerate(snippets) if "name" in s and "text" in s]
    chosen = await app.pick(items, placeholder="Insert snippet…")
    if chosen is not None:
        await pane.send_text(snippets[chosen]["text"].rstrip("\n"))


app.run()
