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
        self.p["KEEP"] = 10
        self.p["append"].__globals__["KEEP"] = 10
        for i in range(30):
            self.p["finished"](self.event(f"echo {i}"))
        history = self.p["append"].__globals__["history"]
        self.assertLessEqual(len(history), 12)
        self.assertEqual(history[-1]["command"], "echo 29")
        self.p["load"]()
        self.assertLessEqual(len(self.p["load"].__globals__["history"]), 10)

    def test_descriptions(self):
        d = self.p["describe"]({"command": "x", "exit": 1, "ms": 2300, "cwd": os.path.expanduser("~/src"), "at": 100}, 160)
        self.assertEqual(d, "✗ exit 1 · 2.3s · ~/src · 1m ago")


class BuildStatusTests(unittest.TestCase):
    def test_build_commands_are_recognised(self):
        name = load("build-status")["build_name"]
        cases = {
            "cargo test --release": "cargo test", "make -j8": "make", "sudo make install": "make",
            "RUST_LOG=1 cargo build": "cargo build", "cargo fmt": None, "ls": None, "git commit": None,
            "./gradlew build": "./gradlew", "npm test": "npm test", "echo 'unbalanced": None, "": None,
            "cd api && cargo test": "cargo test", "make; ls": "make", "git pull && npm ci": "npm ci",
            "cargo build && cargo test -q | tee log": "cargo test",
        }
        for command, want in cases.items():
            self.assertEqual(name(command), want, command)

    def test_its_own_titles_are_recognised(self):
        p = load("build-status")
        for title, ours in [("✓ cargo test · 12s", True), ("✗ make · exit 2 · 3m 05s", True),
                            ("mine", False), ("✓ my tab", False)]:
            self.assertEqual(p["is_ours"](title), ours, title)


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


class ProjectLayoutTests(unittest.TestCase):
    def test_layouts_are_found_upwards_and_checked(self):
        p = load("project-layout")
        d = tempfile.mkdtemp()
        os.makedirs(f"{d}/sub/deeper")
        path = f"{d}/.lumen-layout.toml"
        with open(path, "w") as f:
            f.write('title="api"\n[[pane]]\nrun="nvim ."\n[[pane]]\nsplit="down"\ncwd="sub"\n'
                    'run=["tail","-f","a b"]\n[[pane]]\nof=1\n')
        self.assertEqual(p["find_layout"](f"{d}/sub/deeper"), path)
        title, panes = p["load"](path)
        self.assertEqual((title, [x["of"] for x in panes], panes[1]["cwd"]), ("api", [0, 1, 1], f"{d}/sub"))
        for bad in ['[[pane]]\n[[pane]]\nsplit="up"\n', '[[pane]]\n[[pane]]\nof=2\n', 'title="x"\n',
                    '[[pane]]\nrun=3\n', '[[pane]]\nrun="echo hi\\rrm -rf ~"\n',
                    '[[pane]]\nrun=["echo", "\\u001b[2Khidden"]\n',
                    '[[pane]]\nrun="echo safe #\\u202e ~ fr- mr"\n']:
            with open(path, "w") as f:
                f.write(bad)
            with self.assertRaises(ValueError, msg=bad):
                p["load"](path)


if __name__ == "__main__":
    unittest.main()


class PomodoroTests(unittest.TestCase):
    def setUp(self):
        self.p = load("pomodoro")
        self.t = 1000.0
        self.timer = self.p["Timer"]("classic", now=lambda: self.t)

    def test_breaks_start_by_themselves_and_focus_waits_for_you(self):
        timer = self.timer
        self.assertEqual((timer.phase, timer.badge(), timer.color()), ("focus", "🍅 25:00", "red"))
        self.t += 25 * 60
        self.assertEqual(timer.remaining(), 0)
        timer.finish()
        self.assertEqual((timer.phase, timer.ready, timer.badge()), ("break", False, "☕ 05:00"))
        self.t += 5 * 60
        timer.finish()
        self.assertEqual((timer.phase, timer.ready, timer.badge(), timer.color()), ("focus", True, "🍅 ready", "blue"))
        # Waiting doesn't use up the session.
        self.t += 600
        self.assertEqual(timer.remaining(), 25 * 60)
        timer.toggle()
        self.t += 1
        self.assertEqual(clock := self.p["clock"](timer.remaining()), "24:59", clock)

    def test_every_fourth_break_is_long(self):
        timer = self.timer
        phases = []
        for _ in range(4):
            timer.start("focus")
            timer.finish()
            phases.append(timer.phase)
        self.assertEqual(phases, ["break", "break", "break", "long"])

    def test_narrow_tabs_get_whole_minutes(self):
        timer = self.timer
        self.assertEqual(timer.short_badge(), "25m")
        self.t += 61
        self.assertEqual(timer.short_badge(), "24m")  # 23:59 left
        self.t += 23 * 60 - 1
        self.assertEqual(timer.short_badge(), "60s")
        self.t += 45
        self.assertEqual(timer.short_badge(), "15s")

    def test_pause_holds_the_time(self):
        timer = self.timer
        self.t += 60
        timer.toggle()
        self.t += 3600
        self.assertEqual((timer.badge(), timer.color()), ("‖ 24:00", "yellow"))
        timer.toggle()
        self.t += 30
        self.assertEqual(timer.remaining(), 23 * 60 + 30)

    def test_minutes_can_be_added_but_not_below_one(self):
        timer = self.timer
        timer.adjust(60)
        self.assertEqual(timer.remaining(), 26 * 60)
        for _ in range(40):
            timer.adjust(-60)
        self.assertEqual(timer.remaining(), 60)

    def test_drawn_big_when_it_fits_and_on_one_line_when_not(self):
        strip = re.compile(r"\x1b\[[0-9;]*m")
        lines = [strip.sub("", l) for l in self.timer.render(80, 12, today=2)]
        self.assertEqual(len(lines), 11)  # centred: one blank line on top
        self.assertIn("FOCUS  ·  25 min  ·  today 2 🍅", lines[1])
        self.assertTrue(all(len(l) <= 80 for l in lines))
        self.assertIn("██████", lines[3])
        small = [strip.sub("", l) for l in self.timer.render(30, 3, today=0)]
        self.assertEqual([l for l in small if l], [" FOCUS 25:00  " + "╌" * 16])

    def test_sessions_are_counted_by_day(self):
        self.assertEqual(self.p["today_count"](), 0)
        self.p["count_session"]()
        self.p["count_session"]()
        self.assertEqual(self.p["today_count"](), 2)


class PomodoroLabelTests(unittest.TestCase):
    def test_the_label_follows_minutes_added_or_taken_off(self):
        p = load("pomodoro")
        t = [0.0]
        timer = p["Timer"]("classic", now=lambda: t[0])
        timer.adjust(-6 * 60)
        strip = re.compile(r"\x1b\[[0-9;]*m")
        head = strip.sub("", timer.render(80, 12, today=0)[1])
        self.assertIn("19 min", head)
