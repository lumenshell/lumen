"""How a dev tab's splits are laid out: up to six tasks in a grid, row by row
(one row for two or fewer, else two rows). No Lumen calls in here."""

from __future__ import annotations

import math

#: Tasks per dev tab; more wrap to another tab.
MAX_PER_TAB = 6


def grid_plan(n: int) -> list:
    """For n tasks (1 to MAX_PER_TAB), the splits that make the grid, in the
    order to make them: (task, direction, of, size), where `of` is an
    already-made task's pane and `size` the new pane's share of it. Task 0 is
    the tab's first pane."""
    rows = 1 if n <= 2 else 2
    cols = math.ceil(n / rows)
    plan = []
    # Top row: each split takes all but one column's worth of what's left.
    for c in range(1, min(cols, n)):
        plan.append((c, "right", c - 1, (cols - c) / (cols - c + 1)))
    # Bottom row: under each top one.
    for c in range(cols):
        if cols + c < n:
            plan.append((cols + c, "down", c, 0.5))
    return plan
