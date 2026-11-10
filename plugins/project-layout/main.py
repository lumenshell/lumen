"""Projects & Tasks. Open Project (Ctrl+Shift+L, ⌘⇧L on macOS) finds
.lumen-layout.toml in the focused pane's directory or the nearest one above
it, opens the tabs and splits it describes, and starts the project's dev
environment: each task in its own labelled split of a dev tab. With no dev
environment yet, it offers a checklist of what the project can run and saves
your choice; Edit Dev Environment changes it.

Run Task (Ctrl+Shift+K, ⌘⇧K) runs one saved or discovered task in a split
beside the pane. Run Command… runs a command you type there instead; Save
as Task, in its split, keeps it in the file. Add Task saves one directly.
Restart Task (Ctrl+Alt+R, ⌘⌥R) stops the task in the focused split and
runs it again. Needs shell integration (for the directory, and to see tasks
fail) and Python 3.11+ (for tomllib).

    # .lumen-layout.toml
    title = "api"                 # the tabs' name (optional)

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

    [[task]]                      # a task the dev environment can run
    name = "web"
    run = "npm run dev"
    cwd = "frontend"              # optional, relative to the layout file
    stop = "npm run clean"        # optional, run when its split closes

    [dev]                         # what Open Project starts, in a dev tab
    tasks = ["web"]               # saved tasks, or discovered ones by name

Every key is optional except a task's name and run. A file with only
`title` and [[pane]] opens as it always has; with no [[pane]] and no [dev],
Open Project offers the checklist. Tasks found in the project (compose services, scripts,
make/just targets, Procfile, Cargo) can be named in [dev] without a
[[task]]; a saved task with the same name wins.

A layout runs commands, and a cloned repository can ship one, so you're
shown every command and asked before anything runs (once per version of
the file; Run Task asks once per version of each saved or Procfile task,
except one you typed yourself).
"""

from __future__ import annotations

import asyncio
import os
import subprocess
import sys

try:
    import tomllib
except ImportError:  # Python < 3.11
    tomllib = None

from lumen_plugin import LumenError, Plugin, typeable
from checklist import Checklist
from discover import discover
from grid import MAX_PER_TAB, grid_plan
from layoutfile import NAME, Task, find_layout, load as load_layout, project_root, write_owned
from runner import (OpenGuard, Runner, add_task, all_tasks, approval_key, approved, check_new_task,
                    confirm_first, dev_tasks, label, merge_saved, needs_confirm, open_listing, remember,
                    rerun_text, saved_now, shown, stop_argv, task_approval, task_file, tilde)

app = Plugin()
MAC = sys.platform == "darwin"
KEYS = "cmd-shift-l" if MAC else "ctrl-shift-l"
RUN_KEYS = "cmd-shift-k" if MAC else "ctrl-shift-k"
RESTART_KEYS = "cmd-alt-r" if MAC else "ctrl-alt-r"
TITLE = "Open Project"
EDIT = "Edit Dev Environment"


def load(path):
    """The layout's title and panes, checked; raises ValueError if it's wrong."""
    layout = load_layout(path)
    if not layout.panes:
        raise ValueError("it needs at least one [[pane]]")
    return layout.title, layout.panes


def data_dir():
    """The plugin's data folder, or None if it can't be made."""
    try:
        return app.data_dir
    except OSError:
        return None


runner = Runner()
waiters = {}  # pane -> asyncio.Event set on its next command-finished
guard = OpenGuard()


async def guarded(pane, flow):
    """Run `flow(pane, token)` as the one checklist flow allowed at a time; a
    second press focuses the open checklist."""
    token = guard.enter()
    if not token:
        if guard.view is not None and not guard.view.closed:
            await guard.view.focus()
        return
    handed_off = False  # to the checklist, which leaves when it's done
    try:
        handed_off = await flow(pane, token)
    finally:
        if not handed_off:
            guard.leave(token)


@app.command(TITLE, keys=KEYS, id="open_layout")
async def open_layout(pane):
    await guarded(pane, open_project)


@app.command(EDIT)
async def edit_dev_environment(pane):
    async def edit(pane, token):
        proj = await project(pane, EDIT)
        return bool(proj) and await choose(pane, *proj[1:], token, edit=True)
    await guarded(pane, edit)


async def project(pane, title, cwd=None):
    """(cwd, root, layout file or None, Layout or None, discovered tasks) for
    the project at `cwd`, else at the pane's folder, or None once it has said
    why not."""
    if tomllib is None:
        await app.notify(title, "This needs Python 3.11 or newer.")
        return None
    cwd = cwd or (pane and await pane.cwd())
    if not cwd:
        await app.notify(title, f"No {NAME} here or in any folder above.")
        return None
    root, path = project_root(cwd), find_layout(cwd)
    found = discover(root)
    layout = None
    if path:
        try:
            layout = load_layout(path, known=frozenset(t.name for t in found))
        except (ValueError, OSError, tomllib.TOMLDecodeError) as e:
            await app.notify(title, f"{path}: {e}")
            return None
    return cwd, root, path, layout, found


async def open_project(pane, token):
    """Returns True if a checklist was opened (it then owns the guard)."""
    proj = await project(pane, TITLE)
    if not proj:
        return False
    _, root, path, layout, found = proj
    live = runner.open_tab(root, {p["id"] for p in await app.panes()})
    if live is not None:
        await app.pane(live).focus()
        return

    if layout is None or (not layout.panes and not layout.dev):
        return await choose(pane, root, path, layout, found, token)
    tasks = dev_tasks(layout, found)
    if not await ask(path, root, layout, tasks):
        return
    await start(root, layout, tasks)


async def ask(path, root, layout, tasks):
    """Show every command and ask, unless this exact set was approved before."""
    key = approval_key(path, layout.panes, tasks)
    count, listing = open_listing(path, layout.panes, tasks, root)
    if not count or approved(data_dir(), path, key):
        return True
    what = "project" if tasks else "layout"
    question = f"Open this {what} and run {count} command{'s' if count > 1 else ''}?"
    if not await app.confirm(question, message=listing, yes="Open and run", no="Cancel"):
        return False
    remember(data_dir(), path, key)
    return True


async def start(root, layout, tasks):
    panes = layout.panes
    first = None
    if panes:
        opened = [await app.new_tab(cwd=panes[0]["cwd"], run=panes[0]["run"])]
        if layout.title:
            await opened[0].set_title(str(layout.title))
        for p in panes[1:]:
            opened.append(await opened[p["of"] - 1].split(p["split"], cwd=p["cwd"], run=p["run"], focus=False))
        first = opened[0]
    name = str(layout.title or os.path.basename(root.rstrip(os.sep)) or root)
    for i in range(0, len(tasks), MAX_PER_TAB):
        chunk = tasks[i:i + MAX_PER_TAB]
        made = [await app.new_tab(cwd=chunk[0].cwd, run=chunk[0].run)]
        # Tracked as soon as made, so a task that exits at once is still seen.
        runner.track(root, made[0].id, made[0].id, chunk[0])
        await made[0].set_title(name if i == 0 else f"{name} {i // MAX_PER_TAB + 1}")
        for t, direction, of, size in grid_plan(len(chunk)):
            made.append(await made[of].split(direction, cwd=chunk[t].cwd, run=chunk[t].run, focus=False, size=size))
            runner.track(root, made[0].id, made[-1].id, chunk[t])
        for p, task in zip(made, chunk):
            await p.set_pane_style(label=label(task.name))
        await show_badge(made[0].id)  # "N running", unless one has failed already
        first = first or made[0]
    if first:
        await first.focus()


async def choose(pane, root, path, layout, found, token, edit=False):
    """The checklist: pick the dev environment, save it, and start it (or,
    for `edit`, only save it). Returns True once it's open; it leaves the
    guard when it's done."""
    saved = list(layout.tasks) if layout else []
    names = {t.name for t in saved}
    added = []
    ticked = set(layout.dev or []) if edit and layout else names
    checklist = Checklist(all_tasks(layout, found), ticked)
    info = await pane.info() or {}
    need = len(checklist.tasks) + 4
    size = need if need <= (info.get("rows") or 0) // 2 else 0.5
    view = guard.view = await pane.split("down", plugin_pane=True, focus=True, size=size)
    busy = [False]
    starting = [False]

    @view.on_close
    def closed():
        if not starting[0]:  # cancelled, or closed by you
            guard.leave(token)

    async def draw():
        if not view.closed:
            await view.draw(checklist.render(view.cols, view.rows))

    @view.on_resize
    async def resized():
        await draw()

    @view.on_key
    async def key(name):
        if busy[0] or view.closed:
            return
        busy[0] = True
        try:
            action = checklist.key(name)
            if action == "add":
                task = await ask_task(root, checklist.tasks, EDIT if edit else TITLE)
                if task:
                    checklist.add(task)
                    added.append(task)
            elif action == "cancel":
                await view.close()
                return
            elif action == "start":
                starting[0] = True
                try:
                    chosen = set(checklist.chosen())
                    keep = saved + added + [t for t in found if t.name in chosen and t.name not in names]
                    confirm = needs_confirm([t for t in checklist.tasks if t.name in chosen],
                                            {t.name for t in added})
                    await view.close()
                    target = path or os.path.join(root, NAME)
                    added_names = {t.name for t in added}
                    if edit:
                        await save_dev(target, keep, added_names, checklist.chosen(), found)
                    else:
                        await save_and_start(target, root, keep, added_names, checklist.chosen(), found, confirm)
                finally:
                    guard.leave(token)
                return
            await draw()
        finally:
            busy[0] = False
    return True


async def ask_task(root, tasks, title):
    """A new task from two prompts, not saved yet; None if you stopped or it
    was refused (with a notification saying why)."""
    command = await app.prompt("Command to run", message=f"Runs in {tilde(root)}")
    name = command and await name_task(command, tasks, title)
    return Task(name, command, root, None, "saved") if name else None


async def name_task(command, tasks, title):
    """Ask a name for `command`: None if left empty, or refused (and said why)."""
    name = await app.prompt("Name for this task", initial=(command.split() or [""])[0])
    if not name:
        return None
    try:
        check_new_task(tasks, name, command)
    except ValueError as e:
        await app.notify(title, str(e))
        return None
    return name


async def save_task(proj, command, cwd, title):
    """Name `command` and save it as a task of the project `proj`, run in
    `cwd`: the new Task, or None."""
    _, root, path, layout, found = proj
    tasks = all_tasks(layout, found)
    name = await name_task(command, tasks, title)
    if not name:
        return None
    target = path or os.path.join(root, NAME)
    try:
        task = add_task(path, root, tasks, name, command, cwd=cwd)[-1]
    except ValueError as e:
        await app.notify(title, str(e))
        return None
    except OSError as e:
        await app.notify(title, f"{target}: {e}")
        return None
    # You typed it, so Run Task needn't show it to you again.
    remember(data_dir(), *task_approval(target, task))
    return task


async def write_choice(path, keep, added, chosen, found, title):
    """Write the checklist's choice. The file is read again first, so tasks
    saved to it since the checklist opened (Add Task, Save as Task, a hand
    edit) are kept. Returns what was written, or None (having said why)."""
    try:
        tasks = merge_saved(keep, added, saved_now(path, frozenset(t.name for t in found)))
    except (ValueError, OSError) as e:
        await app.notify(title, f"{path}: {e}")
        return None
    try:
        write_owned(path, tasks, chosen)
    except (ValueError, OSError) as e:
        await app.notify(title, str(e) if isinstance(e, ValueError) else f"{path}: {e}")
        return None
    return tasks


async def save_dev(path, keep, added, chosen, found):
    """Edit Dev Environment's save: write the choice, start nothing."""
    if await write_choice(path, keep, added, chosen, found, EDIT) is not None:
        await app.notify(EDIT, f"Saved. {'⌘⇧L' if MAC else 'Ctrl+Shift+L'} opens it.")


async def save_and_start(path, root, keep, added, chosen, found, confirm):
    written = await write_choice(path, keep, added, chosen, found, TITLE)
    if written is None:
        return
    # A chosen task changed in the file since the checklist showed it: ask.
    confirm = confirm or any(w is not k and w.name in chosen for w, k in zip(written, keep))
    try:
        layout = load_layout(path, known=frozenset(t.name for t in found))
    except (ValueError, OSError) as e:
        await app.notify(TITLE, f"{path}: {e}")
        return
    tasks = dev_tasks(layout, found)
    # Tasks found or added just now were shown in the checklist; ones the
    # file already had may hide a cwd, a stop or the rest of a long command.
    # Either way the file keeps your choice.
    if confirm and not await ask(path, root, layout, tasks):
        return
    await start(root, layout, tasks)


@app.command("Run Task", keys=RUN_KEYS)
async def run_task(pane):
    proj = await project(pane, "Run Task")
    if not proj:
        return
    _, root, path, layout, found = proj
    tasks = all_tasks(layout, found)
    if not tasks:
        await app.notify("Run Task", "Nothing to run found here. Add one with Add Task.")
        return
    choice = await app.pick([(str(i), t.name, shown(t.run)) for i, t in enumerate(tasks)],
                            placeholder="Run a task")
    if choice is None:
        return
    task = tasks[int(choice)]
    # Lumen made a discovered task's command from a name in the project's
    # files, and the picker showed it; one from the layout file or a
    # Procfile is a command as written there, so ask.
    if confirm_first(task) and not await allow(task_file(path, task), task):
        return
    await run_split(pane, task.cwd, task.run, task)


async def allow(path, task):
    """Show the command, folder and stop of a task from the file at `path`
    and ask, unless this exact task was approved before."""
    slot, key = task_approval(path, task)
    if approved(data_dir(), slot, key):
        return True
    listing = f"{tilde(path)} runs: `{shown(task.run)}` in {tilde(task.cwd)}"
    if task.stop:
        listing += f"; and when its split closes: `{shown(task.stop)}`"
    if not await app.confirm(f"Run {task.name}?", message=listing, yes="Run", no="Cancel"):
        return False
    remember(data_dir(), slot, key)
    return True


async def run_split(pane, cwd, command, task=None):
    """Run `command` in a new split right of `pane`, labelled, and tracked
    for Restart Task and failure marks: as `task`, or as a one-off."""
    new = await pane.split("right", cwd=cwd, run=command)
    if task:
        runner.track_single(new.id, task)
    else:
        runner.one_offs[new.id] = (command, cwd)
    try:
        await new.set_pane_style(label=label(task.name if task else command))
    except LumenError:  # closed already
        pass


@app.command("Add Task")
async def add_task_command(pane):
    proj = await project(pane, "Add Task")
    if not proj:
        return
    root = proj[1]
    command = await app.prompt("Command to run", message=f"Runs in {tilde(root)}")
    if command and await save_task(proj, command, root, "Add Task"):
        await app.notify("Add Task", f"Saved. {'⌘⇧K' if MAC else 'Ctrl+Shift+K'} runs it.")


@app.command("Run Command…")
async def run_command(pane):
    if pane is None:
        return
    cwd = await pane.cwd()  # unknown: Lumen starts the split in the pane's folder
    command = await app.prompt("Command to run", message=cwd and f"Runs in {tilde(cwd)}")
    if not command:
        return
    if not typeable(command):
        await app.notify("Run Command…", "That has control or invisible characters in it.")
        return
    await run_split(pane, cwd, command)


@app.command("Save as Task")
async def save_as_task(pane):
    pid = pane and pane.id
    if pid not in runner.one_offs:
        await app.notify("Save as Task", "Run a command with Run Command… first.")
        return
    command, started_in = runner.one_offs[pid]
    # The folder it was started in (else the pane's now) picks the project
    # and is where the task runs.
    proj = await project(pane, "Save as Task", cwd=started_in)
    task = proj and await save_task(proj, command, proj[0], "Save as Task")
    if task and pid in runner.one_offs:  # still open
        # Now a task: Restart Task and failure marks go on working.
        runner.track_single(pid, task)
        try:
            await pane.set_pane_style(label=label(task.name))
        except LumenError:  # closed meanwhile
            pass


@app.command("Restart Task", keys=RESTART_KEYS)
async def restart_task(pane):
    pid = pane and pane.id
    task = runner.tasks_by_pane.get(pid)
    command = task.run if task else runner.one_offs.get(pid, (None,))[0]
    if command is None:  # not a task split: nothing to do
        return
    if pid in runner.restarting:
        return
    name = task.name if task else shown(command)
    # Only command events prove shell integration: Lumen knows a pane's cwd
    # without it (from /proc on Linux).
    integrated = runner.is_integrated(pid)
    runner.restarting.add(pid)
    try:
        stopped = integrated and not runner.is_running(pid)  # already finished
        for _, wait in [] if stopped else runner.restart_steps(integrated):
            waiter = waiters[pid] = asyncio.Event()
            await pane.send_text("\x03")
            if not integrated:
                await asyncio.sleep(wait)
                stopped = True
                break
            try:
                await asyncio.wait_for(waiter.wait(), wait)
                stopped = True
                break
            except asyncio.TimeoutError:
                pass
        if not stopped:
            await app.notify("Restart Task", f"Couldn't stop {name}")
            return
        runner.rerun(pid)
        info = await pane.info() or {}
        await pane.run(rerun_text(command, task and task.cwd, info.get("cwd")))
        await pane.set_pane_style(color=None)
        await show_badge(pid)
    finally:
        runner.restarting.discard(pid)
        waiters.pop(pid, None)


async def show_badge(pid):
    badge = runner.badge(pid)
    if badge:
        try:
            await app.pane(pid).set_tab_style(badge=badge[0], color=badge[1])
        except LumenError:  # the tab is closing
            pass


@app.on("command-started")
def command_started(event):
    if event.pane:
        runner.started(event.pane.id)


@app.on("command-finished")
async def command_finished(event):
    if not event.pane:
        return
    pid = event.pane.id
    mark = runner.failed(pid, event.get("exit"))
    if pid in waiters:
        waiters[pid].set()
    if mark:
        try:
            await event.pane.set_pane_style(color=mark[0])
        except LumenError:
            return
        await show_badge(pid)


@app.on("pane-closed")
async def pane_closed(event):
    if not event.pane:
        return
    pid = event.pane.id
    tab = runner.tab_of.get(pid)
    task = runner.forget(pid)
    rest = [p for p, t in runner.tab_of.items() if t == tab]
    if rest:
        await show_badge(rest[0])
    if task and task.stop:
        try:
            proc = await asyncio.create_subprocess_exec(
                *stop_argv(task.stop), cwd=task.cwd, stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            await proc.wait()
        except (OSError, ValueError):
            pass


app.run()
