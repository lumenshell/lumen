"""Build Status: when a build or test command finishes, the tab title shows
how it went ("✓ cargo test · 12s", "✗ make · exit 2") until the next command
starts. Tabs you've named yourself are left alone. Needs shell integration.
"""

import os
import re
import shlex

from lumen_plugin import Plugin

app = Plugin()

# A command counts as a build if it starts with one of these (after `sudo`,
# `time` or `env X=Y`). Add your own.
BUILDS = {
    "cargo": {"build", "test", "check", "clippy", "run", "nextest", "bench"},
    "make": None, "just": None, "ninja": None, "gradle": None, "./gradlew": None,
    "mvn": None, "bazel": None, "cmake": None, "go": {"build", "test", "vet"},
    "npm": {"test", "run", "ci", "install"}, "pnpm": None, "yarn": None, "bun": None,
    "pytest": None, "tox": None, "mix": None, "dotnet": None, "swift": None,
    "docker": {"build", "compose"}, "tsc": None,
}

# The tab titles we set, by pane, so we only ever change our own.
ours = {}
OUR_TITLE = re.compile(r"^[✓✗] .+ · \d+(s|m \d\ds)$")


def build_name(command: str):
    """"cargo test", "make" or None for a command line. In a chain like
    `cd api && cargo test`, the last build in it counts."""
    try:
        words = shlex.split(command.replace(";", " ; "))
    except ValueError:
        words = command.split()
    found = None
    segment = []
    for word in words + [";"]:
        if word in ("&&", "||", ";", "|", "&"):
            found = _build_in(segment) or found
            segment = []
        else:
            segment.append(word)
    return found


def _build_in(words):
    while words and (words[0] in ("sudo", "time", "env", "nice") or "=" in words[0]):
        words = words[1:]
    if not words:
        return None
    tool = words[0] if words[0].startswith("./") else os.path.basename(words[0])
    if tool not in BUILDS:
        return None
    subcommands = BUILDS[tool]
    if subcommands is None:
        return tool
    if len(words) > 1 and words[1] in subcommands:
        return f"{tool} {words[1]}"
    return None


def took(ms: int) -> str:
    s = ms // 1000
    return f"{s}s" if s < 60 else f"{s // 60}m {s % 60:02d}s"


async def tab_title(pane):
    """The pane's tab title, and whether someone named the tab; None if the
    pane has closed."""
    for tab in await app.tabs():
        if any(p["id"] == pane.id for p in tab["panes"]):
            return tab["title"], tab["custom_title"]
    return None


def is_ours(title: str) -> bool:
    """Did we set this title? Checked by its shape too, so titles left by an
    earlier run of this plugin (before Reload Plugins) are still ours."""
    return title in ours.values() or bool(OUR_TITLE.match(title))


@app.on("command-finished")
async def finished(event):
    name = build_name(event.get("command") or "")
    current = name and await tab_title(event.pane)
    if not current:
        return
    title, named = current
    if named and not is_ours(title):
        return  # you named this tab; leave it alone
    status = f"✓ {name}" if event["exit"] == 0 else f"✗ {name} · exit {event['exit']}"
    ours[event.pane.id] = f"{status} · {took(event['duration_ms'])}"
    await event.pane.set_title(ours[event.pane.id])


@app.on("command-started")
async def started(event):
    ours.pop(event.pane.id, None)
    current = await tab_title(event.pane)
    if current and current[1] and is_ours(current[0]):
        await event.pane.set_title(None)


@app.on("pane-closed")
def closed(event):
    ours.pop(event.pane.id, None)


app.run()
