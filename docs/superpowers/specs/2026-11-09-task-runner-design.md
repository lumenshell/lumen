# Task Runner: design

Date: 2026-11-09. Branch: `feat/task-runner-plugin`.

## Why

Lumen started from one annoyance: to work on a web project, Sean opened four
terminal windows by hand every time, one each for the Docker database, the JS
build, the SCSS build and the app server. The task runner fixes exactly that.
You open a project, press one key, and the whole dev environment comes up.

It's also the first piece of a larger direction: Lumen as an opinionated
workspace that's already set up for a day of work, with the all-decided-for-you
feeling (inspired by Omarchy). So it has to feel finished out of the box, with
no config to write before it's useful.

## Goals

1. **One key starts the dev environment.** It opens a new tab with each chosen
   task in its own split, laid out as a grid.
2. **No config needed to start.** Lumen discovers runnable tasks from the
   project's own files.
3. **Ad hoc tasks, just as important as discovered ones:**
   - **Saved custom tasks:** any command, given a name, saved with the project.
     They behave exactly like discovered tasks.
   - **One-offs:** type a command, it runs in a new split, nothing saved. It
     can be turned into a saved task afterwards.
4. **Run a single task** (e.g. `npm run test`) from the palette, apart from
   the dev environment.
5. **Restart one task** with a key, in its own split.
6. **Shareable:** the choice lives in a file in the project, so anyone who
   clones it gets the same one-key start.

## Non-goals

- Starting anything automatically when you open a project or Lumen. Nothing
  runs until you press the key.
- A process manager: no supervision, auto-restart on crash, health checks or
  dependency ordering between tasks.
- Spotting errors in a task's output (e.g. a watcher that prints an error and
  keeps running). See "Failure" below.

## Anti-goals

- It must not feel bolted on. It should look and behave like the rest of
  Lumen: palette commands, the existing picker and confirmation, tab badges in
  the same style as the Pomodoro timer.
- Two plugins sharing one file and one job would be the bolted-on feeling.
  Hence the merge with Project Layouts below.
- Never run a command from a file the user hasn't seen. A cloned repository can
  ship this file.

## Decided with Sean

- **Trigger:** a key only. Never on open.
- **Where the choice is saved:** a file inside the project, `.lumen-layout.toml`
  (the file Project Layouts already reads). Meant to be committed.
- **Layout of the running environment:** one new tab, each task in its own
  split, arranged as a grid (2 tasks side by side, 4 as 2×2, and so on). Split
  titles are the task names. Your own shell stays in the tab you were in; the
  dev tab holds only tasks. (Picked from mockups: grid over one-tab-per-task and
  over a strip under your shell.)
- **Restart:** a key, with focus in a task's split, restarts that task in the
  same split. No new pane, no layout change.
- **Stop:** closing the dev tab stops everything. No separate stop command.
- **Ad hoc:** both saved custom tasks and throwaway one-offs.
- **Core change allowed:** add per-split titles and markers to Lumen's core
  (Rust), first, rather than faking labels with extra panes. See "Core:
  per-split titles and markers".
- **One key, one idea ("open this project"):** `Ctrl+Shift+L` / `⌘⇧L` opens
  whatever the project's `.lumen-layout.toml` describes: its `[[pane]]` layout
  tabs if it has any, plus the dev tab if it has a dev environment. With no
  file, or a file with no dev environment yet, it shows the checklist first
  (and, for a file with `[[pane]]` entries, still opens those as today).

## How it works

### First press in a project (no saved dev environment)

1. Find the project root: the nearest folder at or above the focused pane's
   folder with a `.lumen-layout.toml`, else the git root, else the pane's folder.
2. Discover tasks (below) and add any saved custom tasks.
3. Show them as a checklist; the user ticks the ones that make up the dev
   environment. An "Add task…" entry is available here too.
4. Write `.lumen-layout.toml` in the project root with that set.
5. Start them as described in "Running".

### Later presses

1. Find the file and read the dev environment from it.
2. Show every command that will run and ask, as Project Layouts does today.
   (Builder's call whether to remember approval of an unchanged file; never
   skip it for a changed one.)
3. Start them. If the dev tab for this project is already open, jump to it
   instead of starting a second copy.

A second command, **Edit Dev Environment** (name is the builder's call), reopens
the checklist with the current set ticked and rewrites the file.

### Discovery

Find what a project can run, fresh on every use (discovered tasks are not
written to the file unless they're part of the dev environment). At least:

- `docker-compose.yml` / `compose.yaml`: each service (`docker compose up <svc>`)
- `package.json` scripts, using the project's package manager (npm, pnpm, yarn
  or bun, from the lockfile)
- `Makefile` targets, `justfile` recipes
- `composer.json` scripts; `php artisan serve` when `artisan` exists
- `Procfile` entries
- `Cargo.toml` (`cargo run`, `cargo test`, `cargo watch` if installed)

Which others to add, and how deep to look (e.g. `frontend/package.json` in a
subfolder), is the builder's call. Keep it fast: discovery runs on a key press.

### Ad hoc tasks

- **Add Task:** asks for a name and a command, saves it to `.lumen-layout.toml`
  as a task. It then appears everywhere discovered tasks do (the run-one picker,
  the checklist).
- **Run Command…:** asks for a command and runs it in a new split of the
  current tab. Nothing is saved.
- **Save as Task:** with focus in a one-off's split, saves that command as a
  task (asks for a name).

### Run a single task

**Run Task** lists all discovered and saved tasks in the picker and runs the
chosen one in a new split. Restart works on it too.

### Running

- Open one new tab, titled after the project, and split it into a grid, one
  task per split, each split titled with the task's name, in the task's folder.
- Badge the tab with the number running (`4 running`), using `set-tab-style`.
- **Failure:** when a task exits with a failure, its split is marked and the
  tab badge turns red, so it's visible from another tab. The plugin can learn a
  command's exit through shell integration's `command-finished` event, if tasks
  run through the shell (check how `run=` starts a command). Spotting errors in
  output is out of scope for now.
- **Restart key** (in a task split): stop the task (Ctrl+C, then something
  firmer if it hasn't exited after a few seconds) and run its command again.
  Outside a task split the key does nothing extra.
- **Closing the tab stops everything** and should leave nothing behind. Check
  in particular that `docker compose up` stops its containers when its pane is
  closed; if it doesn't, closing must still clean up (e.g. run
  `docker compose stop <svc>` for compose tasks).
- **After a relaunch or plugin restart** (builder's call): Lumen restores tabs as
  idle shells, so a restored dev tab has nothing running, and the plugin's
  memory is gone. A dev tab only counts as "already running" if the plugin
  started it in this run (or can confirm it from state kept in
  `LUMEN_PLUGIN_DATA` and the panes still exist and are busy). Otherwise the
  key opens a fresh dev tab; it never sends you to a dead one, and never
  restarts tasks on its own, since nothing runs until you press the key.
- Many tasks: past about six, the grid gets cramped. How to handle it (wrap to
  a second tab, smaller splits) is the builder's call.

## The file

Today `.lumen-layout.toml` is a list of panes (`title`, `[[pane]]` with
`split`, `of`, `cwd`, `run`; see `plugins/project-layout/main.py`). It gains:

- a list of **tasks**: saved custom tasks, and discovered tasks that are part
  of the dev environment, each with at least a name, a command and an optional
  folder relative to the file;
- the **dev environment**: which tasks to start together.

Existing layout files must keep working unchanged. The exact schema is the
builder's call, but it should be easy to read and edit by hand, and it goes
through the same checks Project Layouts applies today (no control or invisible
characters in commands).

**Writing the file keeps what the user wrote** (builder's call). Layout files
are hand-written and commented, so the plugin never regenerates the whole file.
It owns only its own sections (the tasks and the dev environment) and rewrites
just those, leaving comments, formatting and `[[pane]]` entries byte-for-byte
as they were. The schema should make that easy, e.g. top-level tables the
plugin owns, kept together at the end of the file.

## Core: per-split titles and markers

Today `set-title` and `set-tab-style` act on a whole tab, and splits are plain
frames with no label. The grid needs each split to show its task's name and a
failed task to be marked. So, built first, in Rust:

- A way for a plugin (and the socket API) to give one pane its own title and a
  marker/colour (e.g. failed), shown on that split when the tab has more than
  one pane. Exact API shape, how it's drawn, and whether it's kept in the saved
  session are the builder's call; it should match Lumen's existing look (tab
  badge colours) and be documented in `docs/scripting.md` and
  `docs/plugin-protocol.md`, with the Python helper in `lumen_plugin`.
- Useful beyond this plugin (e.g. Project Layouts panes, Build Status per
  split), so design it as a general feature, not a task-runner special case.

## Project Layouts

The task runner and Project Layouts become one built-in plugin; one job and one
file shouldn't be two plugins. Its key (`Ctrl+Shift+L` / `⌘⇧L`) opens the
project as described under "Decided with Sean", and existing layout files must
keep opening exactly as they do today. Whether
the merged plugin keeps the `project-layout` name, or gets a new one with
`project-layout` kept as an alias for people who already turned it on, is the
builder's call; nobody's config should break.

## Constraints

- A built-in plugin in `plugins/`, in Python, using `lumen_plugin`, registered
  in `BUILTINS` (`src/plugins.rs`), off until turned on in the Plugin Gallery,
  like the others. Linux and macOS.
- `lumen_plugin` needs only Python 3.8 and the standard library; reading TOML
  needs `tomllib` (3.11+), as Project Layouts already requires. Writing TOML has
  no standard library support, so the plugin writes it itself.
- Permissions: declare only what's needed (`panes` for tabs and splits,
  `input` for Ctrl+C and rerun, maybe `screen`).
- Plugin commands get palette entries and default keys like the other built-ins,
  on both platforms. Key choices are the builder's call; avoid clashing with
  existing ones (`docs/keyboard.md`, the other built-ins).
- Docs: update `docs/builtin-plugins.md` and the layout file format; tests in
  `plugins/tests/`.

## Left to the builder

- **The checklist.** The picker (`show-picker`) chooses one item. Ticking
  several needs either a small plugin pane drawn as a checklist (as the
  Pomodoro timer and the todo example draw theirs) or a picker that reopens
  with tick marks and a "Start these" entry. Pick whichever feels more like
  Lumen.
- Command names and keys.
- Exact file schema, within the rules above.
- Grid arrangement for odd counts and large counts.
- Which discovery sources beyond the list, and subfolder depth.
- Remembering approval of an unchanged file.
