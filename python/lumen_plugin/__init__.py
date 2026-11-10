"""Write Lumen plugins and scripts in Python.

    from lumen_plugin import Plugin

    app = Plugin()

    @app.command("Say Hello", keys="alt-h")
    async def hello(pane):
        await app.notify("Hello", f"from {await pane.cwd()}")

    @app.on("command-finished")
    async def finished(event):
        app.log("exit", event["exit"], "after", event["duration_ms"], "ms")

    app.run()

Run by Lumen as a plugin (it sets LUMEN_PLUGIN), this talks to Lumen over
stdin and stdout. Run anywhere else, it connects to Lumen's socket instead
($LUMEN_SOCKET, or the most recently started Lumen); commands are for plugins
only, but everything else works the same.

Standard library only, Python 3.8 or newer. The protocol itself is described
in Lumen's README; this module is a thin layer over it.
"""

from __future__ import annotations

import asyncio
import glob
import inspect
import json
import os
import re
import stat
import sys
import traceback
import unicodedata

__all__ = ["Plugin", "Pane", "PluginPane", "Event", "LumenError", "quote", "typeable", "keys", "EVENTS", "TAB_COLORS"]
__version__ = "1.1.0"

#: Colours a tab can take (see `Pane.set_tab_style`), from your theme; an
#: exact "#rrggbb" works too.
TAB_COLORS = ("red", "green", "yellow", "blue", "magenta", "cyan")

_KEEP = object()

#: Events Lumen can send; see `Plugin.on`.
EVENTS = (
    "pane-opened",
    "pane-closed",
    "focus-changed",
    "title-changed",
    "cwd-changed",
    "bell",
    "command-started",
    "command-finished",
)

_PLAIN = re.compile(r"[A-Za-z0-9_./:,@+-]+")


# Control characters (Cc: newlines, escapes...) press keys rather than type
# text. Format characters (Cf: bidi overrides, zero-width ones) and line and
# paragraph separators (Zl, Zp) make text look different from what it is, so
# a command shown for you to confirm could hide what actually runs.
_UNSAFE = {"Cc", "Cf", "Zl", "Zp"}


def typeable(text: str) -> bool:
    """Can `text` be typed into a shell and shown to you as it really is? No
    control characters, invisible formatting (such as right-to-left
    overrides) or line separators."""
    return not any(unicodedata.category(c) in _UNSAFE for c in str(text))


def _control(text: str) -> bool:
    return not typeable(text)


def quote(word) -> str:
    """Quote one word for the shell: safe in bash, zsh and fish alike. Use it
    on anything you put in a command you didn't write yourself (a file or
    branch name, text from the screen). Raises ValueError for a word with
    control characters, which no quoting makes safe to type.

    (shlex.quote isn't enough: fish reads \\' inside single quotes as an
    escaped quote. Here backslashes and quotes never appear inside single
    quotes, so every shell reads the word the same way.)"""
    word = str(word)
    if _control(word):
        raise ValueError(f"can't safely type {word!r}: it contains control or invisible characters")
    if _PLAIN.fullmatch(word):
        return word
    out = []
    for c in word:
        if c == "'":
            out.append("'\"'\"'")
        elif c == "\\":
            out.append("'\"\\\\\"'")
        else:
            out.append(c)
    return "'" + "".join(out) + "'"

# Replies can be large (read-screen); the default 64 KiB line limit isn't enough.
_LINE_LIMIT = 64 * 1024 * 1024


class LumenError(Exception):
    """Lumen refused or couldn't carry out a request. `code` is the JSON-RPC
    error code: -32001 means the plugin lacks a permission (the message says
    which), -32000 that it couldn't be done (say, the pane has closed)."""

    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code


def _dir(path):
    """A directory for Lumen, with ~ expanded (Lumen doesn't)."""
    return None if path is None else os.path.expanduser(str(path))


def _command_text(command) -> str:
    """A command as typed: a string as-is, a list with each word quoted.
    Either way, control characters are refused: they'd press keys (a
    carriage return runs what's typed so far) instead of being text."""
    if isinstance(command, (list, tuple)):
        return " ".join(quote(c) for c in command)
    command = str(command)
    if _control(command):
        raise ValueError(f"can't safely type {command!r}: it contains control or invisible characters")
    return command


class Pane:
    """One terminal pane. Every method talks to Lumen, so await it."""

    def __init__(self, app: "Plugin", id: int):
        self.app = app
        self.id = id

    def __repr__(self) -> str:
        return f"Pane({self.id})"

    def __eq__(self, other) -> bool:
        return isinstance(other, Pane) and other.id == self.id

    def __hash__(self) -> int:
        return hash(self.id)

    async def info(self):
        """This pane's entry from `list` (id, title, cwd, cols, rows,
        last_exit, focused, ...), or None if it has closed."""
        for p in await self.app.panes():
            if p["id"] == self.id:
                return p
        return None

    async def cwd(self):
        """The shell's working directory, or None if unknown (it needs shell
        integration) or the pane has closed."""
        info = await self.info()
        return info and info.get("cwd")

    async def send_text(self, text: str) -> None:
        """Type `text` into the pane, exactly as given. Needs `input`."""
        await self.app.call("send-text", pane=self.id, text=text)

    async def run(self, command) -> None:
        """Type a command and press Enter. A list is quoted word by word, so
        `pane.run(["git", "switch", "--", branch])` is safe for any branch
        name; a string is typed as-is. Text with control characters raises
        ValueError (use send_text to send keys on purpose). Needs `input`."""
        await self.send_text(_command_text(command) + "\r")

    async def read_screen(self, lines: int | None = None) -> str:
        """The text on screen, or its last `lines` lines. Needs `screen`."""
        return (await self.app.call("read-screen", pane=self.id, lines=lines))["text"]

    async def set_title(self, title: str | None = None) -> None:
        """Name this pane's tab; None goes back to the program's title.
        Needs `panes`."""
        await self.app.call("set-title", pane=self.id, title=title)

    async def move_tab(self, position: int) -> None:
        """Move this pane's tab to `position` in its window, 1 being first.
        Needs `panes`."""
        if position < 1:
            raise ValueError("tab positions start at 1")
        await self.app.call("move-tab", pane=self.id, index=position - 1)

    async def focus(self) -> None:
        """Needs `panes`."""
        await self.app.call("focus", pane=self.id)

    async def close(self) -> None:
        """Needs `panes`."""
        await self.app.call("close", pane=self.id)

    async def split(
        self,
        direction: str = "right",
        *,
        cwd=None,
        run=None,
        focus: bool = True,
        size=None,
        plugin_pane: bool = False,
    ) -> "Pane":
        """Split this pane ("right" or "down") and return the new one, which
        starts in `cwd` (default: this pane's directory; ~ works) and runs `run` (a
        string, or a list to quote). Needs `panes`.

        With `plugin_pane=True` the new pane runs nothing: you draw it, and
        get what's typed in it. It returns a PluginPane.

        `size` is the new pane's share: a fraction (0.3), or a number of rows
        (down) or columns (right). Half by default."""
        if plugin_pane:
            if cwd is not None or run is not None:
                raise ValueError("a plugin pane runs no program, so takes no cwd or run")
            result = await self.app.call(
                "split", pane=self.id, direction=direction, focus=focus, size=size, plugin_pane=True
            )
            return self.app._adopt(result["pane"])
        result = await self.app.call(
            "split",
            pane=self.id,
            direction=direction,
            cwd=_dir(cwd),
            run=None if run is None else _command_text(run),
            focus=focus,
            size=size,
        )
        return Pane(self.app, result["pane"])

    async def set_tab_style(self, *, badge=_KEEP, color=_KEEP, short_badge=None) -> None:
        """Mark this pane's tab: `badge` is a short label beside its title (up
        to 24 characters, like "🍅 18:42"), `color` tints it: one of
        TAB_COLORS, or "#rrggbb". None removes either; what you leave out
        stays. `short_badge` (up to 8 characters, like "18m") is shown
        instead when the tab is too narrow for the badge; it goes with the
        badge it was given with. Needs `panes`."""
        params = {"pane": self.id}
        if badge is not _KEEP:
            params["badge"] = badge
            if short_badge is not None:
                params["short_badge"] = short_badge
        if color is not _KEEP:
            params["color"] = color
        await self.app.request("set-tab-style", params)

    async def set_pane_style(self, *, label=_KEEP, color=_KEEP) -> None:
        """Name this pane: `label` (up to 32 characters) is drawn in a strip
        above it when its tab is split, and `color` (one of TAB_COLORS, or
        "#rrggbb") tints the strip. None removes either; what you leave out
        stays. They go when your plugin stops. Needs `panes`."""
        params = {"pane": self.id}
        if label is not _KEEP:
            params["label"] = label
        if color is not _KEEP:
            params["color"] = color
        await self.app.request("set-pane-style", params)

    async def run_action(self, action: str) -> None:
        """Run a Lumen action (as named in [keys]) in this pane. Needs `input`."""
        await self.app.call("run-action", action=action, pane=self.id)


_KEY_NAMES = {
    "\r": "enter", "\n": "enter", "\t": "tab", "\x7f": "backspace", "\x08": "backspace",
    "\x1b": "escape", " ": "space",
}
_CSI_KEYS = {
    "A": "up", "B": "down", "C": "right", "D": "left", "H": "home", "F": "end",
    "2~": "insert", "3~": "delete", "5~": "pageup", "6~": "pagedown", "Z": "shift-tab",
}
_ESCAPE = re.compile(r"\x1b(?:\[[0-9;:<?]*[A-Za-z~]|O[A-Za-z])")


def keys(text: str) -> list:
    """The keys in a plugin pane's input, by name: "a", "A", "space",
    "enter", "escape", "backspace", "tab", "up", "down", "left", "right",
    "home", "end", "pageup", "pagedown", "delete", "ctrl-c" and so on. A
    sequence it doesn't know comes back as it is."""
    out, i = [], 0
    while i < len(text):
        m = _ESCAPE.match(text, i)
        if m:
            seq = m.group(0)
            body = seq[2:]
            # Modifiers (\x1b[1;5A) aren't named; the key is.
            if body.endswith("~"):
                body = body.split(";")[0].rstrip("~") + "~"
            name = _CSI_KEYS.get(body) or _CSI_KEYS.get(body[-1:])
            out.append(name or seq)
            i = m.end()
            continue
        c = text[i]
        if c in _KEY_NAMES:
            out.append(_KEY_NAMES[c])
        elif "\x01" <= c <= "\x1a":
            out.append("ctrl-" + chr(ord(c) + 96))
        else:
            out.append(c)
        i += 1
    return out


class PluginPane(Pane):
    """A pane you draw: it runs no program. What you `write` shows in it, as
    if a program had printed it (escape sequences for colour and cursor
    movement work), and what's typed in it comes to your `on_key` and
    `on_input` handlers. It closes when your plugin (or script) exits.

        view = await pane.split("down", plugin_pane=True, focus=False)

        @view.on_key
        async def key(name):
            if name == "q":
                await view.close()

    `cols` and `rows` are its size, updated before `on_resize` handlers run.
    `closed` turns True when it closes, by you or otherwise."""

    def __init__(self, app: "Plugin", id: int):
        super().__init__(app, id)
        self.cols = 80
        self.rows = 24
        self.closed = False
        self._sized = False
        self._on = {"key": [], "input": [], "resize": [], "close": []}

    def __repr__(self) -> str:
        return f"PluginPane({self.id})"

    async def write(self, text: str) -> None:
        """Show `text` (\\r\\n for a new line). Needs `panes`."""
        if self.closed:
            raise LumenError(-32000, "the pane has closed")
        await self.app.call("write", pane=self.id, text=text)

    async def draw(self, lines) -> None:
        """Replace everything shown with `lines` (a list of strings, or one
        string), cursor hidden: one write, so it never flickers."""
        if isinstance(lines, str):
            lines = lines.split("\n")
        await self.write("\x1b[?25l\x1b[H\x1b[2J" + "\r\n".join(lines))

    def on_key(self, fn):
        """Call `fn(name)` for each key typed in the pane (see `keys`)."""
        self._on["key"].append(fn)
        return fn

    def on_input(self, fn):
        """Call `fn(text)` with input exactly as the terminal sent it: keys,
        pastes, mouse reports if you turned them on."""
        self._on["input"].append(fn)
        return fn

    def on_resize(self, fn):
        """Call `fn()` when the pane changes size, starting with its first
        size (straight away if that's known already)."""
        self._on["resize"].append(fn)
        if self._sized:
            self.app._spawn(fn)
        return fn

    def on_close(self, fn):
        """Call `fn()` once the pane has closed."""
        self._on["close"].append(fn)
        return fn

    def _handle(self, params: dict) -> None:
        kind = params.get("event")
        if kind == "input":
            text = params.get("text", "")
            for fn in self._on["input"]:
                self.app._spawn(fn, text)
            for name in keys(text):
                for fn in self._on["key"]:
                    self.app._spawn(fn, name)
        elif kind == "resize":
            self.cols, self.rows = params.get("cols", self.cols), params.get("rows", self.rows)
            self._sized = True
            for fn in self._on["resize"]:
                self.app._spawn(fn)
        elif kind == "closed" and not self.closed:
            self.closed = True
            self.app._plugin_panes.pop(self.id, None)
            for fn in self._on["close"]:
                self.app._spawn(fn)


class Event:
    """Something that happened in Lumen. `event.name` is the event, `event.pane`
    the Pane it happened in (or None), and its other fields read like a dict:
    `event["exit"]`, `event.get("command")`."""

    def __init__(self, app: "Plugin", params: dict):
        self.data = {k: v for k, v in params.items() if k != "event"}
        self.name = params.get("event", "")
        pane = self.data.get("pane")
        self.pane = Pane(app, pane) if isinstance(pane, int) else None

    def __getitem__(self, key):
        return self.data[key]

    def get(self, key, default=None):
        return self.data.get(key, default)

    def __repr__(self) -> str:
        return f"Event({self.name!r}, {self.data!r})"


class Plugin:
    """A connection to Lumen, plus the commands and event handlers you add
    with the decorators. Call `run()` last."""

    def __init__(self):
        #: The plugin's name, and folder (for its own files).
        self.name = os.environ.get("LUMEN_PLUGIN") or os.path.splitext(os.path.basename(sys.argv[0]))[0]
        self.dir = os.environ.get("LUMEN_PLUGIN_DIR") or os.path.dirname(os.path.abspath(sys.argv[0]))
        #: True when Lumen started this as a plugin; False for a script using
        #: the socket.
        self.is_plugin = "LUMEN_PLUGIN" in os.environ
        self._data_dir = os.environ.get("LUMEN_PLUGIN_DATA")
        self._commands: dict = {}
        self._handlers: dict = {}
        self._startup: list = []
        self._pending: dict = {}
        self._next_id = 0
        self._send_line = None
        self._tasks: set = set()
        self._closed = False
        self._plugin_panes: dict = {}
        # Messages for a plugin pane that came before we knew of it.
        self._early: dict = {}

    @property
    def data_dir(self) -> str:
        """A folder for the plugin's own data (history, caches), kept across
        updates and readable only by you. Created on first use."""
        if not self._data_dir:
            base = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
            if sys.platform == "darwin":
                base = os.path.expanduser("~/Library/Application Support")
            self._data_dir = os.path.join(base, "lumen", "plugin-data", self.name)
        os.makedirs(self._data_dir, mode=0o700, exist_ok=True)
        return self._data_dir

    # ---- decorators -----------------------------------------------------

    def command(self, title: str, *, keys: str | None = None, id: str | None = None):
        """Add a palette command (and optionally a key, like "alt-b"). The
        function gets the focused Pane when you run it:

            @app.command("Open Notes", keys="alt-n")
            async def notes(pane): ...

        You can rebind it in config.toml as `plugin:<plugin>:<id>`, where the
        id defaults to the function's name."""

        def add(fn):
            self._commands[id or fn.__name__] = (title, keys, fn)
            return fn

        return add

    def on(self, *events: str):
        """Call the function with an Event whenever one of `events` happens,
        or any event if none are named:

            @app.on("cwd-changed", "focus-changed")
            async def moved(event): ..."""
        for e in events:
            if e not in EVENTS:
                raise ValueError(f"unknown event {e!r}; Lumen sends {', '.join(EVENTS)}")

        def add(fn):
            for e in events or ("*",):
                self._handlers.setdefault(e, []).append(fn)
            return fn

        return add

    def on_start(self, fn):
        """Call the function (no arguments) once connected."""
        self._startup.append(fn)
        return fn

    # ---- requests -------------------------------------------------------

    async def call(self, method: str, **params):
        """Send any request and return its result. Parameters left as None
        aren't sent. Raises LumenError if Lumen says no."""
        return await self.request(method, {k: v for k, v in params.items() if v is not None})

    async def request(self, method: str, params: dict):
        """Like `call`, with the parameters as a dict sent as they are
        (None as null)."""
        if self._send_line is None:
            raise RuntimeError("not connected: call this from a handler, or inside app.run()")
        if self._closed:
            raise ConnectionError("Lumen has closed the connection")
        self._next_id += 1
        rid = self._next_id
        future = asyncio.get_running_loop().create_future()
        self._pending[rid] = future
        self._send_line(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}))
        return await future

    async def hello(self) -> dict:
        """Lumen's version, protocol version, actions and events."""
        return await self.call("hello")

    async def tabs(self) -> list:
        """Every tab, each with its panes, as `list` returns them."""
        return (await self.call("list"))["tabs"]

    async def panes(self) -> list:
        """Every pane, each with its `tab` index and `window` added."""
        return [dict(p, tab=t["index"], window=t.get("window", 0)) for t in await self.tabs() for p in t["panes"]]

    async def focused(self) -> Pane | None:
        """The pane with keyboard focus."""
        pane = (await self.call("list")).get("focused_pane")
        return None if pane is None else Pane(self, pane)

    def pane(self, id: int) -> Pane:
        """A Pane by id (from an event, a command or `panes()`)."""
        return Pane(self, id)

    async def new_tab(self, *, cwd=None, run=None, plugin_pane: bool = False) -> Pane:
        """Open a tab in `cwd` (~ works) and return its pane. `run` is a
        string, or a list to quote. With `plugin_pane=True`, the tab holds a
        PluginPane for you to draw. Needs `panes`."""
        if plugin_pane:
            if cwd is not None or run is not None:
                raise ValueError("a plugin pane runs no program, so takes no cwd or run")
            return self._adopt((await self.call("new-tab", plugin_pane=True))["pane"])
        result = await self.call("new-tab", cwd=_dir(cwd), run=None if run is None else _command_text(run))
        return Pane(self, result["pane"])

    async def pick(self, items, *, placeholder: str | None = None, message: str | None = None):
        """Show a list in the palette and return the id of the one chosen, or
        None if closed. Items are strings, (id, label) or (id, label, detail)
        tuples, or dicts with those keys. `message` is shown under the input,
        wrapped, for anything that needs explaining."""
        out = []
        for item in items:
            if isinstance(item, str):
                out.append({"id": item, "label": item})
            elif isinstance(item, dict):
                out.append(item)
            else:
                id_, label, *rest = item
                out.append({"id": id_, "label": label, "detail": rest[0] if rest else ""})
        if not out:
            return None
        return (await self.call("show-picker", items=out, placeholder=placeholder, message=message))["id"]

    async def prompt(self, placeholder: str | None = None, *, message: str | None = None, initial: str = "") -> str | None:
        """Ask for a line of text in the palette and return it trimmed, or
        None if closed with Esc or left empty. `initial` is what the input
        starts with; `message` is shown under it, wrapped."""
        text = (await self.call("show-prompt", placeholder=placeholder, message=message, initial=initial))["text"]
        return text or None

    async def confirm(self, question: str, *, message: str | None = None, yes: str = "Yes", no: str = "No") -> bool:
        """Ask a yes/no question in the palette, with `message` (wrapped)
        under it; Esc counts as no."""
        return await self.pick([("yes", yes), ("no", no)], placeholder=question, message=message) == "yes"

    async def notify(self, title: str, body: str = "") -> None:
        """Show a desktop notification."""
        await self.call("notify", title=title, body=body)

    async def run_action(self, action: str) -> None:
        """Run a Lumen action (as named in [keys]) in the focused pane.
        Needs `input`."""
        await self.call("run-action", action=action)

    def _adopt(self, id: int) -> PluginPane:
        pane = PluginPane(self, id)
        self._plugin_panes[id] = pane
        for params in self._early.pop(id, []):
            pane._handle(params)
        return pane

    def log(self, *args) -> None:
        """Write to the plugin's log (stderr). `print` does the same in a
        plugin, since stdout is Lumen's."""
        print(*args, file=sys.stderr, flush=True)

    # ---- running --------------------------------------------------------

    def run(self, main=None) -> None:
        """Connect, register commands, and handle messages until Lumen goes
        away. Blocks; call it last.

        For a one-off script, pass an async function: it runs once connected,
        and the script ends when it returns.

            async def main():
                await app.new_tab(run="htop")
            app.run(main)"""
        try:
            asyncio.run(self._main(main))
        except KeyboardInterrupt:
            pass

    async def _main(self, main=None) -> None:
        if self._commands and not self.is_plugin:
            raise RuntimeError("commands need Lumen to run this as a plugin (see the README's Plugins section)")
        reader = await self._connect()
        loop_task = asyncio.ensure_future(self._read(reader))
        try:
            for cid, (title, keys, _) in self._commands.items():
                await self.call("register-command", id=cid, title=title, keys=keys)
            if self._handlers:
                events = None if "*" in self._handlers else sorted(self._handlers)
                await self.call("subscribe", events=events)
            for fn in self._startup:
                self._spawn(fn)
            if main is not None:
                await main()
                loop_task.cancel()
        except ConnectionError:
            pass
        try:
            await loop_task
        except asyncio.CancelledError:
            pass
        for task in list(self._tasks):
            task.cancel()

    async def _connect(self):
        loop = asyncio.get_running_loop()
        if self.is_plugin:
            out = sys.stdout
            # stdout carries the protocol; send stray prints to the log.
            sys.stdout = sys.stderr
            reader = asyncio.StreamReader(limit=_LINE_LIMIT)
            await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)

            def send(line: str) -> None:
                out.write(line + "\n")
                out.flush()

            self._send_line = send
            return reader
        reader, writer = await _open_socket()
        self._send_line = lambda line: writer.write((line + "\n").encode())
        return reader

    async def _read(self, reader) -> None:
        while True:
            try:
                line = await reader.readline()
            except (ConnectionError, ValueError) as e:
                self.log("lumen_plugin: connection lost:", e)
                break
            if not line:
                break
            try:
                msg = json.loads(line)
            except ValueError:
                self.log("lumen_plugin: ignoring a line that isn't JSON:", line[:200])
                continue
            self._dispatch(msg)
        self._closed = True
        for future in self._pending.values():
            if not future.done():
                future.set_exception(ConnectionError("Lumen has closed the connection"))
        self._pending.clear()

    def _dispatch(self, msg: dict) -> None:
        if "method" not in msg:
            future = self._pending.pop(msg.get("id"), None)
            if future is None or future.done():
                return
            if "error" in msg:
                err = msg["error"] or {}
                future.set_exception(LumenError(err.get("code", -32000), err.get("message", "error")))
            else:
                future.set_result(msg.get("result"))
            return
        params = msg.get("params") or {}
        if msg["method"] == "command":
            entry = self._commands.get(params.get("id"))
            pane = params.get("pane")
            if entry:
                self._spawn(entry[2], Pane(self, pane) if isinstance(pane, int) else None)
        elif msg["method"] == "pane":
            # About one of our plugin panes.
            pane = self._plugin_panes.get(params.get("pane"))
            if pane is not None:
                pane._handle(params)
            elif params.get("event") != "closed":
                self._early.setdefault(params.get("pane"), []).append(params)
        elif msg["method"] == "event":
            event = Event(self, params)
            for fn in self._handlers.get(event.name, []) + self._handlers.get("*", []):
                self._spawn(fn, event)

    def _spawn(self, fn, *args) -> None:
        """Run a handler without holding up other messages; log its errors."""

        async def guarded():
            try:
                result = fn(*args)
                if inspect.isawaitable(result):
                    await result
            except asyncio.CancelledError:
                raise
            except ConnectionError:
                pass
            except Exception:
                self.log(f"lumen_plugin: error in {getattr(fn, '__name__', fn)}:\n{traceback.format_exc()}")

        task = asyncio.ensure_future(guarded())
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)


def _private_to_me(path: str, kind) -> bool:
    """Is `path` (not followed if it's a link) of this kind, ours, and closed
    to everyone else? Another user could otherwise set up the folder (in a
    shared /tmp) and listen in Lumen's place."""
    try:
        st = os.lstat(path)
    except OSError:
        return False
    return kind(st.st_mode) and st.st_uid == os.getuid() and st.st_mode & 0o077 == 0


async def _open_socket():
    """$LUMEN_SOCKET, else the most recently started Lumen that answers, in
    the folder Lumen uses, which must be private to you as Lumen makes it."""
    path = os.environ.get("LUMEN_SOCKET")
    if path:
        return await asyncio.open_unix_connection(path, limit=_LINE_LIMIT)
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime:
        folder = os.path.join(runtime, "lumen")
    else:
        folder = os.path.join(os.environ.get("TMPDIR") or "/tmp", f"lumen-{os.getuid()}")
    if not _private_to_me(folder, stat.S_ISDIR):
        raise ConnectionError(f"{folder} isn't a private folder owned by you; not connecting")
    sockets = [p for p in glob.glob(os.path.join(folder, "lumen-*.sock")) if _private_to_me(p, stat.S_ISSOCK)]
    sockets.sort(key=os.path.getmtime, reverse=True)
    for path in sockets:
        try:
            return await asyncio.open_unix_connection(path, limit=_LINE_LIMIT)
        except OSError:
            continue  # left behind by a Lumen that crashed
    raise ConnectionError(f"no running Lumen found in {folder} (is socket-api turned off?)")
