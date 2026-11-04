//! A single terminal pane: renders the alacritty grid with GPUI and feeds input to the PTY.

use std::cell::{Cell as StdCell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::Event;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{AppContext, Entity, Reservation,
    App, Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, FontStyle,
    ElementInputHandler, EntityInputHandler, FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, Modifiers, UTF16Selection,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    DispatchPhase, ParentElement, Pixels, Point, Render, Rgba, ScrollDelta, ScrollWheelEvent, ShapedLine,
    StatefulInteractiveElement, StrikethroughStyle, Styled, Subscription, Task,
    TextRun, UnderlineStyle, Window,
    canvas, div, fill, font, point, prelude::FluentBuilder, px, size,
};

use crate::boxdraw;
use crate::glyphs::{self, Glyph, GlyphCache};
use crate::links::{self, Link, LinkFinder};
use crate::search::Search;
use crate::shell_integration;
use crate::marks;
use crate::mouse::{self, Action as MouseAction, Button as ReportButton};
use crate::pty::{GridSize, Owner, Pty};
use crate::settings::{MONO_FONT, Settings, UI_FONT};
use crate::theme::{Theme, alpha, indexed};
use crate::vi::{self, Outcome, ViState};
use crate::{Copy, CopyLastOutput, Find, NextPrompt, Paste, PrevPrompt, ToggleSelectMode, input};

/// Half a blink: how long the cursor stays shown, then hidden.
const BLINK_INTERVAL: Duration = Duration::from_millis(530);

pub enum TerminalEvent {
    TitleChanged,
    /// A long command finished out of sight, or its badge was seen.
    Attention,
    Exited,
    Focused,
}

#[derive(Clone, Copy)]
struct GridLayout {
    origin: Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    cols: usize,
    rows: usize,
}

pub struct TerminalView {
    pty: Pty,
    focus: FocusHandle,
    pub title: String,
    /// The title when the program hasn't set one: the shell's name, or the
    /// plugin's for a plugin pane.
    base_title: String,
    layout: Rc<StdCell<Option<GridLayout>>>,
    selecting: bool,
    scroll_acc: f32,
    /// Fractional wheel notches not yet reported to a program in mouse mode.
    wheel_acc: f32,
    links: LinkFinder,
    /// Link under the pointer while the link modifier is held.
    hovered_link: Option<Link>,
    /// Last pointer position and modifiers over this pane.
    pointer: Option<(Point<Pixels>, Modifiers)>,
    /// Button whose press was reported to the program (mouse mode), so its
    /// drags and release go to the program too.
    reported_button: Option<ReportButton>,
    /// Cell of the last reported motion, to send one event per cell crossed.
    last_report_cell: Option<(usize, usize)>,
    /// Find bar state, while it's open.
    search: Option<Search>,
    /// Pending rescan after new output, so heavy output rescans a few times a
    /// second rather than on every chunk.
    search_rescan: Option<Task<()>>,
    /// Exit status of each command, by prompt link id (shell integration).
    exits: Rc<std::collections::HashMap<String, Option<i32>>>,
    marks_rescan: Option<Task<()>>,
    /// Prompt last jumped to, highlighted until you type.
    jump_target: Option<String>,
    /// Keyboard select mode: keys typed toward a count or `gg`.
    vi: ViState,
    /// The last find, kept for `n`/`N` in select mode after the bar closes.
    last_search: Option<Search>,
    /// The find bar was opened with `?`: `n` goes up.
    search_backward: bool,
    /// Keyboard focus is in this pane, and the window is the active one.
    focused: bool,
    window_active: bool,
    /// The command now running, read off the screen once it started.
    running_command: Option<String>,
    /// A long command finished while you weren't here: whether it succeeded.
    /// Shown as a dot on the tab until the pane is focused.
    pub attention: Option<bool>,
    /// The blinking cursor is in its visible half. Stays true while the
    /// cursor isn't blinking.
    blink_on: bool,
    /// Text an input method is composing (CJK, dead keys), drawn at the
    /// cursor until it's committed.
    marked: Option<String>,
    /// Last key typed here: the cursor stays solid for a moment after.
    last_typed: Instant,
    _blink: Task<()>,
    _events: Task<()>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn default_title() -> String {
    std::env::var("SHELL")
        .ok()
        .and_then(|s| s.rsplit('/').next().map(str::to_string))
        .unwrap_or_else(|| "shell".into())
}

/// A shell started before its pane exists (see [`prespawn`]).
struct Prespawned {
    reservation: Reservation<TerminalView>,
    cwd: Option<PathBuf>,
    pty: Pty,
    rx: UnboundedReceiver<Event>,
}

thread_local! {
    static PRESPAWNED: RefCell<Vec<Prespawned>> = const { RefCell::new(Vec::new()) };
}

/// Start shells for the panes about to open in `cwds`, ahead of their
/// windows: a shell takes a while to start (100 ms or more with a busy
/// `.bashrc`), and this way it does so while the GPU is still being set up,
/// so the prompt is there when the window first draws. The panes pick them
/// up as they open; [`discard_prespawned`] ends any left over.
///
/// `panes` gives each one's directory and expected size, so what the shell
/// prints while starting fits; the pane corrects the size once laid out.
pub fn prespawn(panes: Vec<(Option<PathBuf>, GridSize)>, cx: &mut App) {
    let settings = Settings::get(cx);
    let (shell, integrate, config) = (settings.shell.clone(), settings.shell_integration, term_config(settings));
    for (cwd, size) in panes {
        let reservation = cx.reserve_entity::<TerminalView>();
        let pane = reservation.entity_id().as_u64();
        // On failure the pane tries again itself when it opens.
        if let Ok((pty, rx)) = Pty::spawn(cwd.clone(), shell.as_deref(), integrate, pane, config.clone(), size) {
            PRESPAWNED.with_borrow_mut(|p| p.push(Prespawned { reservation, cwd, pty, rx }));
        }
    }
}

/// End the shells [`prespawn`] started that no pane took.
pub fn discard_prespawned() {
    PRESPAWNED.with_borrow_mut(Vec::clear);
}

impl TerminalView {
    /// A pane running your shell in `cwd`, or, with an `owner`, a plugin
    /// pane that the plugin draws. Uses a shell from [`prespawn`] if one is
    /// waiting for `cwd`.
    pub fn open<T>(cwd: Option<PathBuf>, owner: Option<Owner>, window: &mut Window, cx: &mut Context<T>) -> Entity<Self> {
        let waiting = owner.is_none().then(|| {
            PRESPAWNED.with_borrow_mut(|p| p.iter().position(|s| s.cwd == cwd).map(|i| p.remove(i)))
        });
        match waiting.flatten() {
            Some(s) => cx.insert_entity(s.reservation, |cx| Self::with_pty(s.pty, s.rx, default_title(), window, cx)),
            None => cx.new(|cx| Self::new(cwd, owner, window, cx)),
        }
    }

    fn new(cwd: Option<PathBuf>, owner: Option<Owner>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::get(cx);
        let config = term_config(settings);
        let pane = cx.entity_id().as_u64();
        let base_title = owner.as_ref().map_or_else(default_title, |o| o.name.clone());
        let (pty, rx) = match owner {
            Some(owner) => Pty::plugin(owner, pane, config),
            None => Pty::spawn(cwd, settings.shell.as_deref(), settings.shell_integration, pane, config, GridSize::DEFAULT)
                .expect("failed to spawn shell"),
        };
        Self::with_pty(pty, rx, base_title, window, cx)
    }

    fn with_pty(
        pty: Pty,
        mut rx: UnboundedReceiver<Event>,
        base_title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut applied = term_config(Settings::get(cx));
        let focus = cx.focus_handle();

        let events = cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let mut batch = vec![event];
                #[allow(deprecated)]
                while let Ok(Some(e)) = rx.try_next() {
                    batch.push(e);
                }
                let ok = this.update(cx, |this, cx| {
                    for e in batch {
                        this.handle_event(e, cx);
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
        });

        let blink = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK_INTERVAL).await;
                if this.update(cx, |this, cx| this.blink_tick(cx)).is_err() {
                    break;
                }
            }
        });

        let subs = vec![
            cx.on_focus(&focus, window, |this, _, cx| {
                this.report_focus(true);
                this.focused = true;
                this.seen(cx);
                cx.emit(TerminalEvent::Focused)
            }),
            cx.on_blur(&focus, window, |this, _, _| {
                this.report_focus(false);
                this.focused = false;
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                this.window_active = window.is_window_active();
                if this.window_active && this.focused {
                    this.seen(cx);
                }
            }),
            // Apply scrollback and cursor changes from the config file live.
            cx.observe_global::<Settings>(move |this, cx| {
                let config = term_config(Settings::get(cx));
                if !same_term_config(&applied, &config) {
                    applied = config.clone();
                    this.pty.term.lock().set_options(config);
                }
            }),
        ];

        Self {
            pty,
            focus,
            title: base_title.clone(),
            base_title,
            layout: Rc::new(StdCell::new(None)),
            selecting: false,
            scroll_acc: 0.0,
            wheel_acc: 0.0,
            links: LinkFinder::new(),
            hovered_link: None,
            pointer: None,
            reported_button: None,
            last_report_cell: None,
            search: None,
            search_rescan: None,
            exits: Rc::default(),
            marks_rescan: None,
            jump_target: None,
            vi: ViState::default(),
            last_search: None,
            search_backward: false,
            focused: false,
            window_active: window.is_window_active(),
            running_command: None,
            attention: None,
            blink_on: true,
            marked: None,
            last_typed: Instant::now(),
            _blink: blink,
            _events: events,
            _subs: subs,
        }
    }

    pub fn cwd(&self) -> Option<PathBuf> {
        self.pty.cwd()
    }

    /// Drawn by a plugin rather than running a program.
    pub fn is_plugin_pane(&self) -> bool {
        self.pty.owner().is_some()
    }

    /// The connection drawing this pane, if it's a plugin pane.
    pub fn owner_conn(&self) -> Option<u64> {
        self.pty.owner().map(|o| o.conn)
    }

    /// Show `text` in this plugin pane, escape sequences and all.
    pub fn feed(&mut self, text: &str) {
        self.pty.feed(text.as_bytes());
    }

    /// Clear the tab badge once you've looked at the pane.
    fn seen(&mut self, cx: &mut Context<Self>) {
        if self.attention.take().is_some() {
            cx.emit(TerminalEvent::Attention);
        }
    }

    /// Note the command that's running, and tell you about long ones that
    /// finished while you were elsewhere (shell integration).
    fn check_commands(&mut self, cx: &mut Context<Self>) {
        let (running, finished) = {
            let mut shared = self.pty.shell_state.lock().unwrap();
            if !shared.active {
                return;
            }
            (shared.running_since.is_some(), std::mem::take(&mut shared.finished))
        };
        // Name each finished command: the one noted while it ran, else read
        // back from the screen (skipping the prompt of one already running).
        let recent = if finished.is_empty() {
            Vec::new()
        } else {
            let term = self.pty.term.lock();
            marks::recent_commands(&term, &marks::prompts(&term), finished.len(), usize::from(running))
        };
        let pane = cx.entity_id().as_u64();
        for (i, done) in finished.into_iter().enumerate() {
            let noted = if i == 0 { self.running_command.take() } else { None };
            let command = noted.or_else(|| recent[i].clone());
            crate::api::emit(
                "command-finished",
                serde_json::json!({
                    "pane": pane,
                    "exit": done.exit,
                    "duration_ms": done.duration.as_millis() as u64,
                    "command": command,
                }),
            );
            let after = Settings::get(cx).notify_after;
            if after == 0 || done.duration < std::time::Duration::from_secs(after) || (self.focused && self.window_active) {
                continue;
            }
            let ok = done.exit.is_none_or(|c| c == 0);
            if !self.focused {
                // Red wins over green until the pane is visited.
                self.attention = Some(ok && self.attention != Some(false));
                cx.emit(TerminalEvent::Attention);
            }
            let (title, body) = notification_text(command.as_deref(), done.exit, done.duration, self.cwd());
            std::thread::spawn(move || {
                if let Err(e) = crate::api::notify(&title, &body) {
                    eprintln!("lumen: {e}");
                }
            });
        }
        if running && self.running_command.is_none() {
            let term = self.pty.term.lock();
            self.running_command = marks::last_command(&term, &marks::prompts(&term));
        }
    }

    /// Type `text` into the pane, as if pasted without bracketing.
    pub fn send_text(&self, text: &str) {
        self.pty.write(text.as_bytes().to_vec());
    }

    /// Columns and rows of the grid.
    pub fn grid_size(&self) -> (usize, usize) {
        (self.pty.size.cols as usize, self.pty.size.rows as usize)
    }

    /// Exit status of the last command that finished (shell integration).
    pub fn last_exit(&self) -> Option<i32> {
        marks::prompts(&self.pty.term.lock()).last().and_then(|p| p.exit_before)
    }

    /// Text of the screen, or with `lines`, of the last `lines` lines of
    /// history and screen together, counting up from the last line with text
    /// on it. Trailing blanks are trimmed.
    pub fn screen_text(&self, lines: Option<usize>) -> String {
        use alacritty_terminal::grid::Dimensions as _;
        let term = self.pty.term.lock();
        let grid = term.grid();
        let blank = |line: Line| grid[line].into_iter().all(|c| c.c == ' ' || c.c == '\0');
        let bottom = (0..term.screen_lines() as i32)
            .rev()
            .map(Line)
            .find(|&l| !blank(l))
            .unwrap_or(Line(0));
        let top = match lines {
            Some(n) => Line((bottom.0 - n as i32 + 1).max(term.topmost_line().0)),
            None => Line(0),
        };
        let text = term.bounds_to_string(GridPoint::new(top, Column(0)), GridPoint::new(bottom, term.last_column()));
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        lines.join("\n").trim_end().to_string()
    }

    fn handle_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Wakeup => {
                self.check_commands(cx);
                self.schedule_search_rescan(cx);
                self.schedule_marks_rescan(cx);
                // Output may have moved or replaced the text under the pointer.
                if self.hovered_link.is_some() {
                    self.update_hovered_link(cx);
                }
                cx.notify()
            }
            // Shells set the title at every prompt; only a change is news.
            Event::Title(title) if title != self.title => {
                self.title = title;
                cx.emit(TerminalEvent::TitleChanged);
            }
            Event::ResetTitle if self.title != self.base_title => {
                self.title = self.base_title.clone();
                cx.emit(TerminalEvent::TitleChanged);
            }
            Event::PtyWrite(s) => self.pty.write(s.into_bytes()),
            Event::ClipboardStore(_, s) => cx.write_to_clipboard(ClipboardItem::new_string(s)),
            // OSC 52 reads are ignored: any program (including one on a remote
            // host over SSH) could otherwise silently read the clipboard.
            Event::ClipboardLoad(..) => {}
            Event::ColorRequest(index, format) => {
                let theme = Settings::get(cx).theme().clone();
                let color = match index {
                    0..=255 => indexed(&theme, index as u8),
                    256 => theme.text,
                    257 => theme.pane,
                    _ => theme.cursor,
                };
                let rgba: Rgba = color.into();
                let rgb = Rgb {
                    r: (rgba.r * 255.0) as u8,
                    g: (rgba.g * 255.0) as u8,
                    b: (rgba.b * 255.0) as u8,
                };
                self.pty.write(format(rgb).into_bytes());
            }
            Event::TextAreaSizeRequest(format) => {
                self.pty.write(format(self.pty.window_size()).into_bytes())
            }
            Event::Bell => crate::api::emit("bell", serde_json::json!({ "pane": cx.entity_id().as_u64() })),
            Event::ChildExit(_) | Event::Exit => cx.emit(TerminalEvent::Exited),
            _ => {}
        }
    }

    /// Focus in/out reports (mode 1004), used by tmux `focus-events` and vim.
    fn report_focus(&self, focused: bool) {
        if self.pty.term.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
            self.pty.write(if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
        }
    }

    fn mode(&self) -> TermMode {
        *self.pty.term.lock().mode()
    }

    /// Does this event go to the program rather than to local selection?
    /// Holding Shift always keeps the mouse local, as in other terminals.
    fn reports_mouse(&self, mods: &Modifiers) -> bool {
        !mods.shift && mouse::reporting(self.mode())
    }

    /// 0-based viewport cell under `pos`, clamped to the grid so drags that
    /// leave the pane still report the nearest edge.
    fn viewport_cell(&self, pos: Point<Pixels>) -> Option<(usize, usize)> {
        let l = self.layout.get()?;
        let x = ((pos.x - l.origin.x) / l.cell_width).floor().max(0.0) as usize;
        let y = ((pos.y - l.origin.y) / l.line_height).floor().max(0.0) as usize;
        Some((x.min(l.cols.saturating_sub(1)), y.min(l.rows.saturating_sub(1))))
    }

    fn report_mouse(
        &mut self,
        button: ReportButton,
        action: MouseAction,
        pos: Point<Pixels>,
        mods: &Modifiers,
    ) {
        let Some((col, row)) = self.viewport_cell(pos) else { return };
        if action == MouseAction::Motion && self.last_report_cell == Some((col, row)) {
            return;
        }
        self.last_report_cell = Some((col, row));
        if let Some(bytes) = mouse::encode(self.mode(), button, action, mods, col, row) {
            self.pty.write(bytes);
        }
    }

    fn grid_point(&self, pos: Point<Pixels>) -> Option<(GridPoint, Side)> {
        let l = self.layout.get()?;
        let x = (pos.x - l.origin.x) / l.cell_width;
        let y = (pos.y - l.origin.y) / l.line_height;
        let col = (x.floor().max(0.0) as usize).min(l.cols.saturating_sub(1));
        let row = (y.floor().max(0.0) as usize).min(l.rows.saturating_sub(1));
        let side = if x - x.floor() < 0.5 { Side::Left } else { Side::Right };
        let offset = self.pty.term.lock().grid().display_offset() as i32;
        Some((GridPoint::new(Line(row as i32 - offset), Column(col)), side))
    }

    /// Like `grid_point`, but None when the position is outside the grid.
    fn grid_point_inside(&self, pos: Point<Pixels>) -> Option<GridPoint> {
        let l = self.layout.get()?;
        let inside = pos.x >= l.origin.x
            && pos.y >= l.origin.y
            && pos.x < l.origin.x + l.cell_width * l.cols as f32
            && pos.y < l.origin.y + l.line_height * l.rows as f32;
        inside.then(|| self.grid_point(pos)).flatten().map(|(p, _)| p)
    }

    fn link_at(&mut self, pos: Point<Pixels>) -> Option<Link> {
        let point = self.grid_point_inside(pos)?;
        let term = self.pty.term.lock();
        self.links.link_at(&term, point)
    }

    fn update_hovered_link(&mut self, cx: &mut Context<Self>) {
        let link = match self.pointer {
            Some((pos, mods)) if link_modifier(&mods) && !self.selecting => self.link_at(pos),
            _ => None,
        };
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
    }

    fn on_modifiers_changed(
        &mut self,
        ev: &ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((_, mods)) = &mut self.pointer {
            *mods = ev.modifiers;
            self.update_hovered_link(cx);
        }
    }

    fn on_hover(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if !*hovered {
            self.pointer = None;
            self.update_hovered_link(cx);
        }
    }

    /// Whether the cursor should blink now: this pane has focus in the active
    /// window, the setting allows it, and the cursor style (the default or the
    /// one a program chose) blinks. Select mode's cursor never blinks.
    fn blinks(&self, cx: &App) -> bool {
        if !(self.focused && self.window_active && Settings::get(cx).cursor_blink) {
            return false;
        }
        let term = self.pty.term.lock();
        term.cursor_style().blinking && !term.mode().contains(TermMode::VI)
    }

    fn blink_tick(&mut self, cx: &mut Context<Self>) {
        let on = !self.blinks(cx) || self.last_typed.elapsed() < BLINK_INTERVAL || !self.blink_on;
        if on != self.blink_on {
            self.blink_on = on;
            cx.notify();
        }
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.last_typed = Instant::now();
        self.blink_on = true;
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        if self.search.is_some() && self.search_key(ks, cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if vi::active(&self.pty.term.lock()) {
            let outcome = vi::key(&mut self.pty.term.lock(), &mut self.vi, ks);
            self.vi_outcome(outcome, cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if m.shift && !m.control && !m.alt && (ks.key == "pageup" || ks.key == "pagedown") {
            let scroll = if ks.key == "pageup" { Scroll::PageUp } else { Scroll::PageDown };
            self.pty.term.lock().scroll_display(scroll);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let mode = *self.pty.term.lock().mode();
        let event = if ev.is_held { input::KeyEvent::Repeat } else { input::KeyEvent::Press };
        if let Some(bytes) = input::encode(ks, mode, event) {
            self.jump_target = None;
            {
                let mut term = self.pty.term.lock();
                term.scroll_display(Scroll::Bottom);
                term.selection = None;
            }
            self.pty.write(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// Key releases, for programs that asked the kitty keyboard protocol to
    /// report them.
    fn on_key_up(&mut self, ev: &KeyUpEvent, _: &mut Window, _: &mut Context<Self>) {
        let mode = *self.pty.term.lock().mode();
        if !mode.contains(TermMode::REPORT_EVENT_TYPES) || self.search.is_some() {
            return;
        }
        if let Some(bytes) = input::encode(&ev.keystroke, mode, input::KeyEvent::Release) {
            self.pty.write(bytes);
        }
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.layout.get() else { return };
        // GPUI already reports a wheel notch as LINES_PER_NOTCH lines.
        const LINES_PER_NOTCH: f32 = 3.0;
        let lines = match ev.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => p.y / layout.line_height,
        };

        if self.reports_mouse(&ev.modifiers) {
            // Programs expect one wheel event per notch, as xterm sends.
            self.wheel_acc += lines / LINES_PER_NOTCH;
            let n = self.wheel_acc.trunc() as i32;
            self.wheel_acc -= n as f32;
            let button = if n > 0 { ReportButton::WheelUp } else { ReportButton::WheelDown };
            for _ in 0..n.unsigned_abs() {
                self.report_mouse(button, MouseAction::Press, ev.position, &ev.modifiers);
            }
            return;
        }

        self.scroll_acc += lines;
        let n = self.scroll_acc.trunc() as i32;
        if n == 0 {
            return;
        }
        self.scroll_acc -= n as f32;
        let mut term = self.pty.term.lock();
        let mode = *term.mode();
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            drop(term);
            let seq: &[u8] = if n > 0 { b"\x1bOA" } else { b"\x1bOB" };
            for _ in 0..n.unsigned_abs() {
                self.pty.write(seq.to_vec());
            }
        } else {
            term.scroll_display(Scroll::Delta(n));
        }
        cx.notify();
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        if ev.button == MouseButton::Left
            && link_modifier(&ev.modifiers)
            && let Some(link) = self.link_at(ev.position)
        {
            if links::openable(&link) {
                cx.open_url(&link.uri);
            }
            return;
        }
        if self.reports_mouse(&ev.modifiers)
            && let Some(button) = ReportButton::from_gpui(ev.button)
        {
            self.pty.term.lock().selection = None;
            self.reported_button = Some(button);
            self.last_report_cell = None;
            self.report_mouse(button, MouseAction::Press, ev.position, &ev.modifiers);
            cx.notify();
            return;
        }
        match ev.button {
            MouseButton::Left => self.start_selection(ev, cx),
            MouseButton::Middle => self.paste_primary(cx),
            _ => {}
        }
    }

    fn start_selection(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some((point, side)) = self.grid_point(ev.position) else { return };
        let ty = match ev.click_count {
            0 | 1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        let mut term = self.pty.term.lock();
        if vi::active(&term) {
            term.vi_goto_point(point);
        }
        match (&mut term.selection, ev.modifiers.shift) {
            (Some(sel), true) => sel.update(point, side),
            _ => term.selection = Some(Selection::new(ty, point, side)),
        }
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.pointer = Some((ev.position, ev.modifiers));
        self.update_hovered_link(cx);
        self.handle_motion(ev, cx);
    }

    /// Motion while hovering, or while a drag that started here is outside the pane.
    fn handle_motion(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let mode = self.mode();
        if let Some(button) = self.reported_button {
            if mouse::wants_motion(mode, true) {
                self.report_mouse(button, MouseAction::Motion, ev.position, &ev.modifiers);
            }
            return;
        }
        if ev.pressed_button.is_none()
            && !ev.modifiers.shift
            && mouse::wants_motion(mode, false)
        {
            self.report_mouse(ReportButton::None, MouseAction::Motion, ev.position, &ev.modifiers);
            return;
        }
        if !self.selecting || ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some((point, side)) = self.grid_point(ev.position) {
            if let Some(sel) = self.pty.term.lock().selection.as_mut() {
                sel.update(point, side);
            }
            cx.notify();
        }
    }

    /// Release over the pane or anywhere else in the window: a reported press
    /// always gets its release, and a selection always ends.
    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(button) = self.reported_button
            && ReportButton::from_gpui(ev.button) == Some(button)
        {
            self.reported_button = None;
            self.report_mouse(button, MouseAction::Release, ev.position, &ev.modifiers);
            cx.notify();
            return;
        }
        if ev.button != MouseButton::Left || !self.selecting {
            return;
        }
        self.selecting = false;
        let mut term = self.pty.term.lock();
        if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
            term.selection = None;
        }
        #[cfg(target_os = "linux")]
        if let Some(text) = term.selection_to_string() {
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
        cx.notify();
    }

    fn paste_primary(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "linux")]
        if let Some(text) = cx.read_from_primary().and_then(|c| c.text()) {
            self.paste_text(&text);
        }
        let _ = cx;
    }

    fn toggle_select_mode(&mut self, _: &ToggleSelectMode, _: &mut Window, cx: &mut Context<Self>) {
        vi::toggle(&mut self.pty.term.lock(), &mut self.vi);
        cx.notify();
    }

    /// Carry out what a key in select mode asked for.
    fn vi_outcome(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        match outcome {
            Outcome::Handled => {}
            Outcome::Copy(text) => {
                #[cfg(target_os = "linux")]
                cx.write_to_primary(ClipboardItem::new_string(text.clone()));
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                vi::toggle(&mut self.pty.term.lock(), &mut self.vi);
            }
            Outcome::Exit => vi::toggle(&mut self.pty.term.lock(), &mut self.vi),
            Outcome::Search { backward } => {
                self.search_backward = backward;
                self.open_search(cx);
            }
            Outcome::SearchAgain { reverse } => {
                let older = self.search_backward != reverse;
                let search = match self.search.as_mut() {
                    Some(s) => s,
                    None => match self.last_search.as_mut() {
                        Some(s) => {
                            // Output may have arrived since; find matches afresh.
                            s.rescan(&self.pty.term.lock());
                            s
                        }
                        None => return,
                    },
                };
                search.step(&mut self.pty.term.lock(), older);
            }
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.pty.term.lock().selection_to_string() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        if let Some(search) = self.search.as_mut() {
            search.query.push_str(text.lines().next().unwrap_or(""));
            self.update_search(cx);
        } else {
            self.paste_text(&text);
        }
    }

    fn find(&mut self, _: &Find, _: &mut Window, cx: &mut Context<Self>) {
        self.search_backward = false;
        self.open_search(cx);
    }

    fn open_search(&mut self, cx: &mut Context<Self>) {
        if self.search.is_some() {
            return;
        }
        // Start from the selection when it's a single short line.
        let selected = self
            .pty
            .term
            .lock()
            .selection_to_string()
            .filter(|t| !t.contains('\n') && t.len() <= 200);
        let mut search = Search::default();
        search.query = selected.unwrap_or_default();
        self.search = Some(search);
        self.update_search(cx);
    }

    fn close_search(&mut self, cx: &mut Context<Self>) {
        self.last_search = self.search.take().filter(|s| !s.query.is_empty());
        self.search_rescan = None;
        cx.notify();
    }

    /// Re-run the search after the query or its options change.
    fn update_search(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.search.as_mut() {
            search.update(&mut self.pty.term.lock());
        }
        cx.notify();
    }

    fn schedule_marks_rescan(&mut self, cx: &mut Context<Self>) {
        if self.marks_rescan.is_some() || !self.pty.shell_state.lock().unwrap().active {
            return;
        }
        self.marks_rescan = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
            let _ = this.update(cx, |this, cx| {
                this.marks_rescan = None;
                this.rescan_marks();
                cx.notify();
            });
        }));
    }

    /// Find every prompt again and refresh the exit statuses shown in the margin.
    fn rescan_marks(&mut self) -> Vec<marks::Prompt> {
        let prompts = marks::prompts(&self.pty.term.lock());
        self.exits = Rc::new(marks::exits(&prompts));
        prompts
    }

    fn prev_prompt(&mut self, _: &PrevPrompt, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(true, cx);
    }

    fn next_prompt(&mut self, _: &NextPrompt, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(false, cx);
    }

    /// Scroll so the previous or next prompt is at the top of the pane.
    fn jump(&mut self, older: bool, cx: &mut Context<Self>) {
        let prompts = self.rescan_marks();
        let mut term = self.pty.term.lock();
        let offset = term.grid().display_offset() as i32;
        let anchor = self.jump_target.as_ref().and_then(|id| prompts.iter().position(|p| &p.id == id));
        let target = match anchor {
            Some(i) if older => i.checked_sub(1),
            Some(i) => Some(i + 1).filter(|&j| j < prompts.len()),
            // At the bottom, start from the prompt you're typing at.
            None if offset == 0 && older => prompts.len().checked_sub(2),
            None if offset == 0 => None,
            // Scrolled up by hand: start from the top of the view.
            None if older => prompts.iter().rposition(|p| p.line.0 < -offset),
            None => prompts.iter().position(|p| p.line.0 > -offset),
        };
        match target {
            Some(i) => {
                let max = alacritty_terminal::grid::Dimensions::history_size(term.grid()) as i32;
                let wanted = (-prompts[i].line.0).clamp(0, max);
                term.scroll_display(Scroll::Delta(wanted - offset));
                self.jump_target = Some(prompts[i].id.clone());
            }
            None if !older => {
                term.scroll_display(Scroll::Bottom);
                self.jump_target = None;
            }
            None => {}
        }
        drop(term);
        cx.notify();
    }

    /// Copy what the last finished command printed, and select it.
    fn copy_last_output(&mut self, _: &CopyLastOutput, _: &mut Window, cx: &mut Context<Self>) {
        let prompts = self.rescan_marks();
        let mut term = self.pty.term.lock();
        let Some((start, end)) = marks::last_output(&term, &prompts) else { return };
        let text = term.bounds_to_string(start, end);
        let text = text.trim_end();
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        let mut selection = Selection::new(SelectionType::Lines, start, Side::Left);
        selection.update(end, Side::Right);
        term.selection = Some(selection);
        drop(term);
        cx.notify();
    }

    fn schedule_search_rescan(&mut self, cx: &mut Context<Self>) {
        if self.search_rescan.is_some() || self.search.as_ref().is_none_or(|s| s.query.is_empty()) {
            return;
        }
        self.search_rescan = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(150)).await;
            let _ = this.update(cx, |this, cx| {
                this.search_rescan = None;
                let term = this.pty.term.lock();
                if let Some(search) = this.search.as_mut() {
                    search.rescan(&term);
                }
                drop(term);
                cx.notify();
            });
        }));
    }

    /// Keys typed while the find bar is open. Returns false for keys that
    /// should still reach the shell (Ctrl/Cmd combinations such as Ctrl+C).
    fn search_key(&mut self, ks: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let m = ks.modifiers;
        let Some(search) = self.search.as_mut() else { return false };
        match ks.key.as_str() {
            "escape" => self.close_search(cx),
            "enter" | "up" | "down" => {
                let older = match ks.key.as_str() {
                    "enter" => !m.shift,
                    k => k == "up",
                };
                search.step(&mut self.pty.term.lock(), older);
            }
            "backspace" => {
                search.query.pop();
                self.update_search(cx);
            }
            "c" if m.alt => {
                search.case_sensitive = !search.case_sensitive;
                self.update_search(cx);
            }
            "r" if m.alt => {
                search.regex = !search.regex;
                self.update_search(cx);
            }
            _ if m.control || m.platform => return false,
            _ if m.alt => {}
            _ => {
                if let Some(ch) = ks.key_char.as_ref().filter(|c| !c.chars().any(char::is_control)) {
                    search.query.push_str(ch);
                    self.update_search(cx);
                }
            }
        }
        true
    }

    /// Text typed through an input method: into the find bar when it's open,
    /// else to the program, as if typed.
    fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(search) = self.search.as_mut() {
            search.query.push_str(text);
            self.update_search(cx);
            return;
        }
        self.jump_target = None;
        {
            let mut term = self.pty.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.pty.write(text.as_bytes().to_vec());
    }

    fn paste_text(&mut self, text: &str) {
        let bracketed = self.pty.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let body = text.replace("\r\n", "\r").replace('\n', "\r");
        let data = if bracketed {
            format!("\x1b[200~{}\x1b[201~", body.replace('\x1b', ""))
        } else {
            body
        };
        self.pty.term.lock().scroll_display(Scroll::Bottom);
        self.pty.write(data.into_bytes());
    }
}

/// Input methods (CJK input, dead keys) talk to the pane through this. A
/// terminal has no editable document, so the only text it holds is what's
/// being composed; committed text goes straight to the program.
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let utf16: Vec<u16> = self.marked.as_ref()?.encode_utf16().collect();
        let range = range.start.min(utf16.len())..range.end.min(utf16.len());
        *adjusted = Some(range.clone());
        Some(String::from_utf16_lossy(&utf16[range]))
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let end = self.marked.as_ref().map_or(0, |m| m.encode_utf16().count());
        Some(UTF16Selection { range: end..end, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<std::ops::Range<usize>> {
        self.marked.as_ref().map(|m| 0..m.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        if !text.is_empty() {
            self.type_text(text, cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    /// Where the input method should open its candidate window: at the cursor.
    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.get()?;
        let term = self.pty.term.lock();
        let cursor = term.grid().cursor.point;
        let row = cursor.line.0 + term.grid().display_offset() as i32;
        if row < 0 || row as usize >= layout.rows {
            return None;
        }
        Some(Bounds::new(
            point(
                layout.origin.x + layout.cell_width * cursor.column.0 as f32,
                layout.origin.y + layout.line_height * row as f32,
            ),
            size(layout.cell_width, layout.line_height),
        ))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

/// Ctrl on Linux, Cmd on macOS: held to underline and click links.
fn link_modifier(m: &Modifiers) -> bool {
    if cfg!(target_os = "macos") { m.platform } else { m.control }
}

// ---------------------------------------------------------------------------
// Rendering

struct Frame {
    backgrounds: Vec<(Bounds<Pixels>, Hsla)>,
    glyphs: Vec<GlyphDraw>,
    shapes: Vec<(boxdraw::Shape, Hsla)>,
    /// Shell-integration marks in the left margin.
    marks: Vec<(Bounds<Pixels>, Hsla)>,
    decorations: Vec<Decoration>,
    cursor: Option<(Bounds<Pixels>, CursorShape)>,
    cursor_color: Hsla,
    /// Text an input method is composing, shaped, at the cursor's cell.
    preedit: Option<(Bounds<Pixels>, ShapedLine)>,
    /// Target of the hovered link, shown in a pill at the bottom of the pane.
    link_label: Option<(ShapedLine, Hsla, Hsla)>,
}

struct GlyphDraw {
    origin: Point<Pixels>,
    glyph: Glyph,
    color: Hsla,
}

enum Decoration {
    Underline(Point<Pixels>, Pixels, UnderlineStyle),
    Strikethrough(Point<Pixels>, Pixels, StrikethroughStyle),
}

/// A run of operator characters waiting to be drawn.
struct Join {
    row: usize,
    col: usize,
    len: usize,
    style: u8,
    color: Hsla,
    text: String,
}

/// A same-style stretch of cells on one row: (row, start_col, end_col, style).
type Run<S> = (usize, usize, usize, S);

/// Grow `run` by one cell, or move it to `done` and start a new one.
fn extend_run<S: PartialEq>(
    run: &mut Option<Run<S>>,
    style: Option<S>,
    (row, col, span): (usize, usize, usize),
    done: &mut Vec<Run<S>>,
) {
    if let (Some((r, _, end, s)), Some(new)) = (run.as_mut(), style.as_ref())
        && *r == row
        && *end == col
        && s == new
    {
        *end = col + span;
        return;
    }
    done.extend(run.take());
    *run = style.map(|s| (row, col, col + span, s));
}

/// Terminal emulation options derived from the settings.
fn term_config(settings: &Settings) -> alacritty_terminal::term::Config {
    alacritty_terminal::term::Config {
        scrolling_history: settings.scrollback,
        default_cursor_style: settings.cursor_style(),
        kitty_keyboard: true,
        ..Default::default()
    }
}

fn same_term_config(a: &alacritty_terminal::term::Config, b: &alacritty_terminal::term::Config) -> bool {
    a.scrolling_history == b.scrolling_history && a.default_cursor_style == b.default_cursor_style
}

/// The find bar floating at the top right of the pane.
fn search_bar(search: &Search, theme: &Theme) -> gpui::AnyElement {
    let has_query = !search.query.is_empty();
    let status: gpui::SharedString = if search.invalid {
        "Invalid pattern".into()
    } else if !has_query {
        "".into()
    } else if search.count() == 0 {
        "No results".into()
    } else {
        let more = if search.truncated { "+" } else { "" };
        match search.current {
            Some(i) => format!("{} of {}{more}", i + 1, search.count()).into(),
            None => format!("{}{more} results", search.count()).into(),
        }
    };
    let toggle = |label: &'static str, on: bool, hint: &'static str| {
        div()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(5.0))
            .font_family(MONO_FONT)
            .text_size(px(11.5))
            .when(on, |d| d.bg(alpha(theme.accent, 0.22)).text_color(theme.text))
            .when(!on, |d| d.text_color(theme.muted))
            .child(label)
            .id(hint)
    };
    div()
        .absolute()
        .top(px(4.0))
        .right(px(4.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .h(px(34.0))
        .pl(px(12.0))
        .pr(px(6.0))
        .min_w(px(280.0))
        .font_family(UI_FONT)
        .text_size(px(12.5))
        .bg(theme.raised)
        .border_1()
        .border_color(alpha(theme.text, 0.08))
        .rounded(px(9.0))
        .shadow(vec![gpui::BoxShadow {
            color: alpha(gpui::black(), 0.35),
            offset: point(px(0.0), px(6.0)),
            blur_radius: px(18.0),
            spread_radius: px(0.0),
        }])
        .child(div().text_color(theme.accent).child("⌕"))
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .font_family(MONO_FONT)
                .text_color(if has_query { theme.text } else { theme.muted })
                .when(!has_query, |d| d.child(caret(theme.accent)).child("Find"))
                .when(has_query, |d| d.child(search.query.clone()).child(caret(theme.accent))),
        )
        .child(
            div()
                .text_color(if search.invalid { theme.ansi[1] } else { theme.subtext })
                .child(status),
        )
        .child(toggle("Aa", search.case_sensitive, "search-case"))
        .child(toggle(".*", search.regex, "search-regex"))
        .into_any_element()
}

/// Title and body for a finished-command notification.
fn notification_text(
    command: Option<&str>,
    exit: Option<i32>,
    took: std::time::Duration,
    cwd: Option<PathBuf>,
) -> (String, String) {
    let mut command = command.unwrap_or("Command").to_string();
    if command.chars().count() > 60 {
        command = command.chars().take(59).chain(['…']).collect();
    }
    let secs = took.as_secs();
    let took = match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    };
    let (mark, how) = match exit {
        Some(0) | None => ("✓", format!("Finished in {took}")),
        Some(code) => ("✗", format!("Failed with exit {code} after {took}")),
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let place = cwd.map(|p| {
        let p = p.display().to_string();
        match p.strip_prefix(&home) {
            Some(rest) if !home.is_empty() => format!(" · ~{rest}"),
            _ => format!(" · {p}"),
        }
    });
    (format!("{mark} {command}"), format!("{how}{}", place.unwrap_or_default()))
}

/// The pill at the bottom of a pane in keyboard select mode, with what the
/// next keys do.
fn select_badge(text: String, theme: &Theme) -> gpui::AnyElement {
    div()
        .absolute()
        .bottom(px(6.0))
        .right(px(6.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .h(px(26.0))
        .px(px(10.0))
        .font_family(UI_FONT)
        .text_size(px(12.0))
        .text_color(theme.muted)
        .bg(theme.raised)
        .border_1()
        .border_color(alpha(theme.accent, 0.45))
        .rounded(px(8.0))
        .shadow(vec![gpui::BoxShadow {
            color: alpha(gpui::black(), 0.3),
            offset: point(px(0.0), px(4.0)),
            blur_radius: px(12.0),
            spread_radius: px(0.0),
        }])
        .child(div().size(px(7.0)).rounded(px(4.0)).bg(theme.accent))
        .child(text)
        .into_any_element()
}

fn caret(color: Hsla) -> gpui::Div {
    div().w(px(2.0)).h(px(16.0)).mx(px(1.0)).rounded(px(1.0)).bg(color)
}

fn paint_shape(window: &mut Window, shape: boxdraw::Shape, color: Hsla) {
    use boxdraw::{Corner, Shape};
    match shape {
        Shape::Rect(b, a) => window.paint_quad(fill(b, alpha(color, color.a * a))),
        Shape::Arc { quad, clip, corner, radius, width } => {
            let zero = px(0.0);
            let mut radii = gpui::Corners::all(zero);
            let mut edges = gpui::Edges::all(zero);
            match corner {
                Corner::TopLeft => (radii.top_left, edges.top, edges.left) = (radius, width, width),
                Corner::TopRight => (radii.top_right, edges.top, edges.right) = (radius, width, width),
                Corner::BottomRight => {
                    (radii.bottom_right, edges.bottom, edges.right) = (radius, width, width)
                }
                Corner::BottomLeft => {
                    (radii.bottom_left, edges.bottom, edges.left) = (radius, width, width)
                }
            }
            window.with_content_mask(Some(gpui::ContentMask { bounds: clip }), |window| {
                window.paint_quad(gpui::quad(
                    quad,
                    radii,
                    gpui::transparent_black(),
                    edges,
                    color,
                    gpui::BorderStyle::Solid,
                ))
            });
        }
        Shape::Polygon(points) | Shape::Polyline(points, _)
            if points.len() < 2 => {}
        Shape::Polygon(points) => {
            let mut path = gpui::PathBuilder::fill();
            path.move_to(points[0]);
            for p in &points[1..] {
                path.line_to(*p);
            }
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        }
        Shape::Polyline(points, width) => {
            let mut path = gpui::PathBuilder::stroke(width);
            path.move_to(points[0]);
            for p in &points[1..] {
                path.line_to(*p);
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        }
    }
}

fn rgb_to_hsla(rgb: Rgb) -> Hsla {
    Rgba { r: rgb.r as f32 / 255.0, g: rgb.g as f32 / 255.0, b: rgb.b as f32 / 255.0, a: 1.0 }
        .into()
}

fn resolve(color: Color, colors: &Colors, theme: &Theme) -> Hsla {
    match color {
        Color::Spec(rgb) => rgb_to_hsla(rgb),
        Color::Indexed(i) => {
            colors[i as usize].map(rgb_to_hsla).unwrap_or_else(|| indexed(theme, i))
        }
        Color::Named(named) => {
            let idx = named as usize;
            if let Some(rgb) = colors[named] {
                return rgb_to_hsla(rgb);
            }
            match named {
                _ if idx < 16 => theme.ansi[idx],
                NamedColor::Foreground | NamedColor::BrightForeground => theme.text,
                NamedColor::Background => theme.pane,
                NamedColor::Cursor => theme.cursor,
                NamedColor::DimForeground => alpha(theme.text, 0.7),
                _ => {
                    // Dim variants of the eight base colors.
                    let base = idx.saturating_sub(NamedColor::DimBlack as usize).min(7);
                    alpha(theme.ansi[base], 0.7)
                }
            }
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = Settings::get(cx);
        let theme = settings.theme().clone();
        let font_size = settings.font_size();
        let font_family = settings.font_family.clone();
        let line_height_factor = settings.line_height;
        let focused = self.focus.is_focused(window);
        let blink_hidden = focused && !self.blink_on;
        let marked = self.marked.clone();
        let ime_focus = self.focus.clone();
        let ime_entity = cx.entity();
        let preedit_bg = Settings::get(cx).theme().raised;
        let term = self.pty.term.clone();
        let layout_cell = self.layout.clone();
        let entity = cx.entity();
        let drag_entity = entity.clone();
        let hovered_link = self.hovered_link.clone();
        let dragging = self.selecting || self.reported_button.is_some();
        let mouse_mode = mouse::reporting(self.mode());
        let search_hits = self
            .search
            .as_ref()
            .map(|s| s.visible(&self.pty.term.lock()))
            .unwrap_or_default();
        let search_bar = self.search.as_ref().map(|s| search_bar(s, &theme));
        let select_badge = {
            let term = self.pty.term.lock();
            vi::active(&term).then(|| select_badge(vi::status(&term, &self.vi), &theme))
        };
        let exits = self.exits.clone();
        let jump_target = self.jump_target.clone();

        let prepaint = move |bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App| -> Frame {
            let text_system = window.text_system().clone();
            let base_font = font(font_family.clone());
            let font_id = text_system.resolve_font(&base_font);
            let cell_width = text_system
                .advance(font_id, font_size, 'm')
                .map(|s| s.width)
                .unwrap_or(px(8.0));
            let line_height = (font_size * line_height_factor).round();
            let cols = ((bounds.size.width / cell_width).floor() as usize).max(2);
            let rows = ((bounds.size.height / line_height).floor() as usize).max(1);
            // Centre the grid so the space left over by whole cells is shared
            // evenly between opposite edges (whole pixels keep glyphs crisp).
            let slack_x = (bounds.size.width - cell_width * cols as f32).max(px(0.0));
            let slack_y = (bounds.size.height - line_height * rows as f32).max(px(0.0));
            let origin = point(
                (bounds.origin.x + slack_x / 2.0).round(),
                (bounds.origin.y + slack_y / 2.0).round(),
            );

            layout_cell.set(Some(GridLayout {
                origin,
                cell_width,
                line_height,
                cols,
                rows,
            }));

            let grid_size = GridSize {
                cols: cols as u16,
                rows: rows as u16,
                cell_width: f32::from(cell_width) as u16,
                cell_height: f32::from(line_height) as u16,
            };
            entity.update(cx, |this, _| this.pty.resize(grid_size));

            let mut fonts = [base_font.clone(), base_font.clone(), base_font.clone(), base_font];
            fonts[1].weight = FontWeight::BOLD;
            fonts[2].style = FontStyle::Italic;
            fonts[3].weight = FontWeight::BOLD;
            fonts[3].style = FontStyle::Italic;

            let term = term.lock();
            let content = term.renderable_content();
            let offset = content.display_offset as i32;
            let selection = content.selection;
            let colors = content.colors;
            let cursor = content.cursor;
            // Select mode's cursor is the accent colour, so it's clearly not the shell's.
            let vi_cursor = content.mode.contains(TermMode::VI);
            let cursor_row = cursor.point.line.0 + offset;
            let cursor_col = cursor.point.column.0;
            let show_cursor = cursor.shape != CursorShape::Hidden
                && !(blink_hidden && !vi_cursor)
                && cursor_row >= 0
                && (cursor_row as usize) < rows;

            let mut backgrounds: Vec<(Bounds<Pixels>, Hsla)> = Vec::new();
            let mut glyphs: Vec<GlyphDraw> = Vec::new();
            let mut shapes: Vec<(boxdraw::Shape, Hsla)> = Vec::new();
            // First visible row of each prompt, with its link id.
            let mut prompt_rows: Vec<(usize, String)> = Vec::new();
            let mut underlines: Vec<Run<UnderlineStyle>> = Vec::new();
            let mut strikes: Vec<Run<StrikethroughStyle>> = Vec::new();
            let mut cursor_wide = false;
            // (row, start_col, end_col, color) for background run merging
            let mut bg_run: Option<(usize, usize, usize, Hsla)> = None;
            let mut underline_run: Option<(usize, usize, usize, UnderlineStyle)> = None;
            let mut strike_run: Option<(usize, usize, usize, StrikethroughStyle)> = None;
            // Consecutive operator characters, drawn together so ligatures form.
            let mut join: Option<Join> = None;

            let cache = GlyphCache::get(cx, &font_family, font_size);
            let (ascent, descent) = cache.metrics(&text_system, &fonts[0]);
            let baseline = (line_height - ascent - descent) / 2.0 + ascent;
            let underline_y = baseline + descent * 0.618;
            let strike_y = (ascent * 0.5 + baseline) * 0.5;

            let cell_bounds = |row: usize, col: usize, span: usize| {
                Bounds::new(
                    point(
                        origin.x + cell_width * col as f32,
                        origin.y + line_height * row as f32,
                    ),
                    size(cell_width * span as f32, line_height),
                )
            };
            let emit = |glyphs: &mut Vec<GlyphDraw>, row: usize, col: usize, color: Hsla, cached: &[Glyph]| {
                let origin = cell_bounds(row, col, 1).origin;
                glyphs.extend(cached.iter().map(|g| GlyphDraw {
                    origin: point(origin.x + g.x, origin.y + baseline),
                    glyph: *g,
                    color,
                }));
            };
            let flush_join = |join: &mut Option<Join>, glyphs: &mut Vec<GlyphDraw>, cache: &mut GlyphCache| {
                if let Some(j) = join.take() {
                    let cached = if j.len == 1 {
                        cache.char(&text_system, &fonts, j.style, j.text.chars().next().unwrap_or(' '))
                    } else {
                        cache.cluster(&text_system, &fonts, j.style, &j.text)
                    };
                    emit(glyphs, j.row, j.col, j.color, cached);
                }
            };

            for indexed_cell in content.display_iter {
                let row = indexed_cell.point.line.0 + offset;
                if row < 0 || row as usize >= rows {
                    continue;
                }
                let row = row as usize;
                let col = indexed_cell.point.column.0;
                if col >= cols {
                    continue;
                }
                let cell = indexed_cell.cell;
                if let Some(link) = cell.hyperlink()
                    && link.uri().starts_with(shell_integration::PROMPT)
                    && prompt_rows.last().is_none_or(|(_, id): &(usize, String)| id != link.id())
                {
                    prompt_rows.push((row, link.id().to_string()));
                }
                let flags = cell.flags;
                if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                let wide = flags.contains(Flags::WIDE_CHAR);
                let span = if wide { 2 } else { 1 };

                // Colors
                let mut fg_color = cell.fg;
                if flags.contains(Flags::BOLD)
                    && let Color::Named(n) = fg_color
                    && (n as usize) < 8
                {
                    fg_color = Color::Indexed(n as u8 + 8);
                }
                let mut fg = resolve(fg_color, colors, &theme);
                let default_bg = matches!(cell.bg, Color::Named(NamedColor::Background));
                let mut bg = (!default_bg).then(|| resolve(cell.bg, colors, &theme));
                if flags.contains(Flags::INVERSE) {
                    let new_bg = fg;
                    fg = bg.unwrap_or(theme.pane);
                    bg = Some(new_bg);
                }
                if flags.contains(Flags::DIM) {
                    fg = alpha(fg, 0.66);
                }
                let in_link =
                    hovered_link.as_ref().is_some_and(|l| l.range.contains(&indexed_cell.point));
                if in_link {
                    fg = theme.accent;
                }
                if let Some((_, current)) =
                    search_hits.iter().find(|(r, _)| r.contains(&indexed_cell.point))
                {
                    if *current {
                        bg = Some(theme.accent);
                        fg = theme.pane;
                    } else {
                        bg = Some(alpha(theme.accent, 0.3));
                    }
                }
                if selection.is_some_and(|s| s.contains(indexed_cell.point)) {
                    bg = Some(theme.selection);
                }
                let is_cursor = show_cursor
                    && row == cursor_row as usize
                    && col == cursor.point.column.0;
                if is_cursor {
                    cursor_wide = wide;
                    if focused && cursor.shape == CursorShape::Block {
                        fg = theme.pane;
                    }
                }

                // Background runs
                match (&mut bg_run, bg) {
                    (Some((r, _, end, c)), Some(b)) if *r == row && *end == col && *c == b => {
                        *end = col + span;
                    }
                    (_, b) => {
                        if let Some((r, start, end, c)) = bg_run.take() {
                            backgrounds.push((cell_bounds(r, start, end - start), c));
                        }
                        if let Some(b) = b {
                            bg_run = Some((row, col, col + span, b));
                        }
                    }
                }

                // Underline and strikethrough runs
                let underline =
                    (in_link || flags.intersects(Flags::ALL_UNDERLINES)).then(|| UnderlineStyle {
                        thickness: px(1.0),
                        color: Some(fg),
                        wavy: !in_link && flags.contains(Flags::UNDERCURL),
                    });
                extend_run(&mut underline_run, underline, (row, col, span), &mut underlines);
                let strike = flags
                    .contains(Flags::STRIKEOUT)
                    .then(|| StrikethroughStyle { thickness: px(1.0), color: Some(fg) });
                extend_run(&mut strike_run, strike, (row, col, span), &mut strikes);

                // Text
                let ch = if flags.contains(Flags::HIDDEN) || cell.c == '\t' { ' ' } else { cell.c };
                let style = (flags.contains(Flags::BOLD) as u8)
                    | ((flags.contains(Flags::ITALIC) as u8) << 1);
                let zerowidth = cell.zerowidth().filter(|z| !z.is_empty() && ch != ' ');
                if !wide && zerowidth.is_none() && glyphs::joins(ch) {
                    if let Some(j) = &mut join
                        && j.row == row
                        && j.col + j.len == col
                        && j.style == style
                        && j.color == fg
                        && j.len < glyphs::MAX_JOIN
                    {
                        j.text.push(ch);
                        j.len += 1;
                        continue;
                    }
                    flush_join(&mut join, &mut glyphs, cache);
                    join = Some(Join { row, col, len: 1, style, color: fg, text: ch.to_string() });
                    continue;
                }
                flush_join(&mut join, &mut glyphs, cache);
                if ch == ' ' || ch == '\0' {
                    continue;
                }
                if zerowidth.is_none() && boxdraw::handles(ch) {
                    // Snap to whole pixels at edges shared with neighbouring cells.
                    let edge = |c: usize| f32::from(origin.x + cell_width * c as f32).round();
                    let top = f32::from(origin.y + line_height * row as f32).round();
                    let bottom = top + f32::from(line_height);
                    shapes.extend(
                        boxdraw::shapes(ch, edge(col), edge(col + span), top, bottom)
                            .into_iter()
                            .map(|s| (s, fg)),
                    );
                    continue;
                }
                let cached = match zerowidth {
                    Some(extra) => {
                        let mut s = String::from(ch);
                        s.extend(extra.iter());
                        cache.cluster(&text_system, &fonts, style, &s)
                    }
                    None => cache.char(&text_system, &fonts, style, ch),
                };
                emit(&mut glyphs, row, col, fg, cached);
            }
            flush_join(&mut join, &mut glyphs, cache);
            if let Some((r, start, end, c)) = bg_run.take() {
                backgrounds.push((cell_bounds(r, start, end - start), c));
            }
            // Margin marks beside each prompt: muted for success, red for failure.
            let mut marks = Vec::new();
            for (row, id) in &prompt_rows {
                let top = cell_bounds(*row, 0, 1).origin;
                if jump_target.as_deref() == Some(id) {
                    backgrounds.insert(0, (cell_bounds(*row, 0, cols), alpha(theme.accent, 0.14)));
                }
                let color = match exits.get(id) {
                    Some(Some(0)) => alpha(theme.muted, 0.55),
                    Some(Some(_)) => theme.ansi[1],
                    _ => continue,
                };
                let inset = (line_height * 0.2).round();
                marks.push((
                    Bounds::new(point(top.x - px(8.0), top.y + inset), size(px(3.0), line_height - inset * 2.0)),
                    color,
                ));
            }
            underlines.extend(underline_run.take());
            strikes.extend(strike_run.take());
            let place = |(r, start, end): (usize, usize, usize), y: Pixels| {
                let o = cell_bounds(r, start, 1).origin;
                (point(o.x, o.y + y), cell_width * (end - start) as f32)
            };
            let decorations = underlines
                .into_iter()
                .map(|(r, a, b, style)| {
                    let (o, w) = place((r, a, b), underline_y);
                    Decoration::Underline(o, w, style)
                })
                .chain(strikes.into_iter().map(|(r, a, b, style)| {
                    let (o, w) = place((r, a, b), strike_y);
                    Decoration::Strikethrough(o, w, style)
                }))
                .collect();
            // Painting doesn't need the terminal; let the PTY thread carry on.
            drop(term);

            let cursor = show_cursor.then(|| {
                let span = if cursor_wide { 2 } else { 1 };
                (cell_bounds(cursor_row as usize, cursor.point.column.0, span), cursor.shape)
            });

            // Composed input sits over the cursor's cell, whether or not the
            // cursor is showing (blinking, say).
            let preedit = marked.as_ref().filter(|_| cursor_row >= 0 && (cursor_row as usize) < rows).map(|text| {
                let run = TextRun {
                    len: text.len(),
                    font: fonts[0].clone(),
                    color: theme.text,
                    background_color: None,
                    underline: Some(UnderlineStyle { thickness: px(1.0), color: Some(theme.accent), wavy: false }),
                    strikethrough: None,
                };
                let shaped = text_system.shape_line(text.clone().into(), font_size, &[run], None);
                (cell_bounds(cursor_row as usize, cursor_col, 1), shaped)
            });

            let link_label = hovered_link.as_ref().map(|link| {
                let mut uri = link.uri.clone();
                if uri.chars().count() > 120 {
                    uri = uri.chars().take(119).chain(['…']).collect();
                }
                if !links::openable(link) {
                    uri = format!("{uri}  (won't open: unsupported scheme)");
                }
                let len = uri.len();
                let run = TextRun {
                    len,
                    font: font(UI_FONT),
                    color: theme.subtext,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = text_system.shape_line(uri.into(), px(12.0), &[run], None);
                (shaped, theme.raised, theme.border)
            });

            Frame {
                backgrounds,
                glyphs,
                shapes,
                marks,
                decorations,
                cursor,
                cursor_color: if vi_cursor { theme.accent } else { theme.cursor },
                preedit,
                link_label,
            }
        };

        let paint = move |bounds: Bounds<Pixels>, frame: Frame, window: &mut Window, cx: &mut App| {
            // Follow a drag that started in this pane after it leaves the pane;
            // the div's own listener only sees motion while hovered.
            if dragging {
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && !bounds.contains(&ev.position) {
                        drag_entity.update(cx, |this, cx| this.handle_motion(ev, cx));
                    }
                });
            }
            // One layer for the whole grid: GPUI then orders it as a unit instead of
            // tracking overlaps for each of the thousands of glyphs.
            window.paint_layer(bounds, |window| {
                for (b, color) in &frame.backgrounds {
                    window.paint_quad(fill(*b, *color));
                }
                let cursor_color = frame.cursor_color;
                if let Some((b, CursorShape::Block)) = frame.cursor
                    && focused
                {
                    window.paint_quad(fill(b, cursor_color).corner_radii(px(2.0)));
                }
                for (b, color) in &frame.marks {
                    window.paint_quad(fill(*b, *color).corner_radii(px(1.5)));
                }
                for (shape, color) in frame.shapes {
                    paint_shape(window, shape, color);
                }
                for g in &frame.glyphs {
                    let Glyph { font_id, id, emoji, .. } = g.glyph;
                    let _ = if emoji {
                        window.paint_emoji(g.origin, font_id, id, font_size)
                    } else {
                        window.paint_glyph(g.origin, font_id, id, font_size, g.color)
                    };
                }
                for d in &frame.decorations {
                    match d {
                        Decoration::Underline(origin, width, style) => {
                            window.paint_underline(*origin, *width, style)
                        }
                        Decoration::Strikethrough(origin, width, style) => {
                            window.paint_strikethrough(*origin, *width, style)
                        }
                    }
                }
                if let Some((b, shape)) = frame.cursor {
                    let thick = px(2.0);
                    match shape {
                        CursorShape::Block if focused => {}
                        CursorShape::Block | CursorShape::HollowBlock => {
                            window.paint_quad(
                                gpui::outline(b, cursor_color, gpui::BorderStyle::Solid)
                                    .corner_radii(px(2.0)),
                            );
                        }
                        CursorShape::Beam => {
                            window.paint_quad(fill(
                                Bounds::new(b.origin, size(thick, b.size.height)),
                                cursor_color,
                            ));
                        }
                        CursorShape::Underline => {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(b.origin.x, b.origin.y + b.size.height - thick),
                                    size(b.size.width, thick),
                                ),
                                cursor_color,
                            ));
                        }
                        CursorShape::Hidden => {}
                    }
                }
            });
            if let Some((anchor, line)) = &frame.preedit {
                let back = Bounds::new(anchor.origin, size(line.width.max(anchor.size.width), anchor.size.height));
                window.paint_quad(fill(back, preedit_bg).corner_radii(px(2.0)));
                let _ = line.paint(anchor.origin, anchor.size.height, window, cx);
            }
            if focused {
                window.handle_input(&ime_focus, ElementInputHandler::new(bounds, ime_entity.clone()), cx);
            }
            if let Some((label, bg, border)) = &frame.link_label {
                // Clamp to the pane so a long URL can't spill out.
                let (pad_x, height) = (px(8.0), px(22.0));
                let width = (label.width + pad_x * 2.0).min(bounds.size.width);
                let pill = Bounds::new(
                    point(bounds.origin.x, bounds.origin.y + bounds.size.height - height),
                    size(width, height),
                );
                window.paint_quad(
                    fill(pill, *bg).corner_radii(px(6.0)).border_widths(px(1.0)).border_color(*border),
                );
                window.with_content_mask(Some(gpui::ContentMask { bounds: pill }), |window| {
                    let origin = point(pill.origin.x + pad_x, pill.origin.y + px(3.0));
                    let _ = label.paint(origin, px(16.0), window, cx);
                });
            }
        };

        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .when(self.hovered_link.is_some(), |d| d.cursor_pointer())
            .when(self.hovered_link.is_none() && mouse_mode, |d| d.cursor_default())
            .when(self.hovered_link.is_none() && !mouse_mode, |d| d.cursor_text())
            .on_hover(cx.listener(Self::on_hover))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::prev_prompt))
            .on_action(cx.listener(Self::next_prompt))
            .on_action(cx.listener(Self::copy_last_output))
            .on_action(cx.listener(Self::toggle_select_mode))
            .relative()
            .child(canvas(prepaint, paint).size_full())
            .children(search_bar)
            .children(select_badge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn notification_wording() {
        let home = PathBuf::from(std::env::var("HOME").unwrap()).join("proj");
        let (t, b) = notification_text(Some("cargo build"), Some(0), Duration::from_secs(134), Some(home));
        assert_eq!((t.as_str(), b.as_str()), ("✓ cargo build", "Finished in 2m 14s · ~/proj"));
        let (t, b) = notification_text(Some("make test"), Some(2), Duration::from_secs(45), None);
        assert_eq!((t.as_str(), b.as_str()), ("✗ make test", "Failed with exit 2 after 45s"));
        let (t, b) = notification_text(None, None, Duration::from_secs(3720), Some("/srv".into()));
        assert_eq!((t.as_str(), b.as_str()), ("✓ Command", "Finished in 1h 2m · /srv"));
        let long = "x".repeat(80);
        assert_eq!(notification_text(Some(&long), Some(0), Duration::ZERO, None).0.chars().count(), 62);
    }
}
