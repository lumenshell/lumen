"""Play Snake: a game in a tab of its own, drawn in a plugin pane.

Steer with the arrow keys (or WASD, or hjkl), eat the dots to grow, and
don't run into the walls or yourself. It speeds up as you grow. Space
pauses, r starts again, q closes the tab. The tab shows your score, and
your best is kept.

An example of a plugin pane that animates: it redraws the whole pane on
every step, which `draw` does in one write so nothing flickers.
"""

import asyncio
import json
import os
import random

from lumen_plugin import LumenError, Plugin

app = Plugin()
BEST = os.path.join(app.data_dir, "best.json")
DIRECTIONS = {
    "up": (0, -1), "w": (0, -1), "k": (0, -1),
    "down": (0, 1), "s": (0, 1), "j": (0, 1),
    "left": (-1, 0), "a": (-1, 0), "h": (-1, 0),
    "right": (1, 0), "d": (1, 0), "l": (1, 0),
}


def best():
    try:
        with open(BEST) as f:
            return int(json.load(f))
    except (OSError, ValueError, TypeError):
        return 0


def save_best(score):
    with open(BEST, "w") as f:
        json.dump(score, f)


class Game:
    """The board is `width` by `height` squares; each is two cells wide on
    screen, so it looks square."""

    def __init__(self, width, height, rng=random):
        self.width, self.height, self.rng = max(8, width), max(6, height), rng
        x, y = self.width // 2, self.height // 2
        self.snake = [(x, y), (x - 1, y), (x - 2, y)]
        self.heading = self.turn_to = (1, 0)
        self.score = 0
        self.over = False
        self.paused = False
        self.food = self.place_food()

    def place_food(self):
        free = [(x, y) for x in range(self.width) for y in range(self.height) if (x, y) not in self.snake]
        return self.rng.choice(free) if free else None

    def turn(self, d):
        # No turning straight back into yourself.
        if (d[0] + self.heading[0], d[1] + self.heading[1]) != (0, 0):
            self.turn_to = d

    def step(self):
        if self.over or self.paused:
            return
        self.heading = self.turn_to
        hx, hy = self.snake[0]
        head = (hx + self.heading[0], hy + self.heading[1])
        body = self.snake if head == self.food else self.snake[:-1]
        if not (0 <= head[0] < self.width and 0 <= head[1] < self.height) or head in body:
            self.over = True
            return
        self.snake.insert(0, head)
        if head == self.food:
            self.score += 1
            self.food = self.place_food()
        else:
            self.snake.pop()

    def delay(self):
        return max(0.05, 0.14 - 0.004 * self.score)

    def lines(self, best_score):
        cells = set(self.snake[1:])
        rows = [f"\x1b[1m Snake\x1b[0m  score {self.score}\x1b[2m  ·  best {max(best_score, self.score)}\x1b[0m"]
        rows.append(" ╭" + "──" * self.width + "╮")
        for y in range(self.height):
            row = []
            for x in range(self.width):
                if (x, y) == self.snake[0]:
                    row.append("\x1b[1;32m██\x1b[0m")
                elif (x, y) in cells:
                    row.append("\x1b[32m██\x1b[0m")
                elif (x, y) == self.food:
                    row.append("\x1b[31m●\x1b[0m ")
                else:
                    row.append("  ")
            rows.append(" │" + "".join(row) + "│")
        rows.append(" ╰" + "──" * self.width + "╯")
        if self.over:
            rows.append(" \x1b[1;31mGame over.\x1b[0m r to play again, q to close")
        elif self.paused:
            rows.append(" \x1b[33mPaused.\x1b[0m Space to carry on")
        else:
            rows.append(" \x1b[2marrows steer · space pause · r restart · q close\x1b[0m")
        return rows


@app.command("Play Snake")
async def play(pane):
    view = await app.new_tab(plugin_pane=True)
    await view.set_title("snake")
    state = {"game": None, "best": best()}
    started = asyncio.Event()

    def new_game():
        # Leave room for the title, the border and the line under it.
        state["game"] = Game((view.cols - 4) // 2, view.rows - 4)

    @view.on_resize
    def resized():
        game = state["game"]
        # A board that no longer fits starts again at the new size.
        if game is None or game.over or game.width > (view.cols - 4) // 2 or game.height > view.rows - 4:
            new_game()
        started.set()

    @view.on_key
    def key(name):
        game = state["game"]
        if game is None:
            return
        if name in DIRECTIONS:
            game.turn(DIRECTIONS[name])
        elif name == "space":
            game.paused = not game.paused
        elif name == "r":
            new_game()
        elif name in ("q", "escape"):
            asyncio.ensure_future(view.close())

    await started.wait()
    shown_score = None
    while not view.closed:
        game = state["game"]
        game.step()
        if game.over and game.score > state["best"]:
            state["best"] = game.score
            save_best(game.score)
        try:
            await view.draw(game.lines(state["best"]))
            if game.score != shown_score:
                shown_score = game.score
                await view.set_tab_style(badge=f"🐍 {game.score}", color="green")
        except LumenError:
            break
        await asyncio.sleep(game.delay())


app.run()
