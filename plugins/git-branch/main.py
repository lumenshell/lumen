"""Switch Git Branch (Alt+B): pick a branch of the focused pane's repository
in the palette, and switch to it."""

import subprocess

from lumen_plugin import Plugin

app = Plugin()


def git(cwd, *args):
    return subprocess.run(["git", "-C", cwd, *args], capture_output=True, text=True)


@app.command("Switch Git Branch", keys="alt-b")
async def switch(pane):
    cwd = await pane.cwd()
    if not cwd or git(cwd, "rev-parse", "--git-dir").returncode != 0:
        await app.notify("Switch Git Branch", "This pane isn't in a git repository.")
        return
    current = git(cwd, "branch", "--show-current").stdout.strip()
    refs = git(cwd, "for-each-ref", "--sort=-committerdate",
               "--format=%(refname:short)\t%(committerdate:relative)\t%(subject)", "refs/heads").stdout
    branches = []
    for line in refs.splitlines():
        name, when, subject = (line.split("\t", 2) + ["", ""])[:3]
        mark = "● " if name == current else ""
        branches.append((name, mark + name, f"{when} · {subject[:50]}"))
    chosen = await app.pick(branches, placeholder="Switch to branch…")
    print(f"picked {chosen!r} in {cwd}")  # goes to the plugin's log
    if chosen and chosen != current:
        # A list is quoted word by word: branch names can contain $, ; and `.
        await pane.run(["git", "switch", "--", chosen])


app.run()
