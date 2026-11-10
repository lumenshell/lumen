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

sys.path.insert(0, os.path.join(PLUGINS, "project-layout"))

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


class LayoutFileTests(unittest.TestCase):
    def setUp(self):
        import layoutfile
        self.lf = layoutfile
        self.d = tempfile.mkdtemp()
        self.path = os.path.join(self.d, ".lumen-layout.toml")

    def write(self, text):
        with open(self.path, "w", encoding="utf-8", newline="") as f:
            f.write(text)

    def read(self):
        with open(self.path, encoding="utf-8", newline="") as f:
            return f.read()

    def test_old_files_load_unchanged(self):
        self.write('title = "api"\n\n[[pane]]\nrun = "nvim ."\n\n[[pane]]\nsplit = "right"\n'
                   'run = "cargo watch -x test"\n\n[[pane]]\nsplit = "down"\nof = 1\n'
                   'cwd = "frontend"\nrun = "npm run dev"\n')
        lay = self.lf.load(self.path)
        self.assertEqual(lay.title, "api")
        self.assertEqual(lay.panes, [
            {"split": "right", "of": 0, "cwd": self.d, "run": "nvim ."},
            {"split": "right", "of": 1, "cwd": self.d, "run": "cargo watch -x test"},
            {"split": "down", "of": 1, "cwd": os.path.join(self.d, "frontend"), "run": "npm run dev"},
        ])
        self.assertEqual(lay.tasks, [])
        self.assertIsNone(lay.dev)

    def test_tasks_and_dev_load(self):
        os.makedirs(os.path.join(self.d, "web"))
        self.write('[[task]]\nname = "db"\nrun = "docker compose up db"\nstop = "docker compose stop db"\n\n'
                   '[[task]]\nname = "js"\nrun = ["npm", "run", "dev"]\ncwd = "web"\n\n'
                   '[dev]\ntasks = ["db", "js", "server"]\n')
        lay = self.lf.load(self.path, known=frozenset({"server"}))
        T = self.lf.Task
        self.assertEqual(lay.tasks, [
            T("db", "docker compose up db", self.d, "docker compose stop db", "saved"),
            T("js", ["npm", "run", "dev"], os.path.join(self.d, "web"), None, "saved"),
        ])
        self.assertEqual(lay.dev, ["db", "js", "server"])
        self.assertEqual(lay.panes, [])

    def test_bad_entries_are_refused(self):
        cases = [
            ('[[task]]\nname = "a"\nrun = "echo \\u001b[2Khi"\n', "run"),
            ('[[task]]\nname = "a\\u202eb"\nrun = "x"\n', "name"),
            ('[[task]]\nname = "a"\nrun = "x"\n[[task]]\nname = "a"\nrun = "y"\n', "a"),
            ('[[task]]\nname = "a"\nrun = "x"\n[dev]\ntasks = ["nope"]\n', "nope"),
            ('[[task]]\nname = "a"\nrun = "x"\ncwd = "missing"\n', "cwd"),
        ]
        for text, word in cases:
            self.write(text)
            with self.assertRaises(ValueError, msg=text) as cm:
                self.lf.load(self.path, known=frozenset({"other"}))
            self.assertIn(word, str(cm.exception))

    def test_writing_keeps_everything_else(self):
        self.write(
            "# my project\n"
            'title = "api"\n'
            "\n"
            "# editor\n"
            "[[pane]]\n"
            'run = "nvim ."\n'
            "\n"
            "[[task]]   # hand-written\n"
            'name = "old"\n'
            'run = "true"\n'
            "\n"
            "# watcher\n"
            "[[pane]]\n"
            'split = "down"\n'
            "run = 'cargo watch'  # keep\n"
            "\n"
            "[dev]\n"
            'tasks = ["old"]\n'
            "\n"
            "\n")
        T = self.lf.Task
        os.makedirs(os.path.join(self.d, "web"))
        self.lf.write_owned(self.path, [
            T("db", "docker compose up db", self.d, None, "saved"),
            T("js", ["npm", "run", "dev"], os.path.join(self.d, "web"), "pkill node", "npm"),
        ], ["db", "js"])
        self.assertEqual(self.read(),
            "# my project\n"
            'title = "api"\n'
            "\n"
            "# editor\n"
            "[[pane]]\n"
            'run = "nvim ."\n'
            "\n"
            "# watcher\n"
            "[[pane]]\n"
            'split = "down"\n'
            "run = 'cargo watch'  # keep\n"
            "\n"
            "[[task]]\n"
            'name = "db"\n'
            'run = "docker compose up db"\n'
            "\n"
            "[[task]]\n"
            'name = "js"\n'
            'run = ["npm", "run", "dev"]\n'
            'cwd = "web"\n'
            'stop = "pkill node"\n'
            "\n"
            "[dev]\n"
            'tasks = ["db", "js"]\n')

    def test_sub_tables_are_not_owned(self):
        self.write('[[pane]]\nrun = "a"\n\n[dev.extra]\nx = 1\n\n[ dev ]\ntasks = []\n\n[[ task ]]\nname = "z"\nrun = "z"\n')
        self.lf.write_owned(self.path, [], None)
        self.assertEqual(self.read(), '[[pane]]\nrun = "a"\n\n[dev.extra]\nx = 1\n')

    def test_awkward_commands_round_trip(self):
        T = self.lf.Task
        tasks = [
            T("one", 'echo "a\\b" $HOME; printf \'%s\' caf\u00e9', self.d, None, "saved"),
            T("two", ["say", "it's \"x\""], self.d, 'kill "$PID"', "saved"),
        ]
        self.lf.write_owned(self.path, tasks, ["one", "two"])
        lay = self.lf.load(self.path)
        self.assertEqual(lay.tasks, tasks)
        self.assertEqual(lay.dev, ["one", "two"])

    def write_and_read(self, text, tasks=(), dev=None):
        self.write(text)
        self.lf.write_owned(self.path, list(tasks), dev)
        return self.read()

    def test_multiline_string_with_owned_looking_lines_is_untouched(self):
        text = '[[pane]]\nrun = """\nline\n[dev]\n[[task]]\n"""\n'
        self.assertEqual(self.write_and_read(text), text)
        text2 = "[[pane]]\nrun = \'\'\'\n[dev]\n\'\'\'\n\n[dev]\ntasks = []\n"
        self.assertEqual(self.write_and_read(text2), "[[pane]]\nrun = \'\'\'\n[dev]\n\'\'\'\n")

    def test_unusual_table_headers_end_an_owned_block(self):
        got = self.write_and_read('[[task]]\nname = "a"\nrun = "x"\n\n[\'my tbl\']\nk = 1\n["caf\u00e9"]\nz = 1\n')
        self.assertEqual(got, "['my tbl']\nk = 1\n[\"caf\u00e9\"]\nz = 1\n")

    def test_trailing_comments_are_kept(self):
        T = self.lf.Task
        got = self.write_and_read('[dev]\ntasks = []\n\n# note to self\n', [T("a", "x", self.d)], ["a"])
        self.assertEqual(got, '# note to self\n\n[[task]]\nname = "a"\nrun = "x"\n\n[dev]\ntasks = ["a"]\n')

    def test_array_element_line_does_not_end_a_block(self):
        got = self.write_and_read('[[pane]]\nrun = "a"\n\n[dev]\ntasks = [\n  "a",\n  ["b"]\n]\n')
        self.assertEqual(got, '[[pane]]\nrun = "a"\n')

    def test_unsafe_writes_are_refused(self):
        T = self.lf.Task
        for text in ['[task.x]\ny = 2\n', 'title = \n']:
            self.write(text)
            with self.assertRaises(ValueError) as cm:
                self.lf.write_owned(self.path, [T("a", "x", self.d)], None)
            self.assertIn("Couldn't update", str(cm.exception))
            self.assertEqual(self.read(), text)

    def test_non_table_task_entry_is_refused(self):
        self.write('task = ["x"]\n')
        with self.assertRaises(ValueError) as cm:
            self.lf.load(self.path)
        self.assertIn("task", str(cm.exception))

    def test_crlf_and_symlink(self):
        T = self.lf.Task
        got = self.write_and_read('[[pane]]\r\nrun = "a"\r\n', [T("a", "x", self.d)], ["a"])
        self.assertEqual(got, '[[pane]]\r\nrun = "a"\r\n\r\n[[task]]\r\nname = "a"\r\nrun = "x"\r\n\r\n[dev]\r\ntasks = ["a"]\r\n')
        real = self.path + ".real"
        os.rename(self.path, real)
        os.symlink(real, self.path)
        self.lf.write_owned(self.path, [], None)
        self.assertTrue(os.path.islink(self.path))
        with open(real, newline="") as f:
            self.assertEqual(f.read(), '[[pane]]\r\nrun = "a"\r\n')

    def test_first_write_creates_the_file(self):
        self.assertFalse(os.path.exists(self.path))
        T = self.lf.Task
        self.lf.write_owned(self.path, [T("a", "x", self.d, None, "saved")], ["a"])
        self.assertEqual(self.read(), '[[task]]\nname = "a"\nrun = "x"\n\n[dev]\ntasks = ["a"]\n')


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


class DiscoverTests(unittest.TestCase):
    def setUp(self):
        import discover
        self.discover = discover
        self.d = tempfile.mkdtemp()

    def put(self, rel, text=""):
        path = os.path.join(self.d, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)

    def found(self):
        return [(t.name, t.run) for t in self.discover.discover(self.d)]

    def test_finds_each_source(self):
        self.put("compose.yaml", "name: x\nservices:\n  web:\n    image: nginx\n  db:\n    image: pg\n"
                                 "volumes:\n  data:\n")
        self.put("package.json", json.dumps({"scripts": {"dev": "vite", "build:css": "sass"}}))
        self.put("composer.json", json.dumps({"scripts": {"test": "phpunit"}}))
        self.put("artisan", "#!/usr/bin/env php\n")
        self.put("Makefile", ".PHONY: all\nall:\n\techo\nbuild: all\n\tcc\nCC:=gcc\n")
        self.put("justfile", "set shell := ['sh']\n@lint:\n  echo\nfmt arg:\n  echo\n")
        self.put("Procfile", "web: bundle exec puma -p 3000\nworker: ruby w.rb\n")
        self.put("Cargo.toml", "[package]\n")
        tasks = self.discover.discover(self.d)
        self.assertEqual([(t.name, t.run) for t in tasks], [
            ("compose:web", "docker compose up web"), ("db", "docker compose up db"),
            ("dev", "npm run dev"), ("build:css", "npm run build:css"),
            ("composer:test", "composer run test"),
            ("serve", "php artisan serve"),
            ("all", "make all"), ("build", "make build"),
            ("lint", "just lint"), ("fmt", "just fmt"),
            ("procfile:web", "bundle exec puma -p 3000"), ("worker", "ruby w.rb"),
            ("run", "cargo run"), ("cargo:test", "cargo test"),
        ])
        sources = [t.source for t in tasks]
        self.assertEqual(sources[:2], ["compose", "compose"])
        self.assertEqual(tasks[0].stop, "docker compose stop web")
        self.assertEqual(set(sources), {"compose", "npm", "composer", "artisan", "make",
                                        "just", "procfile", "cargo"})
        self.assertTrue(all(os.path.isabs(t.cwd) for t in tasks))

    def test_package_manager_follows_the_lockfile(self):
        self.put("package.json", json.dumps({"scripts": {"dev": "x"}}))
        self.assertEqual(self.found(), [("dev", "npm run dev")])
        for lock, pm in (("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"),
                         ("bun.lockb", "bun"), ("bun.lock", "bun")):
            os.makedirs(os.path.join(self.d, "x"), exist_ok=True)
            for other in ("pnpm-lock.yaml", "yarn.lock", "bun.lockb", "bun.lock"):
                if os.path.exists(os.path.join(self.d, other)):
                    os.remove(os.path.join(self.d, other))
            self.put(lock)
            self.assertEqual(self.found(), [("dev", pm + " run dev")], lock)
            self.assertEqual(self.discover.discover(self.d)[0].source, "npm")

    def test_subfolder_tasks_are_prefixed_and_run_there(self):
        self.put("frontend/package.json", json.dumps({"scripts": {"dev": "x"}}))
        self.put("api/Makefile", "serve:\n\tgo run .\n")
        self.put("node_modules/p/package.json", json.dumps({"scripts": {"bad": "x"}}))
        self.put(".hidden/Makefile", "bad:\n")
        self.put("vendor/Makefile", "bad:\n")
        self.put("target/Makefile", "bad:\n")
        self.put("frontend/deep/Makefile", "bad:\n")
        tasks = self.discover.discover(self.d)
        self.assertEqual([(t.name, t.run) for t in tasks],
                         [("api:serve", "make serve"), ("frontend:dev", "npm run dev")])
        self.assertEqual(tasks[1].cwd, os.path.abspath(os.path.join(self.d, "frontend")))
        self.assertEqual(tasks[0].cwd, os.path.abspath(os.path.join(self.d, "api")))

    def test_name_clashes_get_the_source_prefix(self):
        self.put("package.json", json.dumps({"scripts": {"test": "x", "dev": "y"}}))
        self.put("Makefile", "test:\n\tx\n")
        self.put("Cargo.toml")
        self.assertEqual(self.found(), [("npm:test", "npm run test"), ("dev", "npm run dev"),
                                        ("make:test", "make test"), ("run", "cargo run"),
                                        ("cargo:test", "cargo test")])

    def test_clashes_keep_the_subfolder_prefix(self):
        self.put("a/package.json", json.dumps({"scripts": {"test": "x"}}))
        self.put("a/Makefile", "test:\n")
        self.put("b/Makefile", "test:\n")
        self.assertEqual(self.found(), [("a:npm:test", "npm run test"), ("a:make:test", "make test"),
                                        ("b:test", "make test")])

    def test_unsafe_names_are_quoted_or_skipped(self):
        self.put("package.json", json.dumps({"scripts": {"a b": "x", "evil‮": "y", "ok": "z"}}))
        self.put("Procfile", "web: echo hi\x1b[31m\n")
        self.assertEqual(self.found(), [("a b", "npm run 'a b'"), ("ok", "npm run ok")])

    def test_broken_files_are_skipped(self):
        self.put("package.json", "{not json")
        self.put("composer.json", json.dumps({"scripts": "nope"}))
        self.put("Makefile", "build:\n")
        with open(os.path.join(self.d, "Procfile"), "wb") as f:
            f.write(b"web: ok\nbad: run \xff\n")
        with open(os.path.join(self.d, "justfile"), "wb") as f:
            f.write(b"\xff\xfe\x00bad")
        self.assertEqual(self.found(), [("build", "make build"), ("web", "ok")])

    def test_empty_folder_finds_nothing(self):
        self.assertEqual(self.found(), [])
        self.assertEqual(self.discover.discover(os.path.join(self.d, "missing")), [])

    def test_prefixed_names_stay_unique(self):
        self.put("package.json", json.dumps({"scripts": {"test": "x", "make:test": "y"}}))
        self.put("Makefile", "test:\n")
        names = [t.name for t in self.discover.discover(self.d)]
        self.assertEqual(len(names), 3)
        self.assertEqual(len(set(names)), 3, names)

    def test_names_starting_with_a_dash_are_skipped(self):
        self.put("package.json", json.dumps({"scripts": {"-h": "x", "--script-shell=/tmp/e": "y", "ok": "z"}}))
        self.put("composer.json", json.dumps({"scripts": {"--no-cache": "x"}}))
        self.put("Makefile", "ok2:\n")
        self.put("justfile", "-x:\n  echo\nfine:\n  echo\n")
        self.assertEqual(self.found(), [("ok", "npm run ok"), ("ok2", "make ok2"), ("fine", "just fine")])

    def test_just_recipes(self):
        self.put("justfile", (
            "set shell := ['sh']\nexport A := '1'\nversion := 'a:b'\n"
            "mod foo 'a:b'\nimport 'x.just'\nalias b := build\n"
            "serve port=\"80:80\" host='h': build\n  echo\n"
            "build:\n  echo\n_hidden:\n  echo\n"
            "[private]\n# doc\nsecret:\n  echo\n"
            "[no-cd, private]\nsecret2:\n  echo\n"
            "[no-cd]\nvisible:\n  echo\n@quiet:\n  echo\n"))
        self.assertEqual(self.found(), [("serve", "just serve"), ("build", "just build"),
                                        ("visible", "just visible"), ("quiet", "just quiet")])

    def test_bom_and_stray_bytes(self):
        with open(os.path.join(self.d, "package.json"), "wb") as f:
            f.write(b"\xef\xbb\xbf" + json.dumps({"scripts": {"dev": "x"}}).encode())
        with open(os.path.join(self.d, "Makefile"), "wb") as f:
            f.write(b"good:\n\tx\nb\xe9d:\n")
        self.assertEqual(self.found(), [("dev", "npm run dev"), ("good", "make good")])

    def assertUnique(self):
        names = [t.name for t in self.discover.discover(self.d)]
        self.assertGreater(len(names), 3)
        self.assertEqual(len(set(names)), len(names), names)

    def test_names_unique_when_everything_is_already_prefixed(self):
        self.put("package.json", json.dumps({"scripts": {"make:x": "a"}}))
        self.put("composer.json", json.dumps({"scripts": {"make:x": "a"}}))
        self.put("npm/Makefile", "x:\n")
        self.put("npm/justfile", "x:\n  echo\n")
        self.assertUnique()

    def test_names_unique_with_clashing_folder_names(self):
        self.put("a/package.json", json.dumps({"scripts": {"npm:y": "a", "y": "b"}}))
        self.put("a/Makefile", "y:\n")
        self.put("a:npm/package.json", json.dumps({"scripts": {"y": "a"}}))
        self.put("a:npm/Makefile", "y:\n")
        self.assertUnique()


ANSI = re.compile(r"\x1b\[[0-9;]*m")


def plain(lines):
    return [ANSI.sub("", ln) for ln in lines]


class ChecklistTests(unittest.TestCase):
    def setUp(self):
        import checklist
        import layoutfile
        self.cl = checklist
        self.lf = layoutfile

    def tasks(self, n=3):
        return [self.lf.Task("t%d" % i, "echo %d" % i, ".") for i in range(n)]

    def test_moves_ticks_and_answers(self):
        c = self.cl.Checklist(self.tasks(), set())
        self.assertEqual(c.chosen(), [])
        self.assertIsNone(c.key("x"))
        self.assertIsNone(c.key("enter"))  # nothing ticked
        self.assertIsNone(c.key("space"))
        self.assertIsNone(c.key("down"))
        self.assertIsNone(c.key("j"))
        self.assertIsNone(c.key("down"))  # clamps at the end
        self.assertIsNone(c.key("space"))
        self.assertEqual(c.chosen(), ["t0", "t2"])
        c.key("k")
        c.key("up")
        c.key("up")  # clamps at the top
        c.key("space")  # untick t0
        self.assertEqual(c.chosen(), ["t2"])
        self.assertEqual(c.key("a"), "add")
        self.assertEqual(c.key("enter"), "start")
        self.assertEqual(c.key("escape"), "cancel")
        self.assertEqual(c.key("q"), "cancel")

    def test_initial_ticks_and_order(self):
        c = self.cl.Checklist(self.tasks(), {"t2", "t0", "gone"})
        self.assertEqual(c.chosen(), ["t0", "t2"])

    def test_added_tasks_are_ticked(self):
        c = self.cl.Checklist(self.tasks(2), set())
        c.add(self.lf.Task("new", ["npm", "run", "x y"], "."))
        self.assertEqual(c.chosen(), ["new"])
        c.key("space")  # cursor is on the new task
        self.assertEqual(c.chosen(), [])
        self.assertIn("[ ] new", "\n".join(plain(c.render(40, 10))))

    def test_renders_within_the_pane(self):
        ts = [self.lf.Task("task-%02d" % i, "run something long %d" % i, ".") for i in range(20)]
        c = self.cl.Checklist(ts, {"task-00"})
        for _ in range(12):
            c.key("down")
        for cols, rows in ((40, 8), (12, 5), (80, 30), (10, 3)):
            lines = plain(c.render(cols, rows))
            self.assertLessEqual(len(lines), rows)
            for ln in lines:
                self.assertLessEqual(self.cl.width(ln), cols, ln)
        text = "\n".join(plain(c.render(40, 8)))
        self.assertIn("task-12", text)
        self.assertNotIn("task-00", text)
        self.assertIn("Space tick · a add task · Enter start · Esc cancel", "\n".join(plain(c.render(80, 30))))
        self.assertIn("[x] task-00", "\n".join(plain(c.render(80, 30))))

    def test_wide_characters_fit(self):
        ts = [self.lf.Task("构建项目", "make 日本語のコマンド 😀", "."), self.lf.Task("éé", "x", ".")]
        c = self.cl.Checklist(ts, set())
        for cols in range(10, 40):
            for ln in plain(c.render(cols, 10)):
                self.assertLessEqual(self.cl.width(ln), cols, ln)

    def test_list_run_is_shown_as_shell_words(self):
        c = self.cl.Checklist([self.lf.Task("x", ["npm", "run", "a b"], ".")], set())
        self.assertIn("npm run 'a b'", "\n".join(plain(c.render(80, 10))))

    def test_empty_list_explains_itself(self):
        c = self.cl.Checklist([], set())
        text = "\n".join(plain(c.render(80, 10)))
        self.assertIn("Nothing to run found here. Press a to add a task.", text)
        for k in ("space", "down", "up", "enter", "j"):
            self.assertIsNone(c.key(k))
        self.assertEqual(c.key("a"), "add")
        self.assertEqual(c.key("escape"), "cancel")
        self.assertEqual(c.chosen(), [])

    def test_tiny_sizes_do_not_crash(self):
        c = self.cl.Checklist(self.tasks(), {"t0"})
        for cols in (0, 1, 5, 9, 10):
            for rows in (0, 1, 2, 3):
                lines = plain(c.render(cols, rows))
                self.assertLessEqual(len(lines), rows)
                for ln in lines:
                    self.assertLessEqual(self.cl.width(ln), cols)
        self.cl.Checklist([], set()).render(1, 1)


class GridTests(unittest.TestCase):
    def setUp(self):
        import grid
        self.grid = grid

    def test_plans(self):
        plan = self.grid.grid_plan
        self.assertEqual(self.grid.MAX_PER_TAB, 6)
        self.assertEqual(plan(1), [])
        self.assertEqual(plan(2), [(1, "right", 0, 0.5)])
        self.assertEqual(plan(3), [(1, "right", 0, 0.5), (2, "down", 0, 0.5)])
        self.assertEqual(plan(4), [(1, "right", 0, 0.5), (2, "down", 0, 0.5), (3, "down", 1, 0.5)])
        self.assertEqual(plan(5), [(1, "right", 0, 2 / 3), (2, "right", 1, 0.5), (3, "down", 0, 0.5),
                                   (4, "down", 1, 0.5)])
        self.assertEqual(plan(6), [(1, "right", 0, 2 / 3), (2, "right", 1, 0.5), (3, "down", 0, 0.5),
                                   (4, "down", 1, 0.5), (5, "down", 2, 0.5)])

    def test_every_split_is_of_an_earlier_pane(self):
        for n in range(1, 7):
            made = {0}
            for task, direction, of, size in self.grid.grid_plan(n):
                self.assertIn(of, made)
                self.assertTrue(0 < size < 1)
                made.add(task)
            self.assertEqual(made, set(range(n)))


class RunnerTests(unittest.TestCase):
    def setUp(self):
        import layoutfile
        import runner
        self.p = vars(runner)
        self.lf = layoutfile
        self.r = runner.Runner()

    def task(self, name, stop=None):
        return self.lf.Task(name, "run " + name, "/tmp", stop)

    def open_tab(self, root, panes):
        for pid in panes:
            self.r.track(root, panes[0], pid, self.task("t%d" % pid))

    def test_failures_mark_red_and_count(self):
        self.open_tab("/a", [1, 2, 3])
        self.open_tab("/a", [4])  # its second dev tab
        self.assertEqual(self.r.badge(1), ("3 running", None))
        self.assertEqual(self.r.failed(1, 1), ("red", "1 failed"))
        self.assertEqual(self.r.failed(2, 127), ("red", "2 failed"))
        self.assertEqual(self.r.badge(3), ("2 failed", "red"))
        self.assertEqual(self.r.failed(4, 2), ("red", "1 failed"))  # counted per tab
        self.r.rerun(1)
        self.r.rerun(2)
        self.assertEqual(self.r.badge(1), ("3 running", None))

    def test_success_marks_green(self):
        self.open_tab("/a", [1, 2])
        self.assertEqual(self.r.failed(1, 0), ("green", None))
        self.assertEqual(self.r.badge(2), ("1 running", None))

    def test_untracked_panes_are_ignored(self):
        self.open_tab("/a", [1])
        self.assertIsNone(self.r.failed(9, 1))
        self.assertIsNone(self.r.badge(9))
        self.assertIsNone(self.r.failed(1, None))  # no exit code: no mark rather than a wrong one
        self.r.restarting.add(1)
        self.assertIsNone(self.r.failed(1, 130))  # stopped on purpose for a restart
        self.assertEqual(self.r.badge(1), ("done", None))

    def test_restart_without_shell_integration_just_waits(self):
        self.assertEqual(self.r.restart_steps(False), [("ctrl-c", 1.0)])
        self.assertEqual(self.r.restart_steps(True), [("ctrl-c", 5.0), ("ctrl-c", 3.0)])
        self.open_tab("/a", [1])
        self.assertFalse(self.r.is_integrated(1))
        self.assertTrue(self.r.is_running(1))
        self.r.started(1)
        self.assertTrue(self.r.is_integrated(1))
        self.r.failed(1, 0)
        self.assertFalse(self.r.is_running(1))  # already stopped: rerun without Ctrl+C

    def test_closing_forgets_and_drops_the_project(self):
        self.open_tab("/a", [1, 2])
        self.r.failed(1, 1)
        self.assertEqual(self.r.forget(1).name, "t1")
        self.assertEqual(self.r.badge(2), ("1 running", None))
        self.assertIsNone(self.r.forget(7))
        self.assertEqual(self.r.runs, {"/a": [2]})
        self.r.forget(2)
        self.assertEqual((self.r.runs, self.r.tasks_by_pane), ({}, {}))

    def test_open_tab_needs_live_panes(self):
        self.open_tab("/a", [1, 2])
        self.assertEqual(self.r.open_tab("/a", {2, 5}), 2)
        self.assertIsNone(self.r.open_tab("/a", {5}))
        self.assertNotIn("/a", self.r.runs)
        self.assertEqual(self.r.tasks_by_pane, {})

    def test_stop_runs_as_typed_or_as_argv(self):
        argv = self.p["stop_argv"]
        self.assertEqual(argv("docker compose stop 'web'"), ["/bin/sh", "-c", "docker compose stop 'web'"])
        self.assertEqual(argv(["make", "a b"]), ["make", "a b"])

    def test_approval_remembers_only_the_same_commands(self):
        d = tempfile.mkdtemp()
        path = os.path.join(d, ".lumen-layout.toml")
        panes = [{"cwd": d, "run": "nvim ."}]
        tasks = [self.lf.Task("web", "npm run dev", d, "docker compose stop web")]
        key = self.p["approval_key"]
        data = tempfile.mkdtemp()
        approved = lambda path, k: self.p["approved"](data, path, k)  # noqa: E731
        remember = lambda path, k: self.p["remember"](data, path, k)  # noqa: E731
        k = key(path, panes, tasks)
        self.assertFalse(approved(path, k))
        remember(path, k)
        self.assertTrue(approved(path, k))
        for changed in ([{"cwd": d, "run": "nvim"}], tasks), (panes, [self.lf.Task("web", "npm run dev", "/", None)]), \
                (panes, [self.lf.Task("web", "npm run dev", d, "rm -rf ~")]):
            self.assertFalse(approved(path, key(path, *changed)), changed)
        self.assertNotEqual(k, key(path + "x", panes, tasks))
        store = os.path.join(data, "approved.json")
        with open(store, "w") as f:
            f.write("{not json")
        self.assertFalse(approved(path, k))
        remember(path, k)  # a corrupt file is replaced
        self.assertTrue(approved(path, k))


class OpenProjectFixTests(unittest.TestCase):
    def setUp(self):
        import layoutfile
        import runner
        self.p = vars(runner)
        self.lf = layoutfile

    def test_saved_tasks_from_the_file_need_a_confirm(self):
        needs = self.p["needs_confirm"]
        T = self.lf.Task
        found = T("build", "make build", "/p", None, "make")
        added = T("greet", "echo hi", "/p", None, "saved")
        from_file = T("web", "npm run dev", "/p", "curl x | sh", "saved")
        self.assertFalse(needs([found], set()))
        self.assertFalse(needs([found, added], {"greet"}))
        self.assertTrue(needs([found, from_file], {"greet"}))
        self.assertTrue(needs([added], set()))  # a saved task not added just now

    def test_open_guard_lets_one_press_through(self):
        g = self.p["OpenGuard"]()
        token = g.enter()
        self.assertTrue(token)
        self.assertFalse(g.enter())  # a second press while busy
        g.leave(token + 1)  # a stale leave doesn't free it
        self.assertFalse(g.enter())
        g.leave(token)
        g.leave(token)  # twice is harmless
        self.assertTrue(g.enter())

    def test_rerun_goes_back_to_the_task_folder(self):
        text = self.p["rerun_text"]
        d = tempfile.mkdtemp()
        self.assertEqual(text("npm run dev", d, d), "npm run dev")
        self.assertEqual(text("npm run dev", d, "/"), f"cd {lumen_plugin.quote(d)} && npm run dev")
        self.assertEqual(text(["make", "a b"], "/my dir", None), "cd '/my dir' && make 'a b'")
        self.assertEqual(text("ls", None, "/"), "ls")  # a one-off: no folder of its own

    def test_remember_survives_an_unwritable_data_dir(self):
        folder = os.path.join(tempfile.mkdtemp(), "file")
        open(folder, "w").close()  # a file where the folder should be
        self.p["remember"](folder, "/x", "k")  # no exception
        self.p["remember"](None, "/x", "k")  # no data folder at all
        self.assertFalse(self.p["approved"](folder, "/x", "k"))
        self.assertFalse(self.p["approved"](None, "/x", "k"))


class AdHocTaskTests(unittest.TestCase):
    def setUp(self):
        import layoutfile
        import runner
        self.lf = layoutfile
        self.r = runner
        self.root = tempfile.mkdtemp()
        self.path = os.path.join(self.root, ".lumen-layout.toml")

    def read(self):
        with open(self.path, encoding="utf-8") as f:
            return f.read()

    def test_add_task_refuses_duplicates_and_control_characters(self):
        T = self.lf.Task
        tasks = [T("web", "npm run dev", self.root, None, "saved"), T("test", "make test", self.root, None, "make")]
        for name, run in [("web", "echo"), ("test", "echo"), ("ok", "echo hi\rrm -rf ~"), ("o‮k", "echo"),
                          ("ok", "echo \x1b[2K"), ("o\nk", "echo")]:
            with self.assertRaises(ValueError, msg=(name, run)) as e:
                self.r.add_task(None, self.root, tasks, name, run)
            self.assertTrue(str(e.exception))  # a message for you
        self.assertFalse(os.path.exists(self.path))  # nothing written

    def test_add_task_creates_the_file_in_the_root(self):
        saved = self.r.add_task(None, self.root, [], "hi", 'echo "hi" $HOME; ünï')
        self.assertEqual([(t.name, t.run, t.cwd) for t in saved], [("hi", 'echo "hi" $HOME; ünï', self.root)])
        self.assertNotIn("[dev]", self.read())
        layout = self.lf.load(self.path)
        self.assertEqual([(t.name, t.run, t.cwd) for t in layout.tasks], [("hi", 'echo "hi" $HOME; ünï', self.root)])
        self.assertIsNone(layout.dev)

    def test_add_task_keeps_the_rest_of_the_file_and_dev(self):
        os.makedirs(os.path.join(self.root, "sub"))
        with open(self.path, "w", encoding="utf-8") as f:
            f.write('# mine\ntitle = "api"\n\n[[pane]]\nrun = "nvim ."\n\n[[task]]\nname = "web"\nrun = "npm run dev"\n\n'
                    '[dev]\ntasks = ["web", "build"]\n')
        found = [self.lf.Task("build", "make build", self.root, None, "make")]
        layout = self.lf.load(self.path, known=frozenset({"build"}))
        tasks = self.r.all_tasks(layout, found)
        self.r.add_task(self.path, self.root, tasks, "nap", "sleep 1000", cwd=os.path.join(self.root, "sub"))
        text = self.read()
        self.assertTrue(text.startswith('# mine\ntitle = "api"\n\n[[pane]]\nrun = "nvim ."\n'))
        after = self.lf.load(self.path, known=frozenset({"build"}))
        self.assertEqual([(t.name, t.cwd) for t in after.tasks],
                         [("web", self.root), ("nap", os.path.join(self.root, "sub"))])
        self.assertEqual(after.dev, ["web", "build"])  # unchanged; the discovered task isn't written
        with self.assertRaises(ValueError):
            self.r.add_task(self.path, self.root, self.r.all_tasks(after, found), "build", "x")

    def test_all_tasks_lets_saved_ones_shadow_discovered(self):
        T = self.lf.Task
        layout = self.lf.Layout(tasks=[T("test", "pytest", "/p")])
        found = [T("test", "make test", "/p", None, "make"), T("run", "cargo run", "/p", None, "cargo")]
        self.assertEqual([t.run for t in self.r.all_tasks(layout, found)], ["pytest", "cargo run"])
        self.assertEqual([t.run for t in self.r.all_tasks(None, found)], ["make test", "cargo run"])

    def test_labels_are_cut_to_32_characters(self):
        self.assertEqual(self.r.label("npm run dev"), "npm run dev")
        self.assertEqual(self.r.label("x" * 32), "x" * 32)
        cut = self.r.label("é" * 40)
        self.assertEqual((len(cut), cut[-1]), (32, "…"))

    def test_a_saved_one_off_becomes_a_task(self):
        run = self.r.Runner()
        run.one_offs[5] = ("sleep 1000", "/p")
        task = self.lf.Task("nap", "sleep 1000", "/p")
        run.track_single(5, task)
        self.assertEqual((run.one_offs, run.tasks_by_pane), ({}, {5: task}))
        self.assertEqual(run.failed(5, 1), ("red", None))  # marked, with no dev tab to badge
        self.assertEqual(run.runs, {})  # Open Project won't take it for the dev tab
        self.assertIs(run.forget(5), task)


class ChecklistRereadTests(unittest.TestCase):
    """A checklist snapshots the file's tasks when it opens; what's saved to the
    file meanwhile (Add Task, Save as Task, a hand edit) must survive its save."""

    def setUp(self):
        import layoutfile
        import runner
        self.lf, self.r = layoutfile, runner
        self.root = tempfile.mkdtemp()
        self.path = os.path.join(self.root, ".lumen-layout.toml")

    def test_a_task_added_while_the_checklist_is_open_survives(self):
        T = self.lf.Task
        with open(self.path, "w") as f:
            f.write('[[task]]\nname = "web"\nrun = "npm run dev"\n')
        layout = self.lf.load(self.path)
        snapshot = list(layout.tasks)  # the checklist opens
        # Meanwhile: Add Task in another pane, and a hand edit of web.
        self.r.add_task(self.path, self.root, self.r.all_tasks(layout, []), "nap", "sleep 1000")
        with open(self.path) as f:
            text = f.read().replace("npm run dev", "npm start")
        with open(self.path, "w") as f:
            f.write(text)
        # The checklist added greet, and starts with web and greet.
        keep = snapshot + [T("greet", "echo hi", self.root)]
        tasks = self.r.merge_saved(keep, {"greet"}, self.r.saved_now(self.path, frozenset()))
        self.lf.write_owned(self.path, tasks, ["web", "greet"])
        after = self.lf.load(self.path)
        self.assertEqual([(t.name, t.run) for t in after.tasks],
                         [("web", "npm start"), ("greet", "echo hi"), ("nap", "sleep 1000")])
        self.assertEqual(after.dev, ["web", "greet"])

    def test_what_the_checklist_added_wins_a_name_clash(self):
        T = self.lf.Task
        now = [T("greet", "echo from file", "/p"), T("build", "make all", "/p")]
        keep = [T("greet", "echo hi", "/p"), T("build", "make build", "/p", None, "make")]
        merged = self.r.merge_saved(keep, {"greet"}, now)
        # greet was added in the checklist; build is a discovered task the file now saves.
        self.assertEqual([(t.name, t.run) for t in merged], [("greet", "echo hi"), ("build", "make all")])

    def test_saved_now_reads_the_file_again(self):
        self.assertEqual(self.r.saved_now(self.path, frozenset()), [])  # no file yet
        with open(self.path, "w") as f:
            f.write('[[task]]\nname = "x"\nrun = "echo \\u001b[2K"\n')
        with self.assertRaises(ValueError):
            self.r.saved_now(self.path, frozenset())
        with open(self.path, "w") as f:
            f.write("[[task]\n")  # not TOML
        with self.assertRaises(ValueError):
            self.r.saved_now(self.path, frozenset())


class FakePane:
    """Enough of a pane for the project-layout commands, without Lumen."""

    def __init__(self, id, cwd):
        self.id, self._cwd, self.splits = id, cwd, []

    async def cwd(self):
        return self._cwd

    async def split(self, direction, **kw):
        self.splits.append(kw)
        new = FakePane(self.id + 100, kw.get("cwd"))
        return new

    async def set_pane_style(self, **kw):
        pass


class ReviewFixTests(unittest.TestCase):
    """The final review's findings."""

    def setUp(self):
        import layoutfile
        import runner
        self.lf, self.r = layoutfile, runner
        self.d = tempfile.mkdtemp()

    def write(self, path, text):
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)

    def read(self, path):
        with open(path, encoding="utf-8") as f:
            return f.read()

    # Important 1
    def test_a_symlinked_project_keeps_its_task_cwds(self):
        real = os.path.join(self.d, "real", "proj")
        os.makedirs(os.path.join(real, "frontend"))
        link = os.path.join(self.d, "link")
        os.symlink(real, link)
        path = os.path.join(link, ".lumen-layout.toml")
        self.write(path, '[[task]]\nname = "web"\nrun = "npm run dev"\ncwd = "frontend"\n')
        layout = self.lf.load(path)
        self.r.add_task(path, link, layout.tasks, "nap", "sleep 1", cwd=os.path.join(link, "frontend"))
        text = self.read(path)
        self.assertNotIn("..", text)
        self.assertEqual(text.count('cwd = "frontend"'), 2, text)

    def test_hand_written_cwds_are_kept_as_written(self):
        home = tempfile.mkdtemp()
        os.makedirs(os.path.join(home, "x"))
        os.makedirs(os.path.join(self.d, "b"))
        old_home = os.environ.get("HOME")
        os.environ["HOME"] = home
        try:
            path = os.path.join(self.d, ".lumen-layout.toml")
            self.write(path, '[[task]]\nname = "h"\nrun = "a"\ncwd = "~/x"\n\n'
                             '[[task]]\nname = "d"\nrun = "b"\ncwd = "./a/../b"\n')
            layout = self.lf.load(path)
            self.assertEqual(layout.tasks[0], self.lf.Task("h", "a", os.path.join(home, "x")))  # equality ignores the text
            moved = self.lf.Task("d", "b", self.d)  # one the plugin changed: written fresh
            self.lf.write_owned(path, [layout.tasks[0], layout.tasks[1]], None)
            text = self.read(path)
            self.assertIn('cwd = "~/x"\n', text)
            self.assertIn('cwd = "./a/../b"\n', text)
            self.lf.write_owned(path, [layout.tasks[0], moved], None)
            self.assertNotIn("./a/../b", self.read(path))
        finally:
            if old_home is None:
                del os.environ["HOME"]
            else:
                os.environ["HOME"] = old_home

    # Important 2
    def test_procfile_commands_are_confirmed(self):
        T = self.lf.Task
        proc = T("web", "python -m http.server", "/p", None, "procfile")
        self.assertTrue(self.r.needs_confirm([proc], set()))
        self.assertFalse(self.r.needs_confirm([proc], {"web"}))
        self.assertTrue(self.r.confirm_first(proc))
        self.assertTrue(self.r.confirm_first(T("a", "x", "/p")))
        self.assertFalse(self.r.confirm_first(T("b", "make b", "/p", None, "make")))
        self.assertEqual(self.r.task_file("/p/.lumen-layout.toml", proc), "/p/Procfile")
        self.assertEqual(self.r.task_file(None, proc), "/p/Procfile")
        self.assertEqual(self.r.task_file("/p/.lumen-layout.toml", T("a", "x", "/p/sub")), "/p/.lumen-layout.toml")

    def run_main(self, coro):
        import asyncio
        return asyncio.run(coro)

    def plugin(self, picks=(), prompts=(), confirm=False):
        g = load("project-layout")
        app = g["app"]
        calls = {"confirm": [], "notify": []}
        picks, prompts = list(picks), list(prompts)

        async def pick(items, **kw):
            return picks.pop(0)(items) if picks else None

        async def prompt(*a, **kw):
            return prompts.pop(0)

        async def confirm_(question, **kw):
            calls["confirm"].append((question, kw.get("message")))
            return confirm

        async def notify(title, body=""):
            calls["notify"].append((title, body))

        app.pick, app.prompt, app.confirm, app.notify = pick, prompt, confirm_, notify
        return g, calls

    def test_run_task_asks_before_a_procfile_command(self):
        self.write(os.path.join(self.d, "Procfile"), "web: python -m http.server\n")
        by_name = lambda name: lambda items: next(i for i, n, _ in items if n == name)  # noqa: E731
        g, calls = self.plugin(picks=[by_name("web")], confirm=False)
        pane = FakePane(1, self.d)
        self.run_main(g["run_task"](pane))
        self.assertEqual(len(calls["confirm"]), 1)
        self.assertIn("python -m http.server", calls["confirm"][0][1])
        self.assertEqual(pane.splits, [])  # said no
        g, calls = self.plugin(picks=[by_name("web"), by_name("web")], confirm=True)
        self.run_main(g["run_task"](pane))
        self.run_main(g["run_task"](pane))
        self.assertEqual(len(calls["confirm"]), 1)  # remembered
        self.assertEqual(len(pane.splits), 2)

    # f
    def test_a_task_you_typed_runs_without_asking(self):
        by_name = lambda name: lambda items: next(i for i, n, _ in items if n == name)  # noqa: E731
        g, calls = self.plugin(picks=[by_name("greet")], prompts=["echo hi", "greet"])
        pane = FakePane(1, self.d)
        self.run_main(g["add_task_command"](pane))
        self.run_main(g["run_task"](pane))
        self.assertEqual(calls["confirm"], [])
        self.assertEqual(len(pane.splits), 1)

    # e
    def test_restart_outside_a_task_split_does_nothing(self):
        g, calls = self.plugin()
        self.run_main(g["restart_task"](FakePane(1, self.d)))
        self.assertEqual(calls["notify"], [])

    # a
    def test_the_confirm_shows_where_tasks_run(self):
        T = self.lf.Task
        sub = os.path.join(self.d, "web")
        count, listing = self.r.open_listing(os.path.join(self.d, self.lf.NAME), [{"cwd": self.d, "run": "nvim"}],
                                             [T("a", "make a", self.d), T("b", "npm run dev", sub, "pkill node")],
                                             self.d)
        self.assertEqual(count, 3)
        self.assertIn("`nvim`  ·  `make a`  ·  `npm run dev` in " + self.r.tilde(sub), listing)
        self.assertNotIn("`make a` in", listing)
        self.assertIn("`pkill node`", listing)
        self.assertEqual(self.r.tilde(os.path.expanduser("~/x")), "~/x")

    # b
    def test_ctrl_c_is_stopped_not_failed(self):
        run = self.r.Runner()
        for pid in (1, 2):
            run.track("/a", 1, pid, self.lf.Task("t%d" % pid, "x", "/a"))
        self.assertEqual(run.failed(1, 130), (None, None))
        self.assertNotIn(1, run.failures)
        self.assertEqual(run.badge(2), ("1 running", None))

    # g
    def test_badge_says_done_when_everything_exited_ok(self):
        run = self.r.Runner()
        for pid in (1, 2):
            run.track("/a", 1, pid, self.lf.Task("t%d" % pid, "x", "/a"))
        run.failed(1, 0)
        self.assertEqual(run.badge(1), ("1 running", None))
        run.failed(2, 0)
        self.assertEqual(run.badge(1), ("done", None))

    # c
    def test_add_task_keeps_a_hand_edit_made_while_prompting(self):
        path = os.path.join(self.d, ".lumen-layout.toml")
        self.write(path, '[[task]]\nname = "web"\nrun = "npm run dev"\n\n[dev]\ntasks = ["web"]\n')
        layout = self.lf.load(path)
        tasks = self.r.all_tasks(layout, [])  # taken when Add Task started
        self.write(path, '[[task]]\nname = "web"\nrun = "npm start"\n\n[[task]]\nname = "hand"\nrun = "true"\n\n'
                         '[dev]\ntasks = ["web", "hand"]\n')
        self.r.add_task(path, self.d, tasks, "nap", "sleep 1")
        after = self.lf.load(path)
        self.assertEqual([(t.name, t.run) for t in after.tasks],
                         [("web", "npm start"), ("hand", "true"), ("nap", "sleep 1")])
        self.assertEqual(after.dev, ["web", "hand"])
        with self.assertRaises(ValueError):  # a name the file has now can't be reused
            self.r.add_task(path, self.d, tasks, "hand", "x")

    # d
    def test_unknown_keys_are_refused(self):
        path = os.path.join(self.d, ".lumen-layout.toml")
        for text, words in [('[[task]]\nname = "a"\nrun = "x"\ncolour = "red"\n', ("task 'a'", "colour")),
                            ('[[task]]\nname = "a"\nrun = "x"\n[dev]\ntasks = ["a"]\nextra = 1\n', ("[dev]", "extra"))]:
            self.write(path, text)
            with self.assertRaises(ValueError, msg=text) as cm:
                self.lf.load(path)
            for w in words:
                self.assertIn(w, str(cm.exception))

    # f
    def test_a_saved_task_is_approved_under_the_key_run_task_checks(self):
        path = os.path.join(self.d, ".lumen-layout.toml")
        data = tempfile.mkdtemp()
        saved = self.r.add_task(None, self.d, [], "greet", "echo hi", cwd=self.d)
        self.r.remember(data, *self.r.task_approval(path, saved[-1]))
        loaded = self.lf.load(path).tasks[-1]
        self.assertTrue(self.r.approved(data, *self.r.task_approval(path, loaded)))

