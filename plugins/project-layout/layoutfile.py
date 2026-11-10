"""The project layout file (.lumen-layout.toml): reading, checking, and
writing back only the parts the plugin owns ([[task]] and [dev]).

Reading needs Python 3.11+ (tomllib); writing does not.
"""

from __future__ import annotations

import os
import re
import subprocess
import tempfile
from dataclasses import dataclass, field
from typing import List, Optional, Union

try:
    import tomllib
except ImportError:  # Python < 3.11
    tomllib = None

from lumen_plugin import typeable

NAME = ".lumen-layout.toml"

_TASK_KEYS = {"name", "run", "cwd", "stop"}
_DEV_KEYS = {"tasks"}
_OWNED = re.compile(r"^\s*(\[\[\s*task\s*\]\]|\[\s*dev\s*\])\s*(#.*)?$")


@dataclass
class Task:
    name: str
    run: Union[str, List[str]]
    cwd: str
    stop: Union[str, List[str], None] = None
    source: str = "saved"
    # The cwd as written in the file, so writing an unchanged task back keeps
    # it ("~/x" stays "~/x"). Not part of what the task is.
    cwd_text: Optional[str] = field(default=None, compare=False, repr=False)


@dataclass
class Layout:
    title: Optional[str] = None
    panes: list = field(default_factory=list)
    tasks: List[Task] = field(default_factory=list)
    dev: Optional[List[str]] = None


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


def project_root(start):
    """The folder of the layout file above `start`, else the git root, else `start`."""
    path = find_layout(start)
    if path:
        return os.path.dirname(path)
    try:
        out = subprocess.run(["git", "rev-parse", "--show-toplevel"], cwd=start,
                             capture_output=True, text=True, timeout=5)
        top = out.stdout.strip()
        if out.returncode == 0 and top:
            return top
    except (OSError, subprocess.SubprocessError):
        pass
    return start


def _words(value):
    return [value] if isinstance(value, str) else [str(w) for w in value or []]


def _check_command(what, key, value):
    if value is not None and not isinstance(value, (str, list)):
        raise ValueError(f"{what}: {key} should be a string or a list")
    # A carriage return or escape code would press keys, and invisible
    # characters (like right-to-left overrides) could make a command look
    # different in the confirmation; refuse the layout.
    if not all(typeable(w) for w in _words(value)):
        raise ValueError(f"{what}: {key} contains control or invisible characters")


def _check_keys(what, table, allowed):
    # Writing the file back keeps only these keys, so refuse others rather
    # than drop them.
    for key in table:
        if key not in allowed:
            raise ValueError(f"{what}: unknown key {key!r}")


def _cwd_text(base, t):
    """What to write as `t`'s cwd: as the file had it if that's still where
    it runs, else relative to `base`; None for `base` itself."""
    if t.cwd_text is not None and \
            os.path.normpath(os.path.join(base, os.path.expanduser(t.cwd_text))) == t.cwd:
        return t.cwd_text
    try:
        rel = os.path.relpath(t.cwd, base)
    except ValueError:  # different drive
        rel = t.cwd
    return None if rel == "." else rel


def load(path, known=frozenset()):
    """The layout, checked; raises ValueError naming the bad entry."""
    with open(path, "rb") as f:
        data = tomllib.load(f)
    base = os.path.dirname(os.path.abspath(path))

    panes = data.get("pane") or []
    if not isinstance(panes, list):
        raise ValueError("pane should be a list of [[pane]] tables")
    out = []
    for i, p in enumerate(panes, 1):
        if i > 1 and p.get("split", "right") not in ("right", "down"):
            raise ValueError(f"pane {i}: split should be \"right\" or \"down\"")
        of = p.get("of", i - 1)
        if i > 1 and not (isinstance(of, int) and 1 <= of < i):
            raise ValueError(f"pane {i}: `of` should be the number of an earlier pane")
        cwd = os.path.normpath(os.path.join(base, os.path.expanduser(p.get("cwd", "."))))
        run = p.get("run")
        _check_command(f"pane {i}", "run", run)
        out.append({"split": p.get("split", "right"), "of": of, "cwd": cwd, "run": run})

    raw_tasks = data.get("task") or []
    if not isinstance(raw_tasks, list):
        raise ValueError("task should be a list of [[task]] tables")
    tasks, seen = [], set()
    for i, t in enumerate(raw_tasks, 1):
        if not isinstance(t, dict):
            raise ValueError(f"task {i}: it should be a [[task]] table")
        name = t.get("name")
        if not isinstance(name, str) or not name.strip():
            raise ValueError(f"task {i}: name should be a non-empty string")
        if not typeable(name):
            raise ValueError(f"task {i}: name contains control or invisible characters")
        what = f"task {name!r}"
        _check_keys(what, t, _TASK_KEYS)
        if name in seen:
            raise ValueError(f"{what}: name is used twice")
        seen.add(name)
        run = t.get("run")
        if run is None or run == "" or run == []:
            raise ValueError(f"{what}: it needs a run")
        _check_command(what, "run", run)
        stop = t.get("stop")
        _check_command(what, "stop", stop)
        cwd_text = t.get("cwd", ".")
        if not isinstance(cwd_text, str) or not typeable(cwd_text):
            raise ValueError(f"{what}: cwd should be a plain string")
        cwd = os.path.normpath(os.path.join(base, os.path.expanduser(cwd_text)))
        if not os.path.isdir(cwd):
            raise ValueError(f"{what}: cwd {cwd_text!r} is not a folder")
        tasks.append(Task(name, run, cwd, stop, "saved", cwd_text if "cwd" in t else None))

    dev = None
    if "dev" in data:
        table = data["dev"]
        names = table.get("tasks") if isinstance(table, dict) else None
        if isinstance(table, dict):
            _check_keys("[dev]", table, _DEV_KEYS)
        if not isinstance(names, list) or not all(isinstance(n, str) for n in names):
            raise ValueError("[dev]: tasks should be a list of task names")
        ok = seen | set(known)
        for n in names:
            if n not in ok:
                raise ValueError(f"[dev]: no task named {n!r}")
        dev = list(names)

    if not out and not tasks and dev is None:
        raise ValueError("it needs at least one [[pane]]")
    title = data.get("title")
    return Layout(title, out, tasks, dev)


def toml_str(value):
    """A TOML basic string for `value`."""
    parts = ['"']
    for c in str(value):
        if c == "\\":
            parts.append("\\\\")
        elif c == '"':
            parts.append('\\"')
        elif c == "\n":
            parts.append("\\n")
        elif c == "\t":
            parts.append("\\t")
        elif c == "\r":
            parts.append("\\r")
        elif c == "\b":
            parts.append("\\b")
        elif c == "\f":
            parts.append("\\f")
        elif ord(c) < 0x20 or ord(c) == 0x7F:
            parts.append("\\u%04x" % ord(c))
        else:
            parts.append(c)
    parts.append('"')
    return "".join(parts)


def _command(value):
    if isinstance(value, str):
        return toml_str(value)
    return "[" + ", ".join(toml_str(w) for w in value) + "]"


def _render(path, tasks, dev):
    # Relative to the folder the file was opened from, as load() reads it
    # (not its real path: through a symlink they differ).
    base = os.path.dirname(os.path.abspath(path))
    blocks = []
    for t in tasks:
        lines = ["[[task]]", f"name = {toml_str(t.name)}", f"run = {_command(t.run)}"]
        rel = _cwd_text(base, t)
        if rel is not None:
            lines.append(f"cwd = {toml_str(rel)}")
        if t.stop:
            lines.append(f"stop = {_command(t.stop)}")
        blocks.append("\n".join(lines) + "\n")
    if dev is not None:
        blocks.append("[dev]\ntasks = [" + ", ".join(toml_str(n) for n in dev) + "]\n")
    return "\n".join(blocks)


def _check_safe(path, old, new):
    """Refuse a write that wouldn't parse, or would change anything but the
    task and dev keys. Skipped when tomllib isn't available."""
    if tomllib is None:
        return
    msg = f"Couldn't update {path} safely; edit its [[task]]/[dev] sections by hand."
    try:
        before = tomllib.loads(old) if old else {}
        after = tomllib.loads(new)
    except tomllib.TOMLDecodeError:
        raise ValueError(msg) from None
    for d in (before, after):
        d.pop("task", None)
        d.pop("dev", None)
    if before != after:
        raise ValueError(msg)


def _scan(line, state):
    """Advance the (multi-line string delimiter, bracket depth) state over one line."""
    ml, depth = state
    i, n = 0, len(line)
    while i < n:
        if ml:
            if line[i] == "\\" and ml == '"""':
                i += 2
            elif line.startswith(ml, i):
                i += 3
                while i < n and line[i] == ml[0]:  # up to two extra quotes belong to the string
                    i += 1
                ml = None
            else:
                i += 1
            continue
        c = line[i]
        if c == "#":
            break
        if c in "\"'":
            if line.startswith(c * 3, i):
                ml = c * 3
                i += 3
                continue
            i += 1
            while i < n and line[i] != c:
                i += 2 if (c == '"' and line[i] == "\\") else 1
            i += 1
            continue
        if c in "[{":
            depth += 1
        elif c in "]}":
            depth -= 1
        i += 1
    return ml, depth


def write_owned(path, tasks, dev):
    """Replace every [[task]] block and the [dev] block with fresh ones at the
    end of the file; every other line stays byte-for-byte. Task folders are
    written relative to the folder `path` is in, as given; the file itself is
    replaced at its real path, so a symlink to it stays one."""
    given, path = path, os.path.realpath(path)
    try:
        with open(path, "r", encoding="utf-8", newline="") as f:
            old = f.read()
    except FileNotFoundError:
        old = ""
    eol = "\r\n" if "\r\n" in old else "\n"
    kept, dropped, owned, state = [], [], False, (None, 0)
    for line in old.splitlines(keepends=True):
        text = line.rstrip("\r\n")
        if state == (None, 0) and text.lstrip().startswith("["):
            if owned:
                # Comment lines directly above the next header belong to it.
                tail = []
                while dropped and dropped[-1].lstrip().startswith("#"):
                    tail.insert(0, dropped.pop())
                kept.extend(tail)
            owned = bool(_OWNED.match(text))
            dropped = []
        state = _scan(text, state)
        if owned:
            dropped.append(line)
        else:
            kept.append(line)
    if owned:  # comments at the end of the file stay
        while dropped and not dropped[-1].strip():
            dropped.pop()
        tail = []
        while dropped and dropped[-1].lstrip().startswith("#"):
            tail.insert(0, dropped.pop())
        kept.extend(tail)
    while kept and not kept[-1].strip():
        kept.pop()
    head = "".join(kept)
    new = _render(given, tasks, dev).replace("\n", eol)
    if head and new:
        head += ("" if head.endswith("\n") else eol) + eol
    text = head + new
    _check_safe(path, old, text)
    folder = os.path.dirname(os.path.abspath(path))
    fd, tmp = tempfile.mkstemp(dir=folder, prefix=".lumen-layout.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="") as f:
            f.write(text)
        try:
            os.chmod(tmp, os.stat(path).st_mode & 0o7777)
        except FileNotFoundError:
            os.chmod(tmp, 0o644)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
