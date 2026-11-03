"""Command History: every command you run in Lumen, from every pane and
session, with its exit status, how long it took and where it ran.

- Search Command History (Ctrl+Shift+H, ⌘⇧H on macOS): pick one and it's
  typed into the pane, without pressing Enter.
- Rerun Last Failed Command (Ctrl+Shift+R, ⌘⇧R): the last command that failed
  in this pane (or anywhere, if none did here), shown with its folder to
  confirm, then run again there. (It asks because a program's output can
  fake a "command", for instance a file shown with `cat`.)
- Clear Command History: forget everything.

Commands starting with a space aren't kept, as in bash and zsh. The history
file is readable only by you. Needs shell integration.
"""

import json
import os
import sys
import time

from lumen_plugin import Plugin, quote

app = Plugin()
MAC = sys.platform == "darwin"
FILE = os.path.join(app.data_dir, "history.jsonl")
KEEP = 10_000           # entries kept
INTERRUPTED = {130}     # Ctrl+C isn't a failure worth rerunning

history = []            # oldest first: {"command", "exit", "ms", "cwd", "at", "pane"}
cwds = {}               # pane -> its shell's directory
busy = set()            # panes running a command


def load():
    global history
    history = []
    try:
        with open(FILE) as f:
            for line in f:
                try:
                    entry = json.loads(line)
                except ValueError:
                    continue
                if isinstance(entry, dict) and entry.get("command"):
                    history.append(entry)
    except OSError:
        pass
    history = history[-KEEP:]


def append(entry):
    history.append(entry)
    fd = os.open(FILE, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    with os.fdopen(fd, "a") as f:
        f.write(json.dumps(entry) + "\n")
    if len(history) > KEEP * 1.2:
        rewrite(history[-KEEP:])


def rewrite(entries):
    global history
    history = list(entries)
    tmp = FILE + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        for entry in history:
            f.write(json.dumps(entry) + "\n")
    os.replace(tmp, FILE)


def took(ms):
    s = ms / 1000
    if s < 10:
        return f"{s:.1f}s"
    s = int(s)
    return f"{s}s" if s < 60 else f"{s // 60}m {s % 60:02d}s"


def ago(seconds):
    for unit, size in (("d", 86400), ("h", 3600), ("m", 60)):
        if seconds >= size:
            return f"{int(seconds // size)}{unit} ago"
    return "just now"


def tilde(path):
    home = os.path.expanduser("~")
    if path and (path == home or path.startswith(home + os.sep)):
        return "~" + path[len(home):]
    return path or "?"


def describe(entry, now):
    status = "✓" if entry["exit"] == 0 else f"✗ exit {entry['exit']}"
    return f"{status} · {took(entry['ms'])} · {tilde(entry.get('cwd'))} · {ago(now - entry['at'])}"


def newest_unique(entries):
    """Newest first, each command once (its latest run)."""
    seen, out = set(), []
    for entry in reversed(entries):
        if entry["command"] not in seen:
            seen.add(entry["command"])
            out.append(entry)
    return out


def last_failure(entries, pane_id):
    failures = [e for e in entries if e["exit"] != 0 and e["exit"] not in INTERRUPTED]
    here = [e for e in failures if e.get("pane") == pane_id]
    return (here or failures or [None])[-1]


load()


@app.on_start
async def seed():
    for pane in await app.panes():
        cwds[pane["id"]] = pane.get("cwd")


@app.on("cwd-changed")
def moved(event):
    cwds[event.pane.id] = event["cwd"]


@app.on("command-started")
def started(event):
    busy.add(event.pane.id)


@app.on("pane-closed")
def closed(event):
    busy.discard(event.pane.id)
    cwds.pop(event.pane.id, None)


@app.on("command-finished")
def finished(event):
    busy.discard(event.pane.id)
    command = event.get("command") or ""
    if not command.strip() or command.startswith(" "):
        return
    append({
        "command": command.strip(),
        "exit": event["exit"],
        "ms": event["duration_ms"],
        "cwd": cwds.get(event.pane.id),
        "at": time.time(),
        "pane": event.pane.id,
    })


async def ready(pane):
    """Is the pane at a prompt? Typing into a running program would go wrong."""
    if pane is None:
        return False
    if pane.id in busy:
        await app.notify("Command History", "That pane is busy running something.")
        return False
    return True


@app.command("Search Command History", keys="cmd-shift-h" if MAC else "ctrl-shift-h")
async def search(pane):
    entries = newest_unique(history)[:500]
    if not entries:
        await app.notify("Command History", "Nothing yet: commands are kept as they finish.")
        return
    now = time.time()
    items = [(i, e["command"], describe(e, now)) for i, e in enumerate(entries)]
    chosen = await app.pick(items, placeholder="Search command history…")
    if chosen is not None and await ready(pane):
        await pane.send_text(entries[chosen]["command"])


@app.command("Rerun Last Failed Command", keys="cmd-shift-r" if MAC else "ctrl-shift-r")
async def rerun(pane):
    entry = last_failure(history, pane.id if pane else None)
    if entry is None:
        await app.notify("Command History", "No failed commands to rerun.")
        return
    if not await ready(pane):
        return
    command = entry["command"]
    where = entry.get("cwd")
    if where and where != cwds.get(pane.id) and os.path.isdir(where):
        command = f"cd -- {quote(where)} && {command}"
    message = f"`{entry['command']}` failed (exit {entry['exit']}) in {tilde(where)} {ago(time.time() - entry['at'])}."
    if not await app.confirm("Run it again?", message=message, yes="Run it again", no="Cancel"):
        return
    try:
        await pane.run(command)
    except ValueError:  # control characters: never type those
        await app.notify("Command History", "That command can't be typed safely.")


@app.command("Clear Command History")
async def clear(pane):
    if history and await app.confirm(f"Forget all {len(history)} commands?", yes="Clear history", no="Keep it"):
        rewrite([])
        await app.notify("Command History", "History cleared.")


app.run()
