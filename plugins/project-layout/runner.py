"""What Projects & Tasks decides without talking to Lumen: which panes run
which task, what to mark, what you've approved, and adding a task to the
file. main.py acts on these; keeping them here lets them be tested alone."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile

from lumen_plugin import quote, typeable
from layoutfile import NAME, Task, load, write_owned

LABEL_MAX = 32  # what Lumen draws of a split's label


def shown(command):
    """A command exactly as it'll be typed."""
    return command if isinstance(command, str) else " ".join(quote(w) for w in command)


def label(text):
    """`text` cut to a split label: at most 32 characters, ending in "…" if cut."""
    return text if len(text) <= LABEL_MAX else text[:LABEL_MAX - 1] + "…"


class Runner:
    """The dev tabs and task splits this plugin started and what to show for
    them."""

    def __init__(self):
        self.runs = {}  # project root -> pane ids of its open dev tab(s)
        self.tasks_by_pane = {}  # pane -> Task
        self.one_offs = {}  # pane -> (command, the folder it was started in or None)
        self.tab_of = {}  # task pane -> its dev tab's first pane
        self.failures = set()  # panes whose task failed
        self.done = set()  # panes whose command has finished
        self.integrated = set()  # panes we've had shell integration events from
        self.restarting = set()  # panes being stopped to run again

    def track(self, root, tab, pane, task):
        self.runs.setdefault(root, []).append(pane)
        self.tasks_by_pane[pane] = task
        self.tab_of[pane] = tab

    def track_single(self, pane, task):
        """A task run on its own (Run Task, Save as Task): restartable and
        marked, but not part of a dev tab."""
        self.one_offs.pop(pane, None)
        self.tasks_by_pane[pane] = task

    def badge(self, pane):
        """(badge, colour) for the dev tab `pane` is in, or None."""
        tab = self.tab_of.get(pane)
        if tab is None:
            return None
        panes = [p for p, t in self.tab_of.items() if t == tab]
        failed = sum(p in self.failures for p in panes)
        if failed:
            return f"{failed} failed", "red"
        running = sum(p not in self.done for p in panes)
        return (f"{running} running" if running else "done"), None

    def started(self, pane):
        self.integrated.add(pane)
        self.done.discard(pane)

    def failed(self, pane, exit):
        """On command-finished: the (label colour, tab badge) to show, or None
        for a pane that isn't a task, a task stopped for a restart, or an
        unknown exit. Ctrl+C (130) is a stop, not a failure: no colour."""
        self.integrated.add(pane)
        if pane not in self.tasks_by_pane and pane not in self.one_offs:
            return None
        self.done.add(pane)
        if pane in self.restarting or not isinstance(exit, int):
            return None
        if exit in (0, 130):
            self.failures.discard(pane)
            return ("green" if exit == 0 else None), None
        self.failures.add(pane)
        badge = self.badge(pane)
        return "red", badge and badge[0]

    def rerun(self, pane):
        self.failures.discard(pane)
        self.done.discard(pane)

    def is_integrated(self, pane):
        return pane in self.integrated

    def is_running(self, pane):
        return pane not in self.done

    def restart_steps(self, integrated):
        """Ctrl+C presses and how long to wait after each: for command-finished
        with shell integration, else just that long."""
        return [("ctrl-c", 5.0), ("ctrl-c", 3.0)] if integrated else [("ctrl-c", 1.0)]

    def forget(self, pane):
        """Drop a closed pane; returns its Task, if it had one."""
        for root, panes in list(self.runs.items()):
            if pane in panes:
                panes.remove(pane)
                if not panes:
                    del self.runs[root]
        for s in (self.failures, self.done, self.integrated, self.restarting):
            s.discard(pane)
        self.tab_of.pop(pane, None)
        self.one_offs.pop(pane, None)
        return self.tasks_by_pane.pop(pane, None)

    def open_tab(self, root, alive):
        """A pane of the project's dev tab still open (ids in `alive`), else
        None, having forgotten the dead ones."""
        for pane in list(self.runs.get(root, [])):
            if pane in alive:
                return pane
            self.forget(pane)
        return None


def stop_argv(stop):
    """A task's `stop` as argv: a string through the shell, as typed; a list as is."""
    return ["/bin/sh", "-c", stop] if isinstance(stop, str) else [str(w) for w in stop]


def approval_key(path, panes, tasks):
    """Hash of everything that will run, and where, for the file at `path`."""
    items = [["pane", p["cwd"], p["run"]] for p in panes if p["run"]]
    items += [["task", t.name, t.cwd, t.run, t.stop] for t in tasks]
    return hashlib.sha256((path + "\0" + json.dumps(items)).encode()).hexdigest()


def task_approval(path, task):
    """(slot, key) under which running `task` alone, from the file at `path`,
    is approved."""
    return path + "\0" + task.name, approval_key(path, [], [task])


def tilde(path):
    """`path` with your home folder shown as ~."""
    home = os.path.expanduser("~")
    return "~" + path[len(home):] if path == home or path.startswith(home + os.sep) else path


def open_listing(path, panes, tasks, root):
    """(how many commands, the text listing them) for Open Project's
    confirm: each pane's and task's command, a task's folder when it isn't
    `root`, and the stops."""
    def where(t):
        same = os.path.realpath(t.cwd) == os.path.realpath(root)
        return "" if same else f" in {tilde(t.cwd)}"
    commands = [f"`{shown(p['run'])}`" for p in panes if p["run"]]
    commands += [f"`{shown(t.run)}`{where(t)}" for t in tasks]
    stops = [f"`{shown(t.stop)}`" for t in tasks if t.stop]
    listing = f"{tilde(path)} runs: " + "  ·  ".join(commands)
    if stops:
        listing += "; and when a split closes: " + "  ·  ".join(stops)
    return len(commands), listing


def _approvals(folder):
    if not folder:
        return {}
    try:
        with open(os.path.join(folder, "approved.json"), encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def approved(folder, slot, key):
    """Was `key` approved for `slot` (a file's path, or a file's path and a
    task name), going by approved.json in `folder`?"""
    return _approvals(folder).get(slot) == key


def remember(folder, slot, key):
    """Note the approval; if it can't be saved, you're just asked again."""
    if not folder:
        return
    data = _approvals(folder)
    data[slot] = key
    tmp = None
    try:
        fd, tmp = tempfile.mkstemp(dir=folder, prefix=".approved.", suffix=".tmp")
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(data, f)
        os.replace(tmp, os.path.join(folder, "approved.json"))
    except OSError:
        if tmp:
            try:
                os.unlink(tmp)
            except OSError:
                pass


def confirm_first(task):
    """Must you see `task`'s command in full before it runs? Yes for one
    from the layout file or a Procfile: both are commands written as is in a
    file (maybe a cloned one). Lumen builds the others from a name."""
    return task.source in ("saved", "procfile")


def task_file(path, task):
    """The file `task`'s command comes from: its Procfile, else the layout
    file at `path`."""
    return os.path.join(task.cwd, "Procfile") if task.source == "procfile" else path


def needs_confirm(chosen, added):
    """Must starting these from the checklist ask first? Yes if any is a task
    from the layout file or a Procfile (not one added in this checklist): the
    checklist shows only a cut-down command, never its cwd or stop."""
    return any(confirm_first(t) and t.name not in added for t in chosen)


def rerun_text(command, task_cwd, pane_cwd):
    """A command to type to rerun a task in its own folder: prefixed with a
    cd when the shell is elsewhere, or we can't tell."""
    text = shown(command)
    if task_cwd and not (pane_cwd and os.path.realpath(pane_cwd) == os.path.realpath(task_cwd)):
        return f"cd {quote(task_cwd)} && {text}"
    return text


class OpenGuard:
    """One checklist flow at a time (Open Project or Edit Dev Environment; a
    checklist counts until it closes), so two quick presses can't start
    everything twice."""

    def __init__(self):
        self.token = 0
        self.busy = None
        self.view = None  # the open checklist, if any

    def enter(self):
        """A token for leave(), or 0 if one is already under way."""
        if self.busy:
            return 0
        self.token += 1
        self.busy = self.token
        return self.token

    def leave(self, token):
        if self.busy == token:
            self.busy = None
            self.view = None


def dev_tasks(layout, found):
    """The Tasks the file's [dev] names: saved ones win over discovered ones."""
    by_name = {t.name: t for t in found}
    by_name.update({t.name: t for t in layout.tasks})
    return [by_name[n] for n in layout.dev or []]


def all_tasks(layout, found):
    """Saved tasks, then discovered ones a saved task doesn't shadow."""
    saved = list(layout.tasks) if layout else []
    names = {t.name for t in saved}
    return saved + [t for t in found if t.name not in names]


def check_new_task(tasks, name, run):
    """Raise ValueError (a message for you) if `name`/`run` can't be a new task
    beside `tasks`."""
    if not typeable(name) or not typeable(run):
        raise ValueError("That has control or invisible characters in it.")
    if not name.strip() or not run.strip():
        raise ValueError("A task needs a name and a command.")
    if any(t.name == name for t in tasks):
        raise ValueError(f"There's already a task named {name!r}.")


def add_task(path, root, tasks, name, run, *, cwd=None):
    """Save a new task (run in `cwd`, else `root`) in the layout file at
    `path`, or a new one in `root`. `tasks` is every task you could see when
    you started, saved and discovered; the file is read again first, so a
    task saved or a hand edit made meanwhile is kept, and its [dev] list is
    written back unchanged. The name can't repeat any of them. Returns the
    file's saved tasks as written. Raises ValueError with a message for you."""
    target = path or os.path.join(root, NAME)
    found = [t for t in tasks if t.source != "saved"]
    known = frozenset(t.name for t in found)
    layout = load(target, known=known) if os.path.isfile(target) else None
    saved = list(layout.tasks) if layout else []
    check_new_task(saved + found + [t for t in tasks if t.source == "saved"], name, run)
    write_owned(target, saved + [Task(name, run, cwd or root, None, "saved")], layout and layout.dev)
    return saved_now(target, known)


def saved_now(path, known):
    """The saved tasks in the file at `path` as it is now ([] if there's
    none). Raises ValueError (TOML errors are ValueErrors too) if it's wrong."""
    if not os.path.isfile(path):
        return []
    return list(load(path, known=known).tasks)


def merge_saved(keep, added, now):
    """What a checklist writes: `keep` (its snapshot of saved tasks, what it
    added, chosen discovered ones), with a task the file has now in place of
    its copy unless the checklist added it, then every task saved to the file
    since it opened. So nothing saved meanwhile is lost."""
    current = {t.name: t for t in now}
    out = [current[t.name] if t.name in current and t.name not in added else t for t in keep]
    names = {t.name for t in out}
    return out + [t for t in now if t.name not in names]
