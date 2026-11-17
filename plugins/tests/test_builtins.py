"""Tests for the logic in Lumen's built-in plugins, without a running Lumen.

    python3 -m unittest discover -s plugins/tests
"""

import json
import re
import os
import runpy
import stat
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
PLUGINS = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(PLUGINS, "..", "python"))


import lumen_plugin  # noqa: E402

lumen_plugin.Plugin.run = lambda self, main=None: None  # load a plugin without connecting


def load(name):
    """Run a plugin's main.py with its own fresh data folder; return its globals."""
    os.environ["LUMEN_PLUGIN_DATA"] = tempfile.mkdtemp()
    os.environ["LUMEN_PLUGIN"] = name
    return runpy.run_path(os.path.join(PLUGINS, name, "main.py"))


class RecentDirsTests(unittest.TestCase):
    def setUp(self):
        self.p = load("recent-dirs")
        self.a, self.b = tempfile.mkdtemp(), tempfile.mkdtemp()

    def test_frequent_and_recent_folders_rank_first(self):
        visit, ranked = self.p["visit"], self.p["ranked"]
        now = 1_000_000
        for _ in range(5):
            visit(self.a, now - 30 * 86400)   # often, but a month ago
        visit(self.b, now - 60)               # once, a minute ago
        self.assertEqual(ranked(now=now), [self.b, self.a])
        for _ in range(20):
            visit(self.a, now - 30 * 86400)
        self.assertEqual(ranked(now=now), [self.a, self.b])
        self.assertEqual(ranked(exclude=self.a, now=now), [self.b])

    def test_recent_dirs_list_follows_the_setting(self):
        folders = [tempfile.mkdtemp() for _ in range(5)]
        for f in folders:
            self.p["visit"](f)
        self.p["app"].settings["max-shown"] = 3
        items = self.p["picker_items"](self.p["ranked"](now=2_000_000_000), now=2_000_000_000)
        self.assertEqual(len(items), 3)
        self.p["app"].settings.clear()
        self.assertEqual(len(self.p["picker_items"](self.p["ranked"](), now=2_000_000_000)), 5, "default 200")

    def test_folders_that_are_gone_or_relative_are_skipped(self):
        self.p["visit"]("relative/path")
        self.p["visit"](self.a)
        os.rmdir(self.a)
        self.assertEqual(self.p["ranked"](), [])

    def test_ranks_age_so_old_folders_are_forgotten(self):
        d = self.p["dirs"]
        for i in range(self.p["MAX_TOTAL"] + 10):
            self.p["visit"](self.a if i % 2 else self.b)
        self.assertLessEqual(sum(e["rank"] for e in d.values()), self.p["MAX_TOTAL"])

    def test_saved_privately_and_loaded_back(self):
        self.p["visit"](self.a)
        self.p["save"]()
        path = self.p["FILE"]
        self.assertEqual(stat.S_IMODE(os.stat(path).st_mode), 0o600)
        self.p["load"]()
        self.assertIn(self.a, self.p["dirs"])


class CommandHistoryTests(unittest.TestCase):
    def setUp(self):
        self.p = load("command-history")

    def event(self, command, exit=0, pane=1, ms=1500):
        return lumen_plugin.Event(None, {"event": "command-finished", "pane": pane,
                                         "command": command, "exit": exit, "duration_ms": ms})

    def test_finished_commands_are_kept_but_not_ones_starting_with_a_space(self):
        finished = self.p["finished"]
        self.p["cwds"][1] = "/src"
        finished(self.event("make", exit=2))
        finished(self.event(" export TOKEN=secret"))
        finished(self.event("   "))
        history = self.p["history"]
        self.assertEqual([e["command"] for e in history], ["make"])
        self.assertEqual((history[0]["exit"], history[0]["cwd"], history[0]["ms"]), (2, "/src", 1500))
        with open(self.p["FILE"]) as f:
            self.assertEqual(json.loads(f.readline())["command"], "make")
        self.assertEqual(stat.S_IMODE(os.stat(self.p["FILE"]).st_mode), 0o600)

    def test_search_shows_each_command_once_newest_first(self):
        e = lambda c, ex=0: {"command": c, "exit": ex, "ms": 10, "cwd": "/", "at": 0, "pane": 1}
        out = self.p["newest_unique"]([e("ls"), e("make", 2), e("ls"), e("git status")])
        self.assertEqual([x["command"] for x in out], ["git status", "ls", "make"])

    def test_rerun_prefers_this_panes_failure_and_skips_ctrl_c(self):
        e = lambda c, ex, pane: {"command": c, "exit": ex, "ms": 10, "cwd": "/", "at": 0, "pane": pane}
        entries = [e("cargo test", 101, 1), e("make", 2, 2), e("sleep 100", 130, 1), e("ls", 0, 1)]
        self.assertEqual(self.p["last_failure"](entries, 1)["command"], "cargo test")
        self.assertEqual(self.p["last_failure"](entries, 3)["command"], "make")
        self.assertIsNone(self.p["last_failure"]([e("ls", 0, 1)], 1))

    def test_rerun_and_search_skip_disguised_commands(self):
        typeable = self.p["typeable"]
        self.assertTrue(typeable("git log --oneline"))
        self.assertTrue(typeable("echo naïve 日本 ✓"))
        self.assertFalse(typeable("ls #\u202e~ fr- mr"))

    def test_history_is_capped(self):
        self.p["app"].settings["max-entries"] = 10
        for i in range(30):
            self.p["finished"](self.event(f"echo {i}"))
        history = self.p["append"].__globals__["history"]
        self.assertLessEqual(len(history), 12)
        self.assertEqual(history[-1]["command"], "echo 29")
        self.p["load"]()
        self.assertLessEqual(len(self.p["load"].__globals__["history"]), 10)

    def test_history_cap_follows_the_setting(self):
        self.p["app"].settings["max-entries"] = 10
        for i in range(15):
            self.p["finished"](self.event(f"echo {i}"))
        self.p["load"]()
        history = self.p["load"].__globals__["history"]
        self.assertEqual([e["command"] for e in history], [f"echo {i}" for i in range(5, 15)])

    def test_descriptions(self):
        d = self.p["describe"]({"command": "x", "exit": 1, "ms": 2300, "cwd": os.path.expanduser("~/src"), "at": 100}, 160)
        self.assertEqual(d, "✗ exit 1 · 2.3s · ~/src · 1m ago")


class SshHostsTests(unittest.TestCase):
    def test_hosts_come_from_config_and_includes(self):
        p = load("ssh-hosts")
        d = tempfile.mkdtemp()
        os.makedirs(f"{d}/conf.d")
        with open(f"{d}/config", "w") as f:
            f.write("Host *\n  User x\nHost web db\n  HostName 10.0.0.5\n  User deploy\n  Port 2222\n"
                    "Include conf.d/*\nHost !bad ok?\nMatch host foo\n  User nope\n")
        with open(f"{d}/conf.d/a", "w") as f:
            f.write("Host=box # comment\n  Hostname box.example.com\nInclude ../config\n")
        p["read_config"].__globals__["SSH_DIR"] = d
        hosts = {}
        p["read_config"](f"{d}/config", hosts, set())
        self.assertEqual(sorted(hosts), ["box", "db", "web"])
        self.assertEqual(p["describe"](hosts["web"]), "deploy@10.0.0.5:2222")
