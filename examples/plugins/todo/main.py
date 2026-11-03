"""Todo List (Alt+T): a todo list in a pane on the right of the one you're
in. Choose it again to close it. The tab shows how many are left.

Keys in the list: ↑/↓ (or j/k) move, space ticks one off, a adds one (type,
then Enter; Esc cancels), e edits, d deletes, c clears the ticked ones, q
closes the pane. The list is kept in the plugin's data folder.

A small example of a plugin pane that takes text input: it keeps its own
line editor, since nothing else is reading keys there.
"""

import json
import os

from lumen_plugin import LumenError, Plugin

app = Plugin()
FILE = os.path.join(app.data_dir, "todo.json")


def load():
    try:
        with open(FILE) as f:
            items = json.load(f)
        return [{"text": str(i["text"]), "done": bool(i.get("done"))} for i in items if "text" in i]
    except (OSError, ValueError, TypeError):
        return []


def save(items):
    tmp = FILE + ".tmp"
    with open(tmp, "w") as f:
        json.dump(items, f, indent=1)
    os.replace(tmp, FILE)


class List:
    def __init__(self, view, home):
        self.view, self.home = view, home
        self.items = load()
        self.cursor = 0
        # Text being typed, and which item it replaces (None: a new one).
        self.editing = None
        self.edit_index = None

    def lines(self):
        cols, rows = self.view.cols, self.view.rows
        left = sum(not i["done"] for i in self.items)
        out = [f"\x1b[1m Todo\x1b[0m\x1b[2m  {left} left\x1b[0m", ""]
        if not self.items and self.editing is None:
            out.append("\x1b[2m Nothing yet. Press a to add one.\x1b[0m")
        for n, item in enumerate(self.items):
            mark = "\x1b[32m✓\x1b[0m" if item["done"] else "○"
            text = item["text"][: max(1, cols - 5)]
            if item["done"]:
                text = f"\x1b[2;9m{text}\x1b[0m"
            selected = n == self.cursor and self.editing is None
            out.append(("\x1b[1;34m›\x1b[0m" if selected else " ") + f" {mark} {text}")
        if self.editing is not None:
            what = "edit" if self.edit_index is not None else "new"
            out += ["", f" {what}: {self.editing[-(cols - 10):]}\x1b[7m \x1b[0m"]
        if self.editing is not None:
            hints = ["enter save · esc cancel", "enter · esc"]
        else:
            hints = ["a add · space tick · e edit · d delete · q close", "a add · space tick · q close", "a · space · q"]
        hint = next((h for h in hints if len(h) <= cols - 2), "")
        out = out[: max(1, rows - 2)]
        out += [""] * max(0, rows - 1 - len(out)) + [f"\x1b[2m {hint}\x1b[0m"]
        return out

    async def show(self):
        if self.view.closed:
            return
        left = sum(not i["done"] for i in self.items)
        try:
            await self.view.draw(self.lines())
            await self.home.set_tab_style(badge=f"{left} to do" if left else "all done", color=None)
        except LumenError:
            pass

    def key(self, name):
        if self.editing is not None:
            return self.edit_key(name)
        n = len(self.items)
        if name in ("down", "j"):
            self.cursor = min(n - 1, self.cursor + 1)
        elif name in ("up", "k"):
            self.cursor = max(0, self.cursor - 1)
        elif name in ("space", "x", "enter") and n:
            self.items[self.cursor]["done"] = not self.items[self.cursor]["done"]
        elif name == "a":
            self.editing, self.edit_index = "", None
        elif name == "e" and n:
            self.editing, self.edit_index = self.items[self.cursor]["text"], self.cursor
        elif name == "d" and n:
            del self.items[self.cursor]
        elif name == "c":
            self.items = [i for i in self.items if not i["done"]]
        else:
            return
        self.cursor = max(0, min(self.cursor, len(self.items) - 1))
        save(self.items)

    def edit_key(self, name):
        if name == "enter":
            text = self.editing.strip()
            if text and self.edit_index is None:
                self.items.append({"text": text, "done": False})
                self.cursor = len(self.items) - 1
            elif text:
                self.items[self.edit_index]["text"] = text
            self.editing = None
            save(self.items)
        elif name == "escape":
            self.editing = None
        elif name == "backspace":
            self.editing = self.editing[:-1]
        elif name == "space":
            self.editing += " "
        elif len(name) == 1 and name.isprintable():
            self.editing += name


current = None


@app.command("Todo List", keys="alt-t")
async def todo(pane):
    global current
    if current and not current.view.closed:
        await current.view.close()
        return
    if pane is None:
        return
    # Its tab is your tab, so it leaves the title alone: a plugin pane is
    # named after its plugin anyway.
    view = await pane.split("right", plugin_pane=True, focus=True, size=0.3)
    t = current = List(view, pane)

    @view.on_key
    async def key(name):
        if name == "q" and t.editing is None:
            await view.close()
            return
        t.key(name)
        await t.show()

    view.on_resize(t.show)

    @view.on_close
    async def closed():
        try:
            await t.home.set_tab_style(badge=None)
        except LumenError:
            pass


app.run()
