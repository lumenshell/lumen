"""Pomodoro: focus for a while, then take a break, in a pane of its own.

"Start Pomodoro" asks how long, then opens a strip under the pane you're in
with a big countdown. The tab shows it too (🍅 24:59, tinted red while you
focus and green on a break), so you can see it from any tab. A break starts
by itself when a focus session ends; the next session waits for you.

Keys in the timer's pane: space pauses (or starts the next session), s skips
to the next part, r starts this part again, + and - add or take off a
minute, q closes the timer.
"""

import asyncio
import datetime
import json
import os
import time

from lumen_plugin import LumenError, Plugin

app = Plugin()

# id: (label, focus minutes, break minutes, long break minutes)
PLANS = {
    "classic": ("25 minutes, then a 5 minute break", 25, 5, 15),
    "long": ("50 minutes, then a 10 minute break", 50, 10, 20),
    "short": ("15 minutes, then a 3 minute break", 15, 3, 10),
}
# Every fourth break is a long one.
LONG_EVERY = 4

PHASES = {
    # phase: (title, tab icon, tab colour, ANSI colour)
    "focus": ("FOCUS", "🍅", "red", "31"),
    "break": ("BREAK", "☕", "green", "32"),
    "long": ("LONG BREAK", "☕", "cyan", "36"),
}

# Digits 3 wide and 5 high, each square drawn two cells wide.
FONT = {
    "0": ["###", "#.#", "#.#", "#.#", "###"],
    "1": [".#.", "##.", ".#.", ".#.", "###"],
    "2": ["###", "..#", "###", "#..", "###"],
    "3": ["###", "..#", "###", "..#", "###"],
    "4": ["#.#", "#.#", "###", "..#", "..#"],
    "5": ["###", "#..", "###", "..#", "###"],
    "6": ["###", "#..", "###", "#.#", "###"],
    "7": ["###", "..#", "..#", "..#", "..#"],
    "8": ["###", "#.#", "###", "#.#", "###"],
    "9": ["###", "#.#", "###", "..#", "###"],
    ":": [".", "#", ".", "#", "."],
}
BIG_WIDTH = 34  # MM:SS
BIG_HEIGHT = 10  # title, blank, 5 digit rows, blank, bar, keys
HINT = "space pause · s skip · r restart · +/- minute · q close"


def clock(seconds: float) -> str:
    seconds = max(0, int(seconds + 0.999))  # 0.2s left still shows 0:01
    return f"{seconds // 60:02d}:{seconds % 60:02d}"


def big(text: str) -> list:
    """`text` (digits and colons) in the big font: five lines."""
    rows = []
    for r in range(5):
        row = "  ".join(FONT[c][r] for c in text)
        rows.append(row.replace("#", "██").replace(".", "  "))
    return rows


def bar(fraction: float, width: int) -> str:
    done = round(max(0.0, min(1.0, fraction)) * width)
    return "━" * done + "╌" * (width - done)


class Timer:
    """The timer's state, apart from drawing. `now` is the clock it reads."""

    def __init__(self, plan: str, now=time.monotonic):
        _, self.focus_min, self.break_min, self.long_min = PLANS[plan]
        self.now = now
        self.sessions = 0  # focus sessions finished
        self.start("focus")

    def minutes(self, phase: str) -> int:
        return {"focus": self.focus_min, "break": self.break_min, "long": self.long_min}[phase]

    def start(self, phase: str) -> None:
        self.phase = phase
        self.length = self.minutes(phase) * 60
        self.left = self.length
        self.deadline = self.now() + self.left
        self.paused = False
        # Waiting for you to start it (a focus session, after a break).
        self.ready = False

    def remaining(self) -> float:
        if self.paused or self.ready:
            return self.left
        return max(0.0, self.deadline - self.now())

    def toggle(self) -> None:
        if self.ready:
            self.ready = False
            self.deadline = self.now() + self.left
        elif self.paused:
            self.paused = False
            self.deadline = self.now() + self.left
        else:
            self.left = self.remaining()
            self.paused = True

    def adjust(self, seconds: int) -> None:
        """Add (or take off) time, keeping at least a minute."""
        left = max(60.0, self.remaining() + seconds)
        self.length = max(self.length + (left - self.remaining()), left)
        self.left = left
        self.deadline = self.now() + left

    def finish(self) -> str:
        """End this part and move on; returns what to tell you."""
        if self.phase == "focus":
            self.sessions += 1
            nxt = "long" if self.sessions % LONG_EVERY == 0 else "break"
            self.start(nxt)
            return f"Focus session done. Take {self.minutes(nxt)} minutes."
        self.start("focus")
        self.ready = True
        return "Break's over. Press space in the timer when you're ready."

    def badge(self) -> str:
        icon = "‖" if self.paused else PHASES[self.phase][1]
        return f"{icon} {'ready' if self.ready else clock(self.remaining())}"

    def color(self) -> str:
        if self.ready:
            return "blue"
        return "yellow" if self.paused else PHASES[self.phase][2]

    def render(self, cols: int, rows: int, today: int) -> list:
        """The pane's lines, centred, for a pane `cols` by `rows`."""
        title, _, _, ansi = PHASES[self.phase]
        if self.paused:
            ansi, status = "33", "paused"
        elif self.ready:
            ansi, status = "34", "press space to start"
        else:
            status = f"{self.minutes(self.phase)} min"
        left = self.remaining()
        done = 1 - left / self.length if self.length else 1
        count = f"today {today} 🍅" if today else ""
        head = f"\x1b[1;{ansi}m{title}\x1b[22;2m  ·  {status}" + (f"  ·  {count}" if count else "") + "\x1b[0m"
        head_len = len(title) + 5 + len(status) + (5 + len(count) + 1 if count else 0)

        if cols < BIG_WIDTH + 2 or rows < BIG_HEIGHT:
            # Too small for the big digits: one line.
            line = f"\x1b[1;{ansi}m{title} {clock(left)}\x1b[0m"
            # " TITLE MM:SS  " then the bar.
            width = max(4, min(30, cols - len(title) - 9))
            line += f"  \x1b[{ansi}m{bar(done, width)}\x1b[0m"
            return [""] * max(0, (rows - 1) // 2) + [" " + line]

        def centre(text: str, length: int) -> str:
            return " " * max(0, (cols - length) // 2) + text

        lines = [centre(head, head_len), ""]
        lines += [centre(f"\x1b[{ansi}m{r}\x1b[0m", BIG_WIDTH) for r in big(clock(left))]
        lines += ["", centre(f"\x1b[{ansi}m{bar(done, BIG_WIDTH)}\x1b[0m", BIG_WIDTH)]
        hint = HINT if cols >= len(HINT) + 2 else "space · s · r · +/- · q"
        lines.append(centre(f"\x1b[2m{hint}\x1b[0m", len(hint)))
        top = max(0, (rows - len(lines)) // 2)
        return [""] * top + lines


# ---- today's count -----------------------------------------------------------

def stats_path() -> str:
    return os.path.join(app.data_dir, "stats.json")


def load_stats() -> dict:
    try:
        with open(stats_path()) as f:
            data = json.load(f)
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def today_count() -> int:
    value = load_stats().get(datetime.date.today().isoformat(), 0)
    return value if isinstance(value, int) else 0


def count_session() -> None:
    stats = load_stats()
    day = datetime.date.today().isoformat()
    stats[day] = (stats.get(day) if isinstance(stats.get(day), int) else 0) + 1
    # A year is plenty.
    stats = dict(sorted(stats.items())[-366:])
    tmp = stats_path() + ".tmp"
    with open(tmp, "w") as f:
        json.dump(stats, f)
    os.replace(tmp, stats_path())


# ---- the running timer -------------------------------------------------------

class Running:
    """The timer on screen: its pane, the pane it opened under, its loop."""

    def __init__(self, timer: Timer, view, home):
        self.timer, self.view, self.home = timer, view, home
        self.task = None
        self.wake = asyncio.Event()

    async def draw(self) -> None:
        if self.view.closed:
            return
        try:
            await self.view.draw(self.timer.render(self.view.cols, self.view.rows, today_count()))
            await self.view.set_tab_style(badge=self.timer.badge(), color=self.timer.color())
        except LumenError:
            pass  # closed while drawing

    async def loop(self) -> None:
        while not self.view.closed:
            if not (self.timer.paused or self.timer.ready) and self.timer.remaining() <= 0:
                was_focus = self.timer.phase == "focus"
                message = self.timer.finish()
                if was_focus:
                    count_session()
                await app.notify("🍅 Pomodoro", message)
            await self.draw()
            # Wake on the next whole second, or sooner for a key.
            left = self.timer.remaining()
            wait = (left - int(left)) or 1.0
            self.wake.clear()
            try:
                await asyncio.wait_for(self.wake.wait(), timeout=max(0.05, wait))
            except asyncio.TimeoutError:
                pass

    def poke(self) -> None:
        self.wake.set()


running = None


async def stop() -> None:
    global running
    r, running = running, None
    if r is None:
        return
    if r.task:
        r.task.cancel()
    # The tab it marked may outlive the timer's pane.
    for pane in (r.view, r.home):
        try:
            await pane.set_tab_style(badge=None, color=None)
            break
        except LumenError:
            continue
    if not r.view.closed:
        try:
            await r.view.close()
        except LumenError:
            pass


@app.command("Start Pomodoro")
async def start_pomodoro(pane):
    global running
    if pane is None:
        return
    items = [(plan, label) for plan, (label, *_rest) in PLANS.items()]
    message = "A timer is running; this starts a new one." if running else None
    plan = await app.pick(items, placeholder="Focus for how long?", message=message)
    if plan is None:
        return
    await stop()
    home = pane
    view = await home.split("down", plugin_pane=True, focus=False, size=BIG_HEIGHT + 2)
    r = Running(Timer(plan), view, home)
    running = r

    @view.on_key
    async def key(name):
        t = r.timer
        if name in ("space", "p", "enter"):
            t.toggle()
        elif name == "s":
            # A skipped session isn't counted for today; skipping a break
            # goes straight back to focus.
            t.finish()
            t.ready = False
        elif name == "r":
            t.start(t.phase)
        elif name in ("+", "="):
            t.adjust(60)
        elif name in ("-", "_"):
            t.adjust(-60)
        elif name in ("q", "escape"):
            await stop()
            return
        r.poke()

    @view.on_resize
    def resized():
        r.poke()

    @view.on_close
    async def closed():
        if running is r:
            await stop()

    r.task = asyncio.ensure_future(r.loop())


@app.command("Pause or Resume Pomodoro")
async def pause_pomodoro(pane):
    if running:
        running.timer.toggle()
        running.poke()


@app.command("Stop Pomodoro")
async def stop_pomodoro(pane):
    await stop()


app.run()
