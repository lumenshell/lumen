"""Find what a project can run: compose services, package.json scripts, make
targets and so on. Only the project folder and its direct subfolders are
searched. Nothing here raises: a file that can't be read or parsed skips
only its own source.

Names are unique. When two tasks share a name, every one of them gets its
source in front (`make:test`, `npm:test`). A task found in a subfolder keeps
the folder in front too (`frontend:npm:test`). Exact repeats (the same source,
folder and name twice) are dropped. A name that still clashes after that
(a root script named `make:x` beside a Makefile target `x` in a folder
named `npm`: both become `npm:make:x`) gets a number: `npm:make:x 2`.
"""

from __future__ import annotations

import json
import os
import re

from lumen_plugin import quote, typeable
from layoutfile import Task

SKIP_DIRS = {"node_modules", "vendor", "target", ".git"}

_COMPOSE_FILES = ("compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml")
_SERVICE = re.compile(r"""^["']?([A-Za-z0-9][\w.-]*)["']?\s*:\s*(#.*)?$""")
_MAKE = re.compile(r"^([A-Za-z0-9][\w.-]*):(?!=)")
_JUST_NAME = re.compile(r"^@?([A-Za-z0-9_-]+)")
_JUST_KEYWORDS = {"set", "alias", "export", "import", "mod"}
_JUST_PRIVATE = re.compile(r"^\[.*\bprivate\b.*\]$")
_PROC = re.compile(r"^([A-Za-z0-9_-]+)\s*:\s*(\S.*)$")


def _entry(name, prefix, stop_prefix=None):
    """(name, run, stop) with the name quoted; None if it can't be typed safely."""
    if name.startswith("-"):  # would be read as an option by the tool
        return None
    try:
        return (name, prefix + quote(name), stop_prefix + quote(name) if stop_prefix else None)
    except ValueError:
        return None


def _read(folder, *names):
    """The text of the first of `names` that exists in `folder`, else None."""
    for name in names:
        path = os.path.join(folder, name)
        if os.path.isfile(path):
            with open(path, encoding="utf-8-sig", errors="replace") as f:
                return f.read()
    return None


def _compose(folder):
    text = _read(folder, *_COMPOSE_FILES)
    if text is None:
        return []
    out, inside, indent = [], False, None
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        width = len(line) - len(line.lstrip())
        if width == 0:
            inside = re.match(r"services:\s*(#.*)?$", stripped) is not None
            indent = None
            continue
        if not inside:
            continue
        if indent is None:
            indent = width
        if width == indent:
            m = _SERVICE.match(stripped)
            if m:
                svc = m.group(1)
                out.append(_entry(svc, "docker compose up ", "docker compose stop "))
    return out


def _scripts(folder, filename, prefix):
    text = _read(folder, filename)
    if text is None:
        return []
    scripts = json.loads(text).get("scripts")
    if not isinstance(scripts, dict):
        return []
    return [_entry(name, prefix) for name in scripts if isinstance(name, str)]


def _npm(folder):
    pm = "npm"
    for lock, manager in (("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"),
                          ("bun.lockb", "bun"), ("bun.lock", "bun")):
        if os.path.isfile(os.path.join(folder, lock)):
            pm = manager
            break
    return _scripts(folder, "package.json", pm + " run ")


def _composer(folder):
    return _scripts(folder, "composer.json", "composer run ")


def _artisan(folder):
    if os.path.isfile(os.path.join(folder, "artisan")):
        return [("serve", "php artisan serve", None)]
    return []


def _lines(folder, pattern, names, command):
    text = _read(folder, *names)
    if text is None:
        return []
    out = []
    for line in text.splitlines():
        m = pattern.match(line)
        if m and not m.group(1).startswith("."):
            out.append(_entry(m.group(1), command))
    return out


def _make(folder):
    return _lines(folder, _MAKE, ("Makefile", "makefile"), "make ")


def _recipe_colon(rest):
    """Is the first colon outside quotes a recipe colon (not `:=`)?"""
    quote_char = None
    for i, c in enumerate(rest):
        if quote_char:
            if c == quote_char:
                quote_char = None
        elif c in "'\"`":
            quote_char = c
        elif c == ":":
            return not rest.startswith("=", i + 1)
    return False


def _just(folder):
    text = _read(folder, "justfile", ".justfile")
    if text is None:
        return []
    out, private = [], False
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped.startswith("["):
            private = private or bool(_JUST_PRIVATE.match(stripped))
            continue
        was_private, private = private, False
        m = _JUST_NAME.match(line)
        if (not m or line[0] in " \t" or m.group(1) in _JUST_KEYWORDS
                or m.group(1).startswith("_") or was_private
                or not _recipe_colon(line[m.end():])):
            continue
        out.append(_entry(m.group(1), "just "))
    return out


def _procfile(folder):
    text = _read(folder, "Procfile")
    if text is None:
        return []
    out = []
    for line in text.splitlines():
        m = _PROC.match(line.strip()) if not line.lstrip().startswith("#") else None
        if m:
            out.append((m.group(1), m.group(2).strip(), None))
    return out


def _cargo(folder):
    if os.path.isfile(os.path.join(folder, "Cargo.toml")):
        return [("run", "cargo run", None), ("test", "cargo test", None)]
    return []


SOURCES = (
    ("compose", _compose), ("npm", _npm), ("composer", _composer), ("artisan", _artisan),
    ("make", _make), ("just", _just), ("procfile", _procfile), ("cargo", _cargo),
)


def _subfolders(root):
    try:
        names = sorted(os.listdir(root))
    except OSError:
        return []
    return [n for n in names
            if not n.startswith(".") and n not in SKIP_DIRS
            and os.path.isdir(os.path.join(root, n))]


def discover(root):
    """The tasks a project can run, root first, then subfolders by name; each
    folder in the order of SOURCES, then file order."""
    root = os.path.abspath(root)
    folders = [("", root)] + [(n, os.path.join(root, n)) for n in _subfolders(root)]
    found = []  # (subfolder, source, name, run, stop, cwd)
    seen = set()
    for sub, folder in folders:
        for source, finder in SOURCES:
            try:
                entries = finder(folder)
            except Exception:  # a bad file skips only this source
                continue
            for entry in entries:
                if entry is None:
                    continue
                name, run, stop = entry
                key = (sub, source, name)
                if key in seen or "\ufffd" in name + run or not typeable(name) or not typeable(run):
                    continue
                if stop is not None and not typeable(stop):
                    continue
                seen.add(key)
                found.append((sub, source, name, run, stop, folder))

    def plain(sub, name):
        return f"{sub}:{name}" if sub else name

    labels = [plain(sub, name) for sub, _s, name, *_ in found]
    prefixed = [False] * len(found)
    while True:  # prefix with the source whoever shares a name, until none do
        counts = {}
        for label in labels:
            counts[label] = counts.get(label, 0) + 1
        todo = [i for i, label in enumerate(labels) if counts[label] > 1 and not prefixed[i]]
        if not todo:
            break
        for i in todo:
            sub, source, name = found[i][:3]
            labels[i] = plain(sub, f"{source}:{name}")
            prefixed[i] = True
    taken = set(labels)  # a number added below must not meet any label either
    used = set()
    for i, label in enumerate(labels):
        if label in used:
            n = 2
            while f"{label} {n}" in taken or f"{label} {n}" in used:
                n += 1
            labels[i] = f"{label} {n}"
            taken.add(labels[i])
        used.add(labels[i])
    tasks = []
    for (sub, source, name, run, stop, folder), label in zip(found, labels):
        tasks.append(Task(name=label, run=run, cwd=folder, stop=stop, source=source))
    return tasks
