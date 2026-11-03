#!/usr/bin/env python3
"""Like `watch`, in a pane of its own: run a command every few seconds and
show its output in a pane below, with the lines that changed since last time
highlighted. Your shell stays free: the watcher runs in the background.

    watch_pane.py git status --short
    watch_pane.py -n 10 kubectl get pods

In the pane: space pauses, r runs it again now, q closes it. While the
command fails, the tab turns red with its exit status as a badge.

Needs Lumen's Python package; inside Lumen:

    PYTHONPATH="$(lumen python-path)" watch_pane.py ...

A plugin pane belongs to the connection that opened it, so the pane closes
when this script stops.
"""

import asyncio
import os
import subprocess
import sys
import time

from lumen_plugin import LumenError, Plugin


def parse(args):
    every = 2.0
    if args[:1] == ["-n"] and len(args) > 2:
        every, args = float(args[1]), args[2:]
    if not args:
        sys.exit(__doc__.split("\n\n")[1])
    return max(0.5, every), args


def background():
    """Carry on detached, so the shell that started this gets its prompt back."""
    if os.environ.get("WATCH_PANE_CHILD"):
        return
    env = dict(os.environ, WATCH_PANE_CHILD="1")
    subprocess.Popen([sys.executable, *sys.argv], env=env, start_new_session=True,
                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    sys.exit(0)


every, command = parse(sys.argv[1:])
background()
app = Plugin()


async def run_once():
    proc = await asyncio.create_subprocess_exec(
        *command, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT)
    out, _ = await proc.communicate()
    return proc.returncode, out.decode(errors="replace").expandtabs().splitlines()


async def main():
    here = app.pane(int(os.environ["LUMEN_PANE"])) if os.environ.get("LUMEN_PANE") else await app.focused()
    view = await here.split("down", plugin_pane=True, focus=False, size=0.4)
    state = {"paused": False, "previous": [], "last": ([], 0, "")}
    wake = asyncio.Event()
    done = asyncio.Event()

    async def draw():
        lines, status, stamp = state["last"]
        head = f"\x1b[1mevery {every:g}s: {' '.join(command)}\x1b[0m"
        right = "paused" if state["paused"] else stamp
        if status:
            right = f"\x1b[31mexit {status}\x1b[0m  " + right
        rows = [head + "   \x1b[2m" + right + "\x1b[0m", ""]
        for i, line in enumerate(lines[: max(0, view.rows - 3)]):
            line = line[: view.cols]
            changed = state["previous"] and (i >= len(state["previous"]) or state["previous"][i] != lines[i])
            rows.append(f"\x1b[30;43m{line}\x1b[0m" if changed else line)
        rows.append("")
        rows.append("\x1b[2mspace pause · r run now · q close\x1b[0m")
        try:
            await view.draw(rows)
        except LumenError:
            pass

    @view.on_key
    async def key(name):
        if name in ("q", "escape"):
            done.set()
        elif name == "space":
            state["paused"] = not state["paused"]
            await draw()
        elif name == "r":
            wake.set()

    view.on_resize(draw)
    view.on_close(done.set)

    async def loop():
        failing = False
        while True:
            if not state["paused"]:
                status, lines = await run_once()
                state["previous"] = state["last"][0]
                state["last"] = (lines, status, time.strftime("%H:%M:%S"))
                await draw()
                if (status != 0) != failing:
                    failing = status != 0
                    style = {"badge": f"exit {status}", "color": "red"} if failing else {"badge": None, "color": None}
                    try:
                        await here.set_tab_style(**style)
                    except LumenError:
                        pass
            wake.clear()
            try:
                await asyncio.wait_for(wake.wait(), timeout=every)
            except asyncio.TimeoutError:
                pass

    task = asyncio.ensure_future(loop())
    await done.wait()
    task.cancel()
    try:
        await here.set_tab_style(badge=None, color=None)
    except LumenError:
        pass  # that pane has closed too


app.run(main)
