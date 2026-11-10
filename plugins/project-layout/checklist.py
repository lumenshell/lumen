"""The checklist pane's model: which tasks are ticked, where the cursor is, and
how to draw it as lines. No Lumen calls in here, so it can be tested alone;
main.py feeds it keys and hands render()'s lines to the pane."""

from __future__ import annotations

import unicodedata

import lumen_plugin

HEADING = "Choose your dev environment"
FOOTER = "Space tick · a add task · Enter start · Esc cancel"
EMPTY = "Nothing to run found here. Press a to add a task."

RESET = "\x1b[0m"
BOLD = "\x1b[1m"
DIM = "\x1b[2m"
REVERSE = "\x1b[7m"


def _cell(ch: str) -> int:
    """Columns one character takes on screen."""
    if unicodedata.combining(ch) or unicodedata.category(ch) in ("Mn", "Me", "Cf", "Cc"):
        return 0
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1


def width(text: str) -> int:
    """Display width of plain text (no ANSI)."""
    return sum(_cell(ch) for ch in text)


def truncate(text: str, cols: int) -> str:
    """Cut `text` to at most `cols` columns, ending in "…" when it was cut."""
    if cols <= 0:
        return ""
    if width(text) <= cols:
        return text
    out, used = [], 0
    for ch in text:
        w = _cell(ch)
        if used + w > cols - 1:
            break
        out.append(ch)
        used += w
    return "".join(out) + "…"


def _clean(text: str) -> str:
    """Show control characters as spaces so a stray newline can't break a row."""
    return "".join(" " if unicodedata.category(c) == "Cc" else c for c in text)


def _command(run) -> str:
    """A task's command as one line; a list is joined as shell words."""
    if isinstance(run, str):
        return _clean(run)
    try:
        return " ".join(lumen_plugin.quote(w) for w in run)
    except ValueError:  # a word the shell can't take safely: show it plainly
        return _clean(" ".join(str(w) for w in run))


class Checklist:
    def __init__(self, tasks, ticked):
        self.tasks = list(tasks)
        self.ticked = {t.name for t in self.tasks if t.name in ticked}
        self.cursor = 0

    def key(self, key: str):
        """Handle a key by name. Returns "add", "start" or "cancel" when the
        caller must act, else None."""
        n = len(self.tasks)
        if key in ("up", "k"):
            self.cursor = max(0, self.cursor - 1)
        elif key in ("down", "j"):
            self.cursor = max(0, min(n - 1, self.cursor + 1))
        elif key == "space":
            if n:
                name = self.tasks[self.cursor].name
                self.ticked ^= {name}
        elif key == "a":
            return "add"
        elif key == "enter":
            return "start" if self.ticked else None
        elif key in ("escape", "q"):
            return "cancel"
        return None

    def add(self, task) -> None:
        self.tasks.append(task)
        self.ticked.add(task.name)
        self.cursor = len(self.tasks) - 1

    def chosen(self) -> list:
        return [t.name for t in self.tasks if t.name in self.ticked]

    def _row(self, i: int, cols: int) -> str:
        task = self.tasks[i]
        head = "[%s] %s" % ("x" if task.name in self.ticked else " ", _clean(task.name))
        text = truncate(head + "  " + _command(task.run), cols)
        if i == self.cursor:
            return REVERSE + text + RESET
        if text.startswith(head):
            return head + DIM + text[len(head):] + RESET
        return text

    def render(self, cols: int, rows: int) -> list:
        """Lines to draw in a cols x rows pane: never more than `rows` lines,
        none wider than `cols` columns."""
        if rows <= 0 or cols <= 0:
            return []
        if not self.tasks:
            msg = DIM + truncate(EMPTY, cols) + RESET
            if rows < 3:
                return [msg]
            return [BOLD + truncate(HEADING, cols) + RESET, msg, DIM + truncate(FOOTER, cols) + RESET][:rows]
        room = rows if rows < 3 else rows - 2  # no heading or footer when cramped
        top = max(0, self.cursor - room + 1)  # keep the cursor on the last visible row
        lines = [self._row(i, cols) for i in range(top, min(len(self.tasks), top + room))]
        if rows < 3:
            return lines
        return [BOLD + truncate(HEADING, cols) + RESET] + lines + [DIM + truncate(FOOTER, cols) + RESET]
