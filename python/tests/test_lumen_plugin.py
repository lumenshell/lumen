"""Tests for lumen_plugin: each runs a real plugin process and plays Lumen's
side of the conversation over its stdin and stdout (or a Unix socket).

    python3 -m unittest discover -s python/tests
"""

import json
import os
import select
import shutil
import socket
import subprocess
import sys
import tempfile
import textwrap
import threading
import unittest

PACKAGE_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")


class FakeLumen:
    """Runs `source` as a plugin and talks to it like Lumen does."""

    def __init__(self, source: str):
        env = dict(os.environ, LUMEN_PLUGIN="test", LUMEN_PLUGIN_DIR="/tmp", PYTHONPATH=PACKAGE_DIR)
        env.pop("LUMEN_SOCKET", None)
        self.proc = subprocess.Popen(
            [sys.executable, "-c", textwrap.dedent(source)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )
        self.buf = b""

    def recv(self, timeout: float = 5.0) -> dict:
        while b"\n" not in self.buf:
            ready, _, _ = select.select([self.proc.stdout], [], [], timeout)
            if not ready:
                raise AssertionError("plugin sent nothing; its log:\n" + self.stderr())
            chunk = os.read(self.proc.stdout.fileno(), 65536)
            if not chunk:
                raise AssertionError("plugin closed stdout; its log:\n" + self.stderr())
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line)

    def send(self, msg: dict) -> None:
        self.proc.stdin.write((json.dumps(msg) + "\n").encode())
        self.proc.stdin.flush()

    def reply(self, request: dict, result=None) -> None:
        self.send({"jsonrpc": "2.0", "id": request["id"], "result": {} if result is None else result})

    def expect(self, method: str, result=None) -> dict:
        """Receive a request, check its method, answer it, return its params."""
        msg = self.recv()
        assert msg["method"] == method, f"expected {method}, got {msg}"
        self.reply(msg, result)
        return msg.get("params", {})

    def close(self) -> int:
        self.proc.stdin.close()
        code = self.proc.wait(timeout=5)
        self.proc.stdout.close()
        return code

    def stderr(self) -> str:
        if self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait()
        return self.proc.stderr.read().decode()


class PluginModeTests(unittest.TestCase):
    def test_registers_commands_and_subscribes_then_runs_them(self):
        lumen = FakeLumen("""
            from lumen_plugin import Plugin
            app = Plugin()

            @app.command("Say Hello", keys="alt-h")
            async def hello(pane):
                info = await pane.info()
                await app.notify("Hello", f"{pane.id} in {info['cwd']}")

            @app.on("bell")
            def rang(event):
                print("bell in", event.pane.id)

            app.run()
        """)
        self.assertEqual(lumen.expect("register-command"), {"id": "hello", "title": "Say Hello", "keys": "alt-h"})
        self.assertEqual(lumen.expect("subscribe", {"events": ["bell"]}), {"events": ["bell"]})
        lumen.send({"jsonrpc": "2.0", "method": "command", "params": {"id": "hello", "pane": 7}})
        listing = {"tabs": [{"index": 0, "panes": [{"id": 7, "cwd": "/src"}]}], "focused_pane": 7}
        lumen.expect("list", listing)
        self.assertEqual(lumen.expect("notify"), {"title": "Hello", "body": "7 in /src"})
        lumen.send({"jsonrpc": "2.0", "method": "event", "params": {"event": "bell", "pane": 3}})
        self.assertEqual(lumen.close(), 0)
        # print() went to the log, not into the protocol.
        self.assertIn("bell in 3", lumen.stderr())

    def test_list_commands_are_quoted_and_none_params_left_out(self):
        lumen = FakeLumen("""
            from lumen_plugin import Plugin
            app = Plugin()

            @app.on_start
            async def go():
                pane = app.pane(5)
                await pane.run(["git", "switch", "--", "x;touch pwned"])
                await pane.run("ls -la")
                new = await pane.split("down", run=["tail", "-f", "a b.log"])
                await new.set_title()

            app.run()
        """)
        self.assertEqual(lumen.expect("send-text")["text"], "git switch -- 'x;touch pwned'\r")
        self.assertEqual(lumen.expect("send-text")["text"], "ls -la\r")
        split = lumen.expect("split", {"pane": 9})
        self.assertEqual(split, {"pane": 5, "direction": "down", "run": "tail -f 'a b.log'", "focus": True})
        self.assertEqual(lumen.expect("set-title"), {"pane": 9})
        lumen.close()

    def test_pane_style_sends_only_what_was_given(self):
        lumen = FakeLumen("""
            from lumen_plugin import Plugin
            app = Plugin()

            @app.on_start
            async def go():
                pane = app.pane(5)
                await pane.set_pane_style(label="db")
                await pane.set_pane_style(label=None, color="red")

            app.run()
        """)
        self.assertEqual(lumen.expect("set-pane-style"), {"pane": 5, "label": "db"})
        self.assertEqual(lumen.expect("set-pane-style"), {"pane": 5, "label": None, "color": "red"})
        lumen.close()

    def test_plugin_panes_get_their_keys_size_and_closing(self):
        lumen = FakeLumen("""
            from lumen_plugin import Plugin
            app = Plugin()

            @app.on_start
            async def go():
                view = await app.pane(5).split("down", plugin_pane=True, focus=False)

                @view.on_resize
                async def resized():
                    await view.draw([f"{view.cols}x{view.rows}"])

                @view.on_key
                async def key(name):
                    await view.write(name + ";")

                @view.on_close
                def closed():
                    print("closed", view.closed)

                await view.set_tab_style(badge="🍅 5:00", short_badge="5m", color="red")
                await view.set_tab_style(badge=None)

            app.run()
        """)
        msg = lumen.recv()
        self.assertEqual(msg["params"], {"pane": 5, "direction": "down", "focus": False, "plugin_pane": True})
        # Its size can arrive before the reply is read: it's kept until then.
        lumen.send({"jsonrpc": "2.0", "method": "pane", "params": {"event": "resize", "pane": 9, "cols": 40, "rows": 6}})
        lumen.reply(msg, {"pane": 9})
        # Drawing (on its first size) and styling the tab run side by side.
        got = []
        for _ in range(3):
            got.append(lumen.recv())
            lumen.reply(got[-1])
        by_method = sorted((m["method"], json.dumps(m["params"], sort_keys=True)) for m in got)
        self.assertEqual(by_method, sorted([
            ("set-tab-style", json.dumps({"pane": 9, "badge": "🍅 5:00", "short_badge": "5m", "color": "red"}, sort_keys=True)),
            ("set-tab-style", json.dumps({"pane": 9, "badge": None}, sort_keys=True)),
            ("write", json.dumps({"pane": 9, "text": "\x1b[?25l\x1b[H\x1b[2J40x6"}, sort_keys=True)),
        ]))
        lumen.send({"jsonrpc": "2.0", "method": "pane", "params": {"event": "input", "pane": 9, "text": "q\x1b[A"}})
        self.assertEqual(lumen.expect("write"), {"pane": 9, "text": "q;"})
        self.assertEqual(lumen.expect("write"), {"pane": 9, "text": "up;"})
        lumen.send({"jsonrpc": "2.0", "method": "pane", "params": {"event": "closed", "pane": 9}})
        self.assertEqual(lumen.close(), 0)
        self.assertIn("closed True", lumen.stderr())

    def test_pick_accepts_strings_tuples_and_dicts(self):
        lumen = FakeLumen("""
            from lumen_plugin import Plugin
            app = Plugin()

            @app.on_start
            async def go():
                chosen = await app.pick(["a", ("b", "Bee"), ("c", "Sea", "more"), {"id": 4, "label": "Four"}],
                                        placeholder="Pick one")
                yes = await app.confirm("Sure?", message="It runs `rm`.")
                app.log("chose", chosen, yes)

            app.run()
        """)
        params = lumen.expect("show-picker", {"id": "c"})
        self.assertEqual(params["placeholder"], "Pick one")
        self.assertEqual(params["items"], [
            {"id": "a", "label": "a"},
            {"id": "b", "label": "Bee", "detail": ""},
            {"id": "c", "label": "Sea", "detail": "more"},
            {"id": 4, "label": "Four"},
        ])
        confirm = lumen.expect("show-picker", {"id": None})  # Esc
        self.assertEqual(confirm["message"], "It runs `rm`.")
        self.assertEqual([i["id"] for i in confirm["items"]], ["yes", "no"])
        lumen.close()
        self.assertIn("chose c False", lumen.stderr())

    def test_prompt_returns_text_or_none(self):
        lumen = FakeLumen("""
            from lumen_plugin import Plugin
            app = Plugin()

            @app.on_start
            async def go():
                a = await app.prompt("Command", initial="npm ")
                b = await app.prompt("Name", message="Pick one")
                c = await app.prompt()
                app.log("got", repr(a), repr(b), repr(c))

            app.run()
        """)
        params = lumen.expect("show-prompt", {"text": "npm run dev"})
        self.assertEqual(params["placeholder"], "Command")
        self.assertEqual(params["initial"], "npm ")
        params = lumen.expect("show-prompt", {"text": None})  # Esc
        self.assertEqual(params["message"], "Pick one")
        lumen.expect("show-prompt", {"text": ""})
        lumen.close()
        self.assertIn("got 'npm run dev' None None", lumen.stderr())

    def test_errors_raise_and_a_failing_handler_does_not_stop_the_plugin(self):
        lumen = FakeLumen("""
            from lumen_plugin import Plugin, LumenError
            app = Plugin()

            @app.command("Boom")
            async def boom(pane):
                raise ValueError("kaboom")

            @app.command("Type")
            async def type_(pane):
                try:
                    await pane.send_text("hi")
                except LumenError as e:
                    app.log("refused", e.code, e)

            app.run()
        """)
        lumen.expect("register-command")
        lumen.expect("register-command")
        lumen.send({"jsonrpc": "2.0", "method": "command", "params": {"id": "boom", "pane": 1}})
        lumen.send({"jsonrpc": "2.0", "method": "command", "params": {"id": "type_", "pane": 1}})
        req = lumen.recv()
        self.assertEqual(req["method"], "send-text")
        lumen.send({"jsonrpc": "2.0", "id": req["id"],
                    "error": {"code": -32001, "message": "needs the \"input\" permission"}})
        self.assertEqual(lumen.close(), 0)
        log = lumen.stderr()
        self.assertIn("ValueError: kaboom", log)
        self.assertIn('refused -32001 needs the "input" permission', log)

    def test_unknown_events_are_caught_early(self):
        lumen2 = FakeLumen("""
            from lumen_plugin import Plugin
            try:
                Plugin().on("command-finshed")
            except ValueError as e:
                print(e)
        """)
        printed = lumen2.proc.stdout.read().decode()
        lumen2.close()
        lumen2.proc.stderr.close()
        self.assertIn("command-finished", printed)


class QuotingTests(unittest.TestCase):
    """Words typed into a shell must come out exactly as given, in any shell."""

    NASTY = ["plain", "a b", "it's", "x\\", "x\\'; touch pwned; echo '", "$HOME", "`id`", "$(id)",
             "%self", "=ls", "~", "", "naïve 日本", "-rf", "a;b|c&d", "*?[x]", "{a,b}", "!!"]

    def setUp(self):
        sys.path.insert(0, PACKAGE_DIR)
        from lumen_plugin import quote
        self.quote = quote

    def test_posix_shells_read_the_word_back_exactly(self):
        for shell in ("bash", "dash", "zsh"):
            if not shutil.which(shell):
                continue
            for word in self.NASTY:
                out = subprocess.run([shell, "-c", "printf %s " + self.quote(word)], capture_output=True, text=True)
                self.assertEqual(out.stdout, word, f"{shell}: {word!r} quoted as {self.quote(word)}")

    def test_no_escapes_inside_single_quotes_so_fish_agrees(self):
        # fish (unlike POSIX shells) treats \\ and \\' as escapes inside single
        # quotes; the quoting never puts either there.
        for word in self.NASTY:
            q = self.quote(word)
            inside, quoted = False, []
            i = 0
            while i < len(q):
                c = q[i]
                if c == "'" and not inside:
                    inside, i = True, i + 1
                    continue
                if c == "'" and inside:
                    inside = False
                elif inside:
                    self.assertNotEqual(c, "\\", f"backslash inside quotes in {q}")
                i += 1
            if shutil.which("fish"):
                out = subprocess.run(["fish", "-c", "printf %s " + q], capture_output=True, text=True)
                self.assertEqual(out.stdout, word)

    def test_control_characters_are_refused(self):
        # Control characters, and invisible ones that disguise text: right-to-left
        # override and isolate, zero-width space, line and paragraph separators.
        for bad in ["a\nb", "a\rb", "\x1b[A", "x\x7f", "\x9b", "\t", "rm -rf ~ #\u202eexample",
                    "a\u2066b\u2069", "a\u200bb", "a\u2028b", "a\u2029b"]:
            with self.assertRaises(ValueError):
                self.quote(bad)


class SocketModeTests(unittest.TestCase):
    def test_a_folder_that_isnt_private_is_refused(self):
        folder = tempfile.mkdtemp()
        os.chmod(folder, 0o755)
        env = dict(os.environ, XDG_RUNTIME_DIR=folder, PYTHONPATH=PACKAGE_DIR)
        os.mkdir(os.path.join(folder, "lumen"), 0o755)
        env.pop("LUMEN_SOCKET", None)
        env.pop("LUMEN_PLUGIN", None)
        script = "from lumen_plugin import Plugin\napp = Plugin()\nasync def main(): pass\napp.run(main)"
        out = subprocess.run([sys.executable, "-c", script], env=env, capture_output=True, text=True, timeout=10)
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("isn't a private folder owned by you", out.stderr)

    def test_scripts_talk_over_the_socket(self):
        folder = tempfile.mkdtemp()
        path = os.path.join(folder, "lumen-1.sock")
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(path)
        server.listen(1)
        seen = []

        def serve():
            conn, _ = server.accept()
            f = conn.makefile("rw")
            for line in f:
                msg = json.loads(line)
                seen.append(msg)
                result = {"pane": 42, "tab": 1} if msg["method"] == "new-tab" else {}
                f.write(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}) + "\n")
                f.flush()
            conn.close()

        threading.Thread(target=serve, daemon=True).start()
        env = dict(os.environ, XDG_RUNTIME_DIR="/nonexistent", LUMEN_SOCKET=path, PYTHONPATH=PACKAGE_DIR)
        env.pop("LUMEN_PLUGIN", None)
        script = textwrap.dedent("""
            import asyncio
            from lumen_plugin import Plugin
            app = Plugin()
            async def main():
                pane = await app.new_tab(cwd="~/srv", run=["ssh", "my host"])
                print("opened", pane.id)
            app.run(main)
        """)
        out = subprocess.run([sys.executable, "-c", script], env=env, capture_output=True, text=True, timeout=10)
        server.close()
        self.assertEqual(seen[0]["params"], {"cwd": os.path.expanduser("~/srv"), "run": "ssh 'my host'"})
        self.assertIn("opened 42", out.stdout)

    def test_commands_need_plugin_mode(self):
        env = dict(os.environ, LUMEN_SOCKET="/nonexistent.sock", PYTHONPATH=PACKAGE_DIR)
        env.pop("LUMEN_PLUGIN", None)
        script = "from lumen_plugin import Plugin\napp = Plugin()\n@app.command('X')\ndef x(p): pass\napp.run()"
        out = subprocess.run([sys.executable, "-c", script], env=env, capture_output=True, text=True, timeout=10)
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("commands need Lumen to run this as a plugin", out.stderr)


if __name__ == "__main__":
    unittest.main()
