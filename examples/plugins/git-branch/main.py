"""A Lumen plugin: "Switch Git Branch" (Alt+B) lists the branches of the
focused pane's repository in the palette and switches to the one you pick.

Lumen runs this with JSON-RPC 2.0 on stdin/stdout, one message per line.
stderr goes to the plugin's log ("Plugin Log: git-branch" in the palette).
"""

import json
import shlex
import subprocess
import sys


class Lumen:
    """A tiny client: requests wait for their reply; messages from Lumen that
    arrive meanwhile (commands, events) are kept for `messages()`."""

    def __init__(self):
        self.next_id = 0
        self.waiting = []

    def _send(self, msg):
        sys.stdout.write(json.dumps(msg) + "\n")
        sys.stdout.flush()

    def call(self, method, **params):
        self.next_id += 1
        self._send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        for line in sys.stdin:
            msg = json.loads(line)
            if msg.get("id") == self.next_id:
                if "error" in msg:
                    raise RuntimeError(msg["error"]["message"])
                return msg["result"]
            self.waiting.append(msg)
        sys.exit(0)  # Lumen closed stdin: time to go

    def messages(self):
        while True:
            while self.waiting:
                yield self.waiting.pop(0)
            line = sys.stdin.readline()
            if not line:
                return
            yield json.loads(line)


def git(cwd, *args):
    return subprocess.run(["git", "-C", cwd, *args], capture_output=True, text=True)


def pane_cwd(lumen, pane):
    for tab in lumen.call("list")["tabs"]:
        for p in tab["panes"]:
            if p["id"] == pane:
                return p["cwd"]
    return None


def switch_branch(lumen, pane):
    cwd = pane_cwd(lumen, pane)
    if not cwd or git(cwd, "rev-parse", "--git-dir").returncode != 0:
        lumen.call("notify", title="Switch Git Branch", body="This pane isn't in a git repository.")
        return
    current = git(cwd, "branch", "--show-current").stdout.strip()
    refs = git(cwd, "for-each-ref", "--sort=-committerdate",
               "--format=%(refname:short)\t%(committerdate:relative)\t%(subject)", "refs/heads").stdout
    items = []
    for line in refs.splitlines():
        name, when, subject = (line.split("\t", 2) + ["", ""])[:3]
        mark = "● " if name == current else ""
        items.append({"id": name, "label": mark + name, "detail": f"{when} · {subject[:50]}"})
    if not items:
        return
    chosen = lumen.call("show-picker", placeholder="Switch to branch…", items=items)["id"]
    print(f"picked {chosen!r} in {cwd}", file=sys.stderr)
    if chosen and chosen != current:
        # Branch names may contain $, ;, & and backticks (a cloned repo can
        # have any of them), so quote it before it reaches the shell.
        lumen.call("send-text", pane=pane, text=f"git switch -- {shlex.quote(chosen)}\r")


def main():
    lumen = Lumen()
    lumen.call("register-command", id="switch", title="Switch Git Branch", keys="alt-b")
    print("ready", file=sys.stderr)
    for msg in lumen.messages():
        if msg.get("method") == "command" and msg["params"]["id"] == "switch":
            try:
                switch_branch(lumen, msg["params"]["pane"])
            except Exception as e:  # keep running; the log says what happened
                print(f"error: {e}", file=sys.stderr)


if __name__ == "__main__":
    main()
