"""SSH to Host (Ctrl+Shift+S, ⌘⇧S on macOS): pick a host from ~/.ssh/config
(and the files it Includes) and open `ssh <host>` in a new tab named after it.
"""

import glob
import os
import sys

from lumen_plugin import Plugin

app = Plugin()
KEYS = "cmd-shift-s" if sys.platform == "darwin" else "ctrl-shift-s"
SSH_DIR = os.path.expanduser("~/.ssh")


def read_config(path, hosts, seen):
    """Collect concrete Host entries ({name: details}) from an ssh config."""
    path = os.path.realpath(path)
    if path in seen or not os.path.isfile(path):
        return
    seen.add(path)
    current = []
    with open(path, errors="replace") as f:
        for line in f:
            words = line.split("#", 1)[0].replace("=", " ", 1).split()
            if len(words) < 2:
                continue
            key, values = words[0].lower(), words[1:]
            if key == "include":
                for pattern in values:
                    pattern = os.path.expanduser(pattern)
                    if not os.path.isabs(pattern):
                        pattern = os.path.join(SSH_DIR, pattern)
                    for included in sorted(glob.glob(pattern)):
                        read_config(included, hosts, seen)
            elif key == "host":
                # Patterns (*, ?, !) aren't hosts you can connect to by name.
                current = [h for h in values if not any(c in h for c in "*?!")]
                for h in current:
                    hosts.setdefault(h, {})
            elif key == "match":
                current = []
            elif key in ("hostname", "user", "port"):
                for h in current:
                    hosts[h].setdefault(key, values[0])


def describe(details):
    where = details.get("hostname", "")
    if "user" in details:
        where = f"{details['user']}@{where}" if where else details["user"] + "@"
    if "port" in details:
        where += f":{details['port']}"
    return where


@app.command("SSH to Host", keys=KEYS)
async def ssh(pane):
    hosts = {}
    read_config(os.path.join(SSH_DIR, "config"), hosts, set())
    if not hosts:
        await app.notify("SSH to Host", "No hosts found in ~/.ssh/config.")
        return
    items = [(name, name, describe(d)) for name, d in sorted(hosts.items())]
    host = await app.pick(items, placeholder="SSH to…")
    if host:
        tab = await app.new_tab(run=["ssh", host])  # a list: quoted for the shell
        await tab.set_title(host)


app.run()
