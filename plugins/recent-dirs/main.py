"""Recent Directories: remembers the folders your shells visit and lets you
jump back to one from the palette (Ctrl+Shift+J, ⌘⇧J on macOS), ranked by how
often and how recently you were there. Needs shell integration.

If the pane is busy running something, the folder opens in a new tab instead
of typing `cd` into the running program.
"""

import json
import os
import sys
import time

from lumen_plugin import Plugin

app = Plugin()
KEYS = "cmd-shift-j" if sys.platform == "darwin" else "ctrl-shift-j"
FILE = os.path.join(app.data_dir, "dirs.json")
# When the ranks add up to more than this, they're all scaled down and the
# least used folders forgotten, so the list follows what you do now.
MAX_TOTAL = 5000

dirs = {}       # path -> {"rank": float, "last": unix time}
busy = set()    # panes running a command


def load():
    global dirs
    try:
        with open(FILE) as f:
            dirs = {p: d for p, d in json.load(f).items() if isinstance(d, dict)}
    except (OSError, ValueError):
        dirs = {}


def save():
    tmp = FILE + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump(dirs, f)
    os.replace(tmp, FILE)


def visit(path, now=None):
    if not path or not os.path.isabs(path):
        return
    now = now or time.time()
    entry = dirs.setdefault(path, {"rank": 0.0, "last": now})
    entry["rank"] += 1
    entry["last"] = now
    if sum(d["rank"] for d in dirs.values()) > MAX_TOTAL:
        for p in list(dirs):
            dirs[p]["rank"] *= 0.9
            if dirs[p]["rank"] < 1:
                del dirs[p]


def score(entry, now):
    """Visits, weighted by how recent the last one was."""
    age = now - entry["last"]
    if age < 3600:
        weight = 4
    elif age < 86400:
        weight = 2
    elif age < 7 * 86400:
        weight = 0.5
    else:
        weight = 0.25
    return entry["rank"] * weight


def ago(seconds):
    for unit, size in (("d", 86400), ("h", 3600), ("m", 60)):
        if seconds >= size:
            return f"{int(seconds // size)}{unit} ago"
    return "just now"


def tilde(path):
    home = os.path.expanduser("~")
    return "~" + path[len(home):] if path == home or path.startswith(home + os.sep) else path


def ranked(exclude=None, now=None):
    now = now or time.time()
    gone = [p for p in dirs if not os.path.isdir(p)]
    for p in gone:
        del dirs[p]
    return sorted((p for p in dirs if p != exclude), key=lambda p: score(dirs[p], now), reverse=True)


load()


@app.on_start
async def seed():
    for pane in await app.panes():
        if pane.get("cwd"):
            visit(pane["cwd"])
    save()


@app.on("cwd-changed")
def moved(event):
    visit(event["cwd"])
    save()


@app.on("command-started")
def started(event):
    busy.add(event.pane.id)


@app.on("command-finished", "pane-closed")
def stopped(event):
    busy.discard(event.pane.id)


@app.command("Go to Recent Directory", keys=KEYS)
async def jump(pane):
    here = await pane.cwd() if pane else None
    now = time.time()
    paths = ranked(exclude=here, now=now)
    if not paths:
        await app.notify("Recent Directories", "No folders yet: they're remembered as your shells visit them.")
        return
    items = []
    for p in paths[:200]:
        visits = round(dirs[p]["rank"])
        items.append((p, tilde(p), f"{visits} visit{'s' if visits != 1 else ''} · {ago(now - dirs[p]['last'])}"))
    chosen = await app.pick(items, placeholder="Go to folder…")
    if not chosen:
        return
    if pane is None or pane.id in busy:
        await app.new_tab(cwd=chosen)
    else:
        await pane.run(["cd", "--", chosen])


app.run()
