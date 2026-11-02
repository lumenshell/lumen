#!/usr/bin/env python3
"""Talk to Lumen's socket directly, without `lumen msg`: print a running log
of what happens in every pane, with the command each one ran.

    pane_log.py

    21:04:11  pane 2  opened
    21:04:15  pane 2  ✓ cargo build              12.3s  ~/code/lumen
    21:04:40  pane 2  ✗ cargo test (exit 101)     8.0s  ~/code/lumen

A starting point for your own tools: the protocol is JSON-RPC 2.0, one
message per line, on the socket in $LUMEN_SOCKET (set inside Lumen panes).
Commands are reported by shell integration.
"""

import json
import os
import socket
import sys
import time


class Lumen:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.file = self.sock.makefile("r", encoding="utf-8")
        self.next_id = 0
        self.events = []  # events that arrived while waiting for a reply

    def call(self, method, **params):
        self.next_id += 1
        msg = {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}
        self.sock.sendall((json.dumps(msg) + "\n").encode())
        while True:
            reply = json.loads(self.file.readline())
            if reply.get("method") == "event":
                self.events.append(reply["params"])
            elif reply.get("id") == self.next_id:
                if "error" in reply:
                    raise RuntimeError(reply["error"]["message"])
                return reply["result"]

    def next_event(self):
        if self.events:
            return self.events.pop(0)
        line = self.file.readline()
        if not line:
            raise EOFError("Lumen closed the connection")
        return json.loads(line)["params"]


def main():
    path = os.environ.get("LUMEN_SOCKET")
    if not path:
        sys.exit("Run this inside a Lumen pane (LUMEN_SOCKET isn't set).")
    # One connection for events, one for questions, so replies and events
    # don't have to be untangled.
    events = Lumen(path)
    ask = Lumen(path)
    events.call("subscribe", events=["pane-opened", "pane-closed", "command-finished"])

    # Short numbers for the long pane ids, in the order we meet them.
    short = {}
    home = os.path.expanduser("~")

    def name(pane):
        return f"pane {short.setdefault(pane, len(short) + 1)}"

    def pane_info(pane):
        for tab in ask.call("list")["tabs"]:
            for p in tab["panes"]:
                if p["id"] == pane:
                    return p
        return {}

    def log(pane, text):
        print(f"{time.strftime('%H:%M:%S')}  {name(pane):<8} {text}", flush=True)

    print("Watching Lumen; Ctrl+C to stop.", flush=True)
    while True:
        e = events.next_event()
        pane = e["pane"]
        if e["event"] == "pane-opened":
            log(pane, "opened")
        elif e["event"] == "pane-closed":
            log(pane, "closed")
        elif e["event"] == "command-finished":
            cmd = e.get("command") or "?"
            code = e.get("exit")
            mark = "✓" if code in (0, None) else "✗"
            what = cmd if code in (0, None) else f"{cmd} (exit {code})"
            cwd = (pane_info(pane).get("cwd") or "").replace(home, "~", 1)
            log(pane, f"{mark} {what:<28} {e['duration_ms'] / 1000:5.1f}s  {cwd}")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
