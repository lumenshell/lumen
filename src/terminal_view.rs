//! A single terminal pane: renders the alacritty grid with GPUI and feeds input to the PTY.

use std::cell::Cell as StdCell;
use std::path::PathBuf;
use std::rc::Rc;

use alacritty_terminal::event::Event;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use futures::StreamExt;
use gpui::{
    App, Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, FontStyle,
    FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, Modifiers,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, Render, Rgba, ScrollDelta, ScrollWheelEvent, ShapedLine,
    SharedString, StatefulInteractiveElement, StrikethroughStyle, Styled, Subscription, Task,
    TextRun, UnderlineStyle, Window,
    canvas, div, fill, font, point, prelude::FluentBuilder, px, size,
};

use crate::links::{self, Link, LinkFinder};
use crate::pty::{GridSize, Pty};
use crate::settings::{MONO_FONT, Settings, UI_FONT};
use crate::theme::{Theme, alpha, indexed};
use crate::{Copy, Paste, input};

pub enum TerminalEvent {
    TitleChanged,
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
    layout: Rc<StdCell<Option<GridLayout>>>,
    selecting: bool,
    scroll_acc: f32,
    links: LinkFinder,
    /// Link under the pointer while the link modifier is held.
    hovered_link: Option<Link>,
    /// Last pointer position and modifiers over this pane.
    pointer: Option<(Point<Pixels>, Modifiers)>,
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

impl TerminalView {
    pub fn new(cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (pty, mut rx) = Pty::spawn(cwd).expect("failed to spawn shell");
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

        let subs = vec![cx.on_focus(&focus, window, |_, _, cx| cx.emit(TerminalEvent::Focused))];

        Self {
            pty,
            focus,
            title: default_title(),
            layout: Rc::new(StdCell::new(None)),
            selecting: false,
            scroll_acc: 0.0,
            links: LinkFinder::new(),
            hovered_link: None,
            pointer: None,
            _events: events,
            _subs: subs,
        }
    }

    pub fn cwd(&self) -> Option<PathBuf> {
        self.pty.cwd()
    }

    fn handle_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Wakeup => {
                // Output may have moved or replaced the text under the pointer.
                if self.hovered_link.is_some() {
                    self.update_hovered_link(cx);
                }
                cx.notify()
            }
            Event::Title(title) => {
                self.title = title;
                cx.emit(TerminalEvent::TitleChanged);
            }
            Event::ResetTitle => {
                self.title = default_title();
                cx.emit(TerminalEvent::TitleChanged);
            }
            Event::PtyWrite(s) => self.pty.write(s.into_bytes()),
            Event::ClipboardStore(_, s) => cx.write_to_clipboard(ClipboardItem::new_string(s)),
            Event::ClipboardLoad(_, format) => {
                let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                self.pty.write(format(&text).into_bytes());
            }
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
            Event::ChildExit(_) | Event::Exit => cx.emit(TerminalEvent::Exited),
            _ => {}
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

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        if m.shift && !m.control && !m.alt && (ks.key == "pageup" || ks.key == "pagedown") {
            let scroll = if ks.key == "pageup" { Scroll::PageUp } else { Scroll::PageDown };
            self.pty.term.lock().scroll_display(scroll);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let mode = *self.pty.term.lock().mode();
        if let Some(bytes) = input::to_esc(ks, mode) {
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

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.layout.get() else { return };
        let lines = match ev.delta {
            ScrollDelta::Lines(p) => p.y * 3.0,
            ScrollDelta::Pixels(p) => p.y / layout.line_height,
        };
        self.scroll_acc += lines;
        let n = self.scroll_acc.trunc() as i32;
        if n == 0 {
            return;
        }
        self.scroll_acc -= n as f32;

        let mut term = self.pty.term.lock();
        let mode = *term.mode();
        if mode.intersects(TermMode::MOUSE_MODE) && mode.contains(TermMode::SGR_MOUSE) {
            drop(term);
            let button = if n > 0 { 64 } else { 65 };
            if let Some((p, _)) = self.grid_point(ev.position) {
                let seq = format!("\x1b[<{};{};{}M", button, p.column.0 + 1, p.line.0 + 1);
                for _ in 0..n.unsigned_abs() {
                    self.pty.write(seq.clone().into_bytes());
                }
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
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
        if link_modifier(&ev.modifiers)
            && let Some(link) = self.link_at(ev.position)
        {
            if links::openable(&link.uri) {
                cx.open_url(&link.uri);
            }
            return;
        }
        let Some((point, side)) = self.grid_point(ev.position) else { return };
        let ty = match ev.click_count {
            0 | 1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        let mut term = self.pty.term.lock();
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

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
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

    fn on_middle_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "linux")]
        if let Some(text) = cx.read_from_primary().and_then(|c| c.text()) {
            self.paste_text(&text);
        }
        let _ = cx;
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.pty.term.lock().selection_to_string() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            self.paste_text(&text);
        }
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

/// Ctrl on Linux, Cmd on macOS: held to underline and click links.
fn link_modifier(m: &Modifiers) -> bool {
    if cfg!(target_os = "macos") { m.platform } else { m.control }
}

// ---------------------------------------------------------------------------
// Rendering

struct Frame {
    backgrounds: Vec<(Bounds<Pixels>, Hsla)>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<(Bounds<Pixels>, CursorShape)>,
    cursor_color: Hsla,
    line_height: Pixels,
    /// Target of the hovered link, shown in a pill at the bottom of the pane.
    link_label: Option<(ShapedLine, Hsla, Hsla)>,
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

struct Segment {
    col: usize,
    text: String,
    runs: Vec<TextRun>,
}

impl Segment {
    fn push(&mut self, s: &str, run: TextRun) {
        self.text.push_str(s);
        if let Some(last) = self.runs.last_mut()
            && last.font == run.font
            && last.color == run.color
            && last.underline == run.underline
            && last.strikethrough == run.strikethrough
        {
            last.len += s.len();
            return;
        }
        self.runs.push(TextRun { len: s.len(), ..run });
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = Settings::get(cx);
        let theme = settings.theme().clone();
        let font_size = settings.font_size();
        let line_height_factor = settings.line_height;
        let focused = self.focus.is_focused(window);
        let term = self.pty.term.clone();
        let layout_cell = self.layout.clone();
        let entity = cx.entity();
        let hovered_link = self.hovered_link.clone();

        let prepaint = move |bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App| -> Frame {
            let text_system = window.text_system().clone();
            let base_font = font(MONO_FONT);
            let font_id = text_system.resolve_font(&base_font);
            let cell_width = text_system
                .advance(font_id, font_size, 'm')
                .map(|s| s.width)
                .unwrap_or(px(8.0));
            let line_height = (font_size * line_height_factor).round();
            let cols = ((bounds.size.width / cell_width).floor() as usize).max(2);
            let rows = ((bounds.size.height / line_height).floor() as usize).max(1);

            layout_cell.set(Some(GridLayout {
                origin: bounds.origin,
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
            let cursor_row = cursor.point.line.0 + offset;
            let show_cursor = cursor.shape != CursorShape::Hidden
                && cursor_row >= 0
                && (cursor_row as usize) < rows;

            let mut backgrounds: Vec<(Bounds<Pixels>, Hsla)> = Vec::new();
            let mut segments: Vec<(usize, Segment)> = Vec::new();
            let mut current: Option<(usize, Segment)> = None;
            let mut cursor_wide = false;
            // (row, start_col, end_col, color) for background run merging
            let mut bg_run: Option<(usize, usize, usize, Hsla)> = None;

            let cell_bounds = |row: usize, col: usize, span: usize| {
                Bounds::new(
                    point(
                        bounds.origin.x + cell_width * col as f32,
                        bounds.origin.y + line_height * row as f32,
                    ),
                    size(cell_width * span as f32, line_height),
                )
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

                // Text
                let needs_new = match &current {
                    Some((r, seg)) => *r != row || seg.col + seg.text.chars().count() != col,
                    None => true,
                };
                if needs_new && let Some(seg) = current.take() {
                    segments.push(seg);
                }
                let seg = current.get_or_insert_with(|| {
                    (row, Segment { col, text: String::new(), runs: Vec::new() })
                });

                let mut ch = if flags.contains(Flags::HIDDEN) { ' ' } else { cell.c };
                if ch == '\t' || ch == '\0' {
                    ch = ' ';
                }
                let mut s = String::from(ch);
                if let Some(extra) = cell.zerowidth() {
                    s.extend(extra.iter());
                }
                let font_idx = (flags.contains(Flags::BOLD) as usize)
                    | ((flags.contains(Flags::ITALIC) as usize) << 1);
                let underline =
                    (in_link || flags.intersects(Flags::ALL_UNDERLINES)).then(|| UnderlineStyle {
                        thickness: px(1.0),
                        color: Some(fg),
                        wavy: !in_link && flags.contains(Flags::UNDERCURL),
                    });
                let strikethrough = flags
                    .contains(Flags::STRIKEOUT)
                    .then(|| StrikethroughStyle { thickness: px(1.0), color: Some(fg) });
                seg.1.push(
                    &s,
                    TextRun {
                        len: 0,
                        font: fonts[font_idx].clone(),
                        color: fg,
                        background_color: None,
                        underline,
                        strikethrough,
                    },
                );
                if wide {
                    // Wide glyphs get their own segment so following text stays on the grid.
                    if let Some(seg) = current.take() {
                        segments.push(seg);
                    }
                }
            }
            if let Some(seg) = current.take() {
                segments.push(seg);
            }
            if let Some((r, start, end, c)) = bg_run.take() {
                backgrounds.push((cell_bounds(r, start, end - start), c));
            }

            let lines = segments
                .into_iter()
                .filter(|(_, seg)| !seg.text.trim_end().is_empty())
                .map(|(row, seg)| {
                    let origin = cell_bounds(row, seg.col, 1).origin;
                    let shaped = text_system.shape_line(
                        SharedString::from(seg.text),
                        font_size,
                        &seg.runs,
                        Some(cell_width),
                    );
                    (origin, shaped)
                })
                .collect();

            let cursor = show_cursor.then(|| {
                let span = if cursor_wide { 2 } else { 1 };
                (cell_bounds(cursor_row as usize, cursor.point.column.0, span), cursor.shape)
            });

            let link_label = hovered_link.as_ref().map(|link| {
                let mut uri = link.uri.clone();
                if uri.chars().count() > 120 {
                    uri = uri.chars().take(119).chain(['…']).collect();
                }
                if !links::openable(&link.uri) {
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
                lines,
                cursor,
                cursor_color: theme.cursor,
                line_height,
                link_label,
            }
        };

        let paint = move |bounds: Bounds<Pixels>, frame: Frame, window: &mut Window, cx: &mut App| {
            for (b, color) in &frame.backgrounds {
                window.paint_quad(fill(*b, *color));
            }
            let cursor_color = frame.cursor_color;
            if let Some((b, CursorShape::Block)) = frame.cursor
                && focused
            {
                window.paint_quad(fill(b, cursor_color).corner_radii(px(2.0)));
            }
            for (origin, line) in &frame.lines {
                let _ = line.paint(*origin, frame.line_height, window, cx);
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
            .when(self.hovered_link.is_none(), |d| d.cursor_text())
            .on_hover(cx.listener(Self::on_hover))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_middle_click))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .child(canvas(prepaint, paint).size_full())
    }
}
