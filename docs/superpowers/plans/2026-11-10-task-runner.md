# Task Runner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One key opens a project's dev environment (each task in its own labelled split, in a grid), with discovered, saved and one-off tasks, restart, and failure marks.

**Architecture:** Two small, general core features first (per-pane label/colour, and a text prompt for plugins), exposed over the socket API and `lumen_plugin`. Then the built-in `project-layout` plugin grows into the task runner, split into focused Python modules: the layout file (read and owned-section write), discovery, the grid, the checklist pane, and `main.py` wiring the commands.

**Tech Stack:** Rust (GPUI, `src/workspace.rs`, `src/api.rs`), Python 3.8+ standard library (`tomllib` 3.11+ for reading), `unittest`.

**Spec:** `docs/superpowers/specs/2026-11-09-task-runner-design.md`

## Global Constraints

- **Never `git commit`, tag or push.** Each task ends by staging its files (`git add`); the user commits. No Claude attribution anywhere.
- Built-in plugin stays named `project-layout` (keeps existing configs and the `plugin:project-layout:open_layout` binding working, no alias code). Its gallery title becomes "Projects & Tasks".
- Linux and macOS. Keys: Linux `ctrl-…`, macOS `cmd-…`, picked with `sys.platform == "darwin"`.
- `lumen_plugin` and the plugin use only the standard library; Python 3.8 must import them. Reading TOML needs `tomllib` (3.11+); without it, notify "This needs Python 3.11 or newer." as today. Writing never needs `tomllib`.
- Commands from a file are checked with `lumen_plugin.typeable` (no control or invisible characters) and shown before running, as Project Layouts does today.
- Existing `.lumen-layout.toml` files (only `title` and `[[pane]]`) open exactly as they do today, with no checklist.
- Per-pane labels and colours aren't saved in the session (like tab badges); they go when the plugin that set them disconnects.
- Socket protocol version is unchanged (additions only).
- Tests: `cargo test`, `python3 -m unittest discover -s python/tests`, `python3 -m unittest discover -s plugins/tests`.

## Review Focus

1. **Awkward commands** (quotes, backslashes, `$`, `;`, non-ASCII) must survive write → read exactly. Test in Task 3.
2. **Hand-edited files** with comments, blank lines, `[[pane]]` entries and hand-written `[[task]]`/`[dev]` blocks: a plugin write changes only the owned blocks. Test in Task 3.
3. **Shell integration off** (no cwd, no `command-finished`): Open Project notifies as today; restart falls back to a timed wait; no failure marks rather than wrong ones. Test in Task 6 (restart decision).
4. **Nothing discovered** (empty folder): the checklist still offers "Add task…" and explains, instead of an empty or broken pane. Test in Task 5.
5. **Hostile cloned file**: control or bidi characters in a task's `name`/`run`/`cwd`, `[dev]` naming an unknown task, `cwd` escaping with `..` is allowed (as `[[pane]]` allows) but must exist. Refused with a message naming the entry. Test in Task 3.

---

### Task 1: Core: per-pane label and colour (`set-pane-style`)

**Files:**
- Modify: `src/terminal_view.rs` (fields on `TerminalView`)
- Modify: `src/workspace.rs` (API handler near `"set-tab-style"` ~2066; disconnect cleanup ~2130; `pane_frame` ~1674 and its caller `render_node` ~1202; `api_list`)
- Modify: `src/api.rs:157` (`required`: `"set-pane-style"` → `Permission::Panes`)
- Modify: `python/lumen_plugin/__init__.py` (`Pane.set_pane_style`)
- Modify: `docs/scripting.md` (method table), `docs/plugin-protocol.md` (what plugins can do, permissions table), `docs/plugins.md` (Pane reference, permissions table)
- Test: `src/workspace.rs` tests module, `python/tests/test_lumen_plugin.py`

**Interfaces:**
- Produces (socket API): `set-pane-style` with `pane`, `label` (string ≤ 32 chars, trimmed, empty or `null` clears), `color` (a `TabColor` name or `#rrggbb`, `null` clears). Fields left out stay. Answers `{}`. Needs `panes`.
- Produces (`list`): each pane entry gains `label` and `color` (as given, or `null`).
- Produces (Python): `async Pane.set_pane_style(self, *, label=_KEEP, color=_KEEP) -> None`. Left out stays, `None` clears.
- Produces (Rust): `fn parse_pane_style(params: &Value, label: Option<String>, color: Option<TabColor>) -> Result<(Option<String>, Option<TabColor>), ApiError>` in `src/workspace.rs`. Pure, shared by the handler and the test.

- [ ] **Step 1: Write the failing Rust test** `pane_styles_keep_clear_and_check` in the `workspace.rs` tests:

```rust
let cur = (Some("db".to_string()), Some(TabColor::Ansi(2)));
let p = |v: Value| parse_pane_style(&v, cur.0.clone(), cur.1);
assert_eq!(p(json!({})).unwrap(), cur);                                   // left out stays
assert_eq!(p(json!({"label": null})).unwrap(), (None, cur.1));            // null clears
assert_eq!(p(json!({"label": "  "})).unwrap(), (None, cur.1));            // blank clears
assert_eq!(p(json!({"label": " web ", "color": "red"})).unwrap(), (Some("web".into()), Some(TabColor::Ansi(1))));
assert!(p(json!({"label": "x".repeat(33)})).is_err());
assert!(p(json!({"color": "orange"})).is_err());
assert!(p(json!({"label": 3})).is_err());
```

- [ ] **Step 2: Run it and see it fail.** `cargo test pane_styles_keep_clear_and_check`. Expected: compile error, `parse_pane_style` not found.

- [ ] **Step 3: Implement.**
  - `TerminalView` gains `pub label: Option<String>`, `pub label_color: Option<TabColor>`, `pub style_owner: Option<u64>`.
  - `parse_pane_style` as above, with messages in the style of `set-tab-style`'s (e.g. "`label` should be at most 32 characters").
  - Handler `"set-pane-style"`: target pane via `target_pane`, apply, set `style_owner` the same way `set-tab-style` does (plugin connection, or `None` for a socket client), `cx.notify()`.
  - Disconnect cleanup: next to the tab-style cleanup, clear `label`, `label_color` and `style_owner` on every pane whose `style_owner == conn`.
  - Drawing: in `pane_frame`, when `multi` and the pane has a label, draw a header strip inside the frame above the terminal: the label in the UI font, small, with a dot or tint in `label_color` (resolve the colour the way tab tints are resolved). The terminal fills the rest, so its grid shrinks by the strip's height. Unfocused panes keep their dimming. Match the existing look; exact metrics are the implementer's call.
  - `api_list`: add `label` and `color` to each pane entry.
  - `required`: add `"set-pane-style"` to the `Panes` arm.

- [ ] **Step 4: Run.** `cargo test`. Expected: all pass.

- [ ] **Step 5: Python helper, test first.** In `python/tests/test_lumen_plugin.py`, add `test_pane_style_sends_only_what_was_given`, following the existing fake-connection tests: `await pane.set_pane_style(label="db")` sends `set-pane-style` with `{"pane": id, "label": "db"}` (no `color` key); `set_pane_style(label=None, color="red")` sends `{"pane": id, "label": None, "color": "red"}`. Run it and see it fail, then add `Pane.set_pane_style` (mirror `set_tab_style`'s `_KEEP` handling), run `python3 -m unittest discover -s python/tests` and see it pass.

- [ ] **Step 6: Docs.** Add `set-pane-style` to the methods table in `docs/scripting.md` and note `label`/`color` in `list`; add a "Label splits" bullet to `docs/plugin-protocol.md` and the method to its `panes` row; add `set_pane_style` to the Pane reference and `panes` row in `docs/plugins.md`.

- [ ] **Step 7: Try it.** `cargo run`, split a tab, then from another terminal: `lumen msg set-pane-style` (or `socat` as in `docs/scripting.md`) with `{"label":"db","color":"green"}`. Expected: the split shows "db" with a green mark; a single-pane tab shows nothing.

- [ ] **Step 8: Stage.** `git add src/terminal_view.rs src/workspace.rs src/api.rs python/lumen_plugin/__init__.py python/tests/test_lumen_plugin.py docs/scripting.md docs/plugin-protocol.md docs/plugins.md`

---

### Task 2: Core: text prompt for plugins (`show-prompt`)

Add Task, Run Command… and Save as Task need typed text; plugins can only pick from a list today. The palette already has a text prompt (`Palette::prompt` in `src/palette.rs:185`, used for renaming tabs).

**Files:**
- Modify: `src/workspace.rs` (handler next to `"show-picker"`)
- Modify: `python/lumen_plugin/__init__.py` (`Plugin.prompt`)
- Modify: `docs/scripting.md`, `docs/plugin-protocol.md`, `docs/plugins.md`
- Test: `python/tests/test_lumen_plugin.py`

**Interfaces:**
- Produces (socket API): `show-prompt` with `placeholder`, `message`, `initial` (all optional strings). Opens the palette as a text prompt. Answers `{"text": "<trimmed text>"}` on Enter, or `{"text": null}` on Esc. No permission needed (like `show-picker`).
- Produces (Python): `async Plugin.prompt(self, placeholder: str | None = None, *, message: str | None = None, initial: str = "") -> str | None`. Returns `None` for Esc or empty text.

- [ ] **Step 1: Failing Python test** `test_prompt_returns_text_or_none`: with the fake connection answering `{"text": "npm run dev"}`, `await app.prompt("Command", initial="npm ")` returns `"npm run dev"` and sent `show-prompt` with `placeholder="Command"`, `initial="npm "`; answering `{"text": None}` or `{"text": ""}` returns `None`.
- [ ] **Step 2: Run and see it fail.** `python3 -m unittest discover -s python/tests`. Expected: `AttributeError: 'Plugin' object has no attribute 'prompt'`.
- [ ] **Step 3: Implement** `Plugin.prompt`, and the Rust handler: reply the same way `show-picker` replies, but open `Palette::prompt` whose submit callback answers with the text, and closing answers `null`. Validate the params are strings (invalid params error otherwise).
- [ ] **Step 4: Run** all three test commands. Expected: pass.
- [ ] **Step 5: Try it.** `cargo run`, then send `show-prompt` over the socket with `{"placeholder":"Name"}`; type, Enter. Expected: the reply carries the text; Esc gives `null`.
- [ ] **Step 6: Docs**: method table row; "Ask you to type" bullet in `docs/plugin-protocol.md`; `prompt` in the plugin guide's "Talking to Lumen" and reference.
- [ ] **Step 7: Stage** the changed files.

---

### Task 3: Plugin: the layout file (`layoutfile.py`)

Moves the file handling out of `plugins/project-layout/main.py` and adds tasks, the dev environment, and a writer that touches only its own blocks.

**File format** (additions; `title` and `[[pane]]` unchanged):

```toml
[[task]]
name = "db"                       # unique; shown on the split
run = "docker compose up db"      # string (typed as-is) or list (quoted)
cwd = "."                         # optional, relative to the file
stop = "docker compose stop db"   # optional, run when its split closes

[dev]
tasks = ["db", "server", "js", "scss"]   # names: from [[task]] or discovered
```

**Files:**
- Create: `plugins/project-layout/layoutfile.py`
- Modify: `plugins/project-layout/main.py` (import from it; remove the moved code)
- Test: `plugins/tests/test_builtins.py` (new `LayoutFileTests`; make the test loader put `plugins/project-layout` on `sys.path` so `import layoutfile` works)

**Interfaces:**
- Produces:
  - `NAME = ".lumen-layout.toml"`
  - `find_layout(start: str) -> str | None` (moved as is)
  - `project_root(start: str) -> str`: the folder of `find_layout(start)`, else `git rev-parse --show-toplevel`, else `start`.
  - `load(path: str, known: frozenset[str] = frozenset()) -> Layout` raising `ValueError` naming the bad entry; `known` is the discovered task names `[dev]` may also refer to.
  - `Layout` (a dataclass or plain class): `title: str | None`, `panes: list[dict]` (as today's `load` returns), `tasks: list[Task]`, `dev: list[str] | None` (`None` when there's no `[dev]` table).
  - `Task`: `name: str`, `run: str | list[str]`, `cwd: str` (absolute), `stop: str | list[str] | None`, `source: str` (`"saved"` here, a discovery source in Task 4). Equality by all fields.
  - `write_owned(path: str, tasks: list[Task], dev: list[str] | None) -> None`: replaces every `[[task]]` block and the `[dev]` block in the file (or creates the file) with freshly written ones appended at the end, leaving every other byte unchanged. `cwd` is written relative to the file (`"."` omitted). Atomic write (temp file + rename).
  - `toml_str(value: str) -> str`: a TOML basic string with `\`, `"` and control characters escaped.

- [ ] **Step 1: Failing tests** in `LayoutFileTests`:
  - `test_old_files_load_unchanged`: today's example from `main.py`'s docstring loads with the same `panes` as before, `tasks == []`, `dev is None`.
  - `test_tasks_and_dev_load`: a file with two `[[task]]` and `[dev]` loads them; `cwd` resolves against the file's folder.
  - `test_bad_entries_are_refused`: each raises `ValueError` whose message names the entry: `run` containing `"\x1b"`; a name with `"‮"`; duplicate names; `[dev]` listing `"nope"` when `known=` (see below) doesn't include it; a `cwd` that doesn't exist.
  - `test_writing_keeps_everything_else`: start from text with a header comment, `title`, two commented `[[pane]]` blocks, a hand-written `[[task]]` between them and a `[dev]` block; `write_owned` with new tasks/dev; assert the result equals the original minus the old owned blocks, plus the new blocks at the end. Write the expected text out in full.
  - `test_awkward_commands_round_trip`: tasks whose `run` is `'echo "a\\b" $HOME; printf \'%s\' café'` and a list `["say", "it's \"x\""]` survive `write_owned` → `load` exactly.
  - `test_first_write_creates_the_file`: no file → creates one containing only the owned blocks.
- [ ] **Step 2: Run and see them fail.** `python3 -m unittest discover -s plugins/tests -k LayoutFile`.
- [ ] **Step 3: Implement.** `[dev]` names must be a saved task or in `known`. The writer works on lines: an owned block starts at a line that is exactly `[[task]]` or `[dev]` (ignoring surrounding spaces and a trailing comment) and runs until the next table header line or the end; strip trailing blank lines left behind; append one blank line, the `[[task]]` blocks, then `[dev]`.
- [ ] **Step 4: Run.** All plugin tests pass, including the existing Project Layouts ones.
- [ ] **Step 5: Stage** `plugins/project-layout/layoutfile.py plugins/project-layout/main.py plugins/tests/test_builtins.py`.

---

### Task 4: Plugin: discovery (`discover.py`)

**Files:**
- Create: `plugins/project-layout/discover.py`
- Test: `plugins/tests/test_builtins.py` (`DiscoverTests`, building fixture folders in `tempfile.mkdtemp()`)

**Interfaces:**
- Consumes: `Task` from Task 3.
- Produces: `discover(root: str) -> list[Task]`. Names are unique (prefix with the source on a clash, e.g. `make:test`). Ordered by source as below, then file order. Never raises: an unreadable or malformed file is skipped. Only `root` and its direct subfolders are searched (e.g. `frontend/package.json`, with `cwd` set to that subfolder and the name prefixed `frontend:`).

| Source | Detect | Task name → run |
|---|---|---|
| `compose` | `compose.yaml`, `compose.yml`, `docker-compose.yml`, `docker-compose.yaml` | each service → `docker compose up <svc>`, `stop = "docker compose stop <svc>"` |
| `npm` | `package.json` `scripts` | script → `<pm> run <script>`; `<pm>` from lockfile: `pnpm-lock.yaml` pnpm, `yarn.lock` yarn, `bun.lockb`/`bun.lock` bun, else npm |
| `composer` | `composer.json` `scripts` | script → `composer run <script>` |
| `artisan` | `artisan` file | `serve` → `php artisan serve` |
| `make` | `Makefile`/`makefile` | targets matching `^[A-Za-z0-9][\w.-]*:` not starting with `.` → `make <t>` |
| `just` | `justfile`/`.justfile` | recipes matching `^@?([A-Za-z0-9_-]+)[^:=]*:(?!=)` → `just <r>` |
| `procfile` | `Procfile` | `name: command` → command as-is |
| `cargo` | `Cargo.toml` | `run` → `cargo run`, `test` → `cargo test` |

Compose services are read without a YAML library: lines under a top-level `services:` key, indented exactly one level, of the form `<name>:`.

- [ ] **Step 1: Failing tests**: `test_finds_each_source` (one fixture per row, assert `(name, run)` pairs exactly); `test_package_manager_follows_the_lockfile`; `test_subfolder_tasks_are_prefixed_and_run_there`; `test_name_clashes_get_the_source_prefix`; `test_broken_files_are_skipped` (invalid JSON `package.json` → no npm tasks, others still found); `test_empty_folder_finds_nothing`.
- [ ] **Step 2: Run and see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Pass.
- [ ] **Step 5: Stage.**

---

### Task 5: Plugin: the checklist pane (`checklist.py`)

A plugin pane (as Pomodoro draws its timer) for ticking the dev environment's tasks.

**Files:**
- Create: `plugins/project-layout/checklist.py`
- Test: `plugins/tests/test_builtins.py` (`ChecklistTests`)

**Interfaces:**
- Consumes: `Task`.
- Produces: `class Checklist`:
  - `__init__(self, tasks: list[Task], ticked: set[str])`
  - `key(self, key: str) -> str | None`: keys by name as `@on_key` delivers them. `up`/`k` and `down`/`j` move; `space` toggles; `a` returns `"add"`; `enter` returns `"start"`; `escape`/`q` returns `"cancel"`; anything else `None`.
  - `add(self, task: Task) -> None`: appends it, ticked, and moves the cursor to it.
  - `chosen(self) -> list[str]`: ticked names in list order.
  - `render(self, cols: int, rows: int) -> list[str]`: lines with ANSI colours; a heading ("Choose your dev environment"), one row per task (`[x] name  run`, truncated to `cols`, cursor row highlighted), and a footer hint (`Space tick · a add task · Enter start · Esc cancel`). Scrolls to keep the cursor visible when there are more tasks than rows. With no tasks, says "Nothing to run found here. Press a to add a task." No line wider than `cols`.

- [ ] **Step 1: Failing tests**: `test_moves_ticks_and_answers` (keys sequence → `chosen()` and return values); `test_added_tasks_are_ticked`; `test_renders_within_the_pane` (strip ANSI; every line ≤ cols; cursor row visible after moving past `rows`); `test_empty_list_explains_itself`.
- [ ] **Step 2: Run, fail. Step 3: Implement. Step 4: Run, pass.**
- [ ] **Step 5: Stage.**

---

### Task 6: Plugin: opening the project and running tasks (`main.py`, `grid.py`)

**Files:**
- Create: `plugins/project-layout/grid.py`
- Modify: `plugins/project-layout/main.py`, `plugins/project-layout/plugin.toml`
- Modify: `src/plugins.rs:216-224` (`BUILTINS`: title "Projects & Tasks", files `["plugin.toml", "main.py", "layoutfile.py", "discover.py", "checklist.py", "grid.py"]`)
- Test: `plugins/tests/test_builtins.py` (`GridTests`, `RunnerTests`)

**Interfaces:**
- Consumes: Tasks 1–5 (`Pane.set_pane_style`, `Plugin.prompt`, `layoutfile.*`, `discover`, `Checklist`).
- Produces:
  - `grid.py`: `MAX_PER_TAB = 6`; `grid_plan(n: int) -> list[tuple[int, str, int, float]]`: for 1 ≤ n ≤ 6, one `(task, direction, of, size)` per task after task 0, in creation order; `of` is an already-created task; `size` is the new pane's share for the `split` call. Row-major: `rows = 1 if n <= 2 else 2`, `cols = ceil(n / rows)`; top row task `c` splits task `c-1` `"right"` with size `(cols-c)/(cols-c+1)`, bottom row task `cols+c` splits task `c` `"down"` with size `0.5`.
  - `main.py`: `Runner` holding the live state, with no Lumen calls in its decisions so it can be tested:
    - `runs: dict[str, list[int]]` project root → pane ids of its open dev tab(s); `tasks_by_pane: dict[int, Task]`; `one_offs: dict[int, str]` pane → command.
    - `failed(self, pane: int, exit: int) -> tuple[str, str] | None`: on `command-finished` for a task pane, the `(label colour, tab badge)` to show: exit 0 → `("green", None)`, non-zero → `("red", "N failed")` counting this tab's failures; `None` for panes it doesn't track.
    - `restart_steps(self, integrated: bool) -> list[tuple[str, float]]`: `[("ctrl-c", 5.0), ("ctrl-c", 3.0)]` with shell integration (wait up to that long for `command-finished`), `[("ctrl-c", 1.0)]` without (just wait). After the steps, rerun; if the task's still running after them, notify "Couldn't stop <name>" and don't rerun.
  - Commands: **Open Project** (`keys` ctrl-shift-l / cmd-shift-l, `id="open_layout"` so existing bindings keep working) and **Restart Task** (ctrl-alt-r / cmd-alt-r).
  - `plugin.toml`: description "Open a project's dev environment and run its tasks (Ctrl+Shift+L, ⌘⇧L)", version `2.0.0`, `permissions = ["panes", "input"]`.

**Open Project flow** (in `main.py`):
1. No `tomllib` / no cwd: notify as today.
2. `root = project_root(cwd)`, `path = find_layout(cwd)`, `found = discover(root)`, `layout = load(path, known=frozenset(t.name for t in found))` if there's a file.
3. If `runs[root]` has panes that still exist (check `app.panes()`), focus that tab and stop.
4. If there's no file, or the file has no `[dev]` and no `[[pane]]`: open the checklist (Task 5) as a plugin-pane split under the focused pane sized to fit (`size` = rows needed, at most half). On `"add"`: `prompt` for the command, then the name (initial: the command's first word), check both with `typeable`, `add` to the checklist. On `"start"`: `write_owned(path or join(root, NAME), saved + added tasks + chosen discovered tasks, chosen)`, close the checklist, continue at 6 without asking again (the user just chose these). On `"cancel"`: close it, nothing written.
5. Otherwise confirm, as today, listing every command from `[[pane]]` and from the dev tasks. Skip the question when the exact same command list was approved before for this file: keep `sha256` of `path + "\0" + commands` in `LUMEN_PLUGIN_DATA/approved.json`.
6. Open `[[pane]]` tabs as today. If there's a dev environment: for each chunk of `MAX_PER_TAB` tasks, open a tab (title = `title` or the folder name, `" 2"` etc. for later chunks), split per `grid_plan`, `run` each task in its `cwd`, `set_pane_style(label=task.name)`, `set_tab_style(badge=f"{n} running", color=None)`; record in `runs`/`tasks_by_pane`.
7. Events: subscribe to `command-finished` and `pane-closed`. `command-finished` on a task pane → `failed` → `set_pane_style(color=…)` and tab badge (red when any failed, cleared back to "N running" when none). `pane-closed` on a task pane with `stop` → run `stop` in its `cwd` with `subprocess` (not typed anywhere), then forget the pane.

**Restart Task:** focused pane not in `tasks_by_pane` or `one_offs` → do nothing (notify "This split isn't a task."). Otherwise follow `restart_steps`, then `pane.run(task.run)` and clear its colour.

- [ ] **Step 1: Failing tests** in `GridTests`: exact `grid_plan` output for n = 1…6, including `grid_plan(4) == [(1, "right", 0, 0.5), (2, "down", 0, 0.5), (3, "down", 1, 0.5)]` and `grid_plan(5) == [(1, "right", 0, 2/3), (2, "right", 1, 0.5), (3, "down", 0, 0.5), (4, "down", 1, 0.5)]`. In `RunnerTests`: `test_failures_mark_red_and_count`, `test_success_marks_green`, `test_untracked_panes_are_ignored`, `test_restart_without_shell_integration_just_waits`.
- [ ] **Step 2: Run, fail. Step 3: Implement** `grid.py`, `Runner`, the two commands, events, `plugin.toml`, `BUILTINS`.
- [ ] **Step 4: Run** all three test commands and `cargo build`. Pass.
- [ ] **Step 5: Stage.**

---

### Task 7: Plugin: ad hoc and single tasks, editing, docs

**Files:**
- Modify: `plugins/project-layout/main.py`, `docs/builtin-plugins.md`, `docs/keyboard.md` (if it lists plugin keys)
- Test: `plugins/tests/test_builtins.py`

**Interfaces:**
- Consumes: everything above.
- Produces commands (palette; only Run Task has a key):
  - **Run Task** (ctrl-shift-k / cmd-shift-k): pick from saved + discovered tasks (label = name, detail = command); run in a split `"right"` of the focused pane, in the task's `cwd`, labelled; tracked in `tasks_by_pane`, so Restart and failure marks work.
  - **Add Task**: `prompt` command, then name; refuse a duplicate name or non-`typeable` text with a notification; `write_owned` with it added (dev unchanged).
  - **Run Command…**: `prompt` a command; split `"right"` of the focused pane in its cwd, run it, label it with the command (cut to 32 characters); tracked in `one_offs`.
  - **Save as Task**: focused pane must be in `one_offs` (else notify "Run a command with Run Command… first."); `prompt` the name, add as in Add Task, and relabel the split with the name.
  - **Edit Dev Environment**: the checklist with the current `dev` ticked; on start, `write_owned` and notify "Saved. Ctrl+Shift+L opens it." (⌘⇧L on macOS); nothing is started.
- Helper: `add_task(path: str | None, root: str, tasks: list[Task], name: str, run: str) -> list[Task]` raising `ValueError("…")` for a duplicate or untypeable entry.

- [ ] **Step 1: Failing tests**: `test_add_task_refuses_duplicates_and_control_characters`; `test_add_task_creates_the_file_in_the_root` (no file yet → file in `root` with just that task, no `[dev]`).
- [ ] **Step 2: Run, fail. Step 3: Implement** the helper and the five commands.
- [ ] **Step 4: Docs.** Replace the Project Layouts row in `docs/builtin-plugins.md` with **Projects & Tasks** (`project-layout`): what Open Project does, the commands and keys, permissions `panes`, `input`. Document the file format (as in Task 3) in `main.py`'s docstring and link it from the row, as today.
- [ ] **Step 5: Run** all tests. Pass.
- [ ] **Step 6: Stage.**

---

### Task 8: End-to-end check in a real Lumen

- [ ] **Step 1:** `./install.sh` (or `cargo run`), turn on Projects & Tasks in the Plugin Gallery; Lumen asks again because the permissions changed.
- [ ] **Step 2:** In a scratch project with `compose.yaml` (one service), `package.json` (two `watch` scripts) and `artisan` (a stub): Ctrl+Shift+L shows the checklist; tick four; Enter. Expected: one new tab, 2×2 grid, each split labelled, badge "4 running", `.lumen-layout.toml` written with `[dev]`.
- [ ] **Step 3:** Make one task exit non-zero. Expected: its split marked red, badge red "1 failed". Ctrl+Alt+R in it reruns it and clears the mark.
- [ ] **Step 4:** Ctrl+Shift+L again. Expected: jumps to the open tab. Close the tab; `docker compose ps` shows the service stopped. Ctrl+Shift+L again: no question (same commands), the grid opens.
- [ ] **Step 5:** Run Command… `sleep 1000`, then Save as Task as `nap`; Add Task `echo hi`/`hi`; Run Task shows both. Edit Dev Environment lists them.
- [ ] **Step 6:** An old-style layout file (only `[[pane]]`) opens exactly as before, with no checklist.
- [ ] **Step 7:** On macOS if available: the same with ⌘ keys.
- [ ] **Step 8:** Report results to the user; nothing is committed.
