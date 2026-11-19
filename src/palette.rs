//! Keyboard-first command palette.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, BoxShadow, Context, EventEmitter, FocusHandle, Focusable, HighlightStyle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, StyledText, Window, div,
    point, prelude::FluentBuilder, px,
};

use crate::settings::{MONO_FONT, Settings, ui_font};
use crate::theme::alpha;
use gpui::{ScrollStrategy, uniform_list};

/// Something the palette runs: a choice, or what happens on cancel.
pub type PaletteFn = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a text prompt does with what you typed.
pub type SubmitFn = Rc<dyn Fn(String, &mut Window, &mut App)>;

#[derive(Clone)]
pub struct PaletteItem {
    pub label: SharedString,
    /// What goes on the right: the item's shortcut, or a dimmer note.
    pub hint: Hint,
    /// A coloured dot before the label: on or off, running or failed.
    pub dot: Option<Dot>,
    /// The heading it's listed under ("Lumen", "Plugins"); none for pickers.
    pub section: Option<&'static str>,
    pub run: PaletteFn,
}

/// One row of the list: a section heading, or an item (its index and the
/// byte ranges of its label that matched).
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Header(&'static str),
    Item(usize, Vec<Range<usize>>),
}

/// The rows for `query`: the matching items, best first within each section,
/// each under its heading. Sections come in the order they first appear,
/// except that while typing, the one with the best match comes first. Items
/// without a section get no heading.
pub fn arrange(items: &[PaletteItem], query: &str) -> Vec<Row> {
    let mut order: Vec<Option<&'static str>> = Vec::new();
    for item in items {
        if !order.contains(&item.section) {
            order.push(item.section);
        }
    }
    let mut scored: Vec<(usize, usize, usize, Vec<Range<usize>>)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let (score, m) = fuzzy_match(query, &item.label)?;
            let group = order.iter().position(|s| *s == item.section).unwrap_or(0);
            Some((group, score, i, m))
        })
        .collect();
    // While typing, the section holding the best match comes first, so Enter
    // takes it; ties (and an empty query) keep the sections' own order.
    let mut best = vec![usize::MAX; order.len()];
    for &(g, s, _, _) in &scored {
        best[g] = best[g].min(s);
    }
    scored.sort_by_key(|&(g, s, i, _)| (best[g], g, s, i));
    let mut rows = Vec::with_capacity(scored.len() + order.len());
    let mut last = None;
    for (g, _, i, m) in scored {
        if last != Some(g) {
            last = Some(g);
            if let Some(name) = order[g] {
                rows.push(Row::Header(name));
            }
        }
        rows.push(Row::Item(i, m));
    }
    rows
}

/// The item at row `row`, unless that's a heading.
pub fn highlighted(rows: &[Row], row: usize) -> Option<usize> {
    match rows.get(row) {
        Some(Row::Item(i, _)) => Some(*i),
        _ => None,
    }
}

/// The row showing item `item`, if it's shown.
pub fn row_of(rows: &[Row], item: usize) -> Option<usize> {
    rows.iter().position(|r| matches!(r, Row::Item(i, _) if *i == item))
}

/// Told the item now highlighted, as the selection moves.
pub type HighlightFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The first item row, if any.
pub fn first_item(rows: &[Row]) -> Option<usize> {
    rows.iter().position(|r| matches!(r, Row::Item(..)))
}

/// The item row after (or before) row `from`, skipping headings and
/// wrapping around; `from` itself if there's no other.
pub fn step(rows: &[Row], from: usize, down: bool) -> usize {
    let n = rows.len();
    if n == 0 {
        return 0;
    }
    let mut at = from.min(n - 1);
    for _ in 0..n {
        at = if down { (at + 1) % n } else { (at + n - 1) % n };
        if matches!(rows[at], Row::Item(..)) {
            return at;
        }
    }
    from
}

#[derive(Clone, Default)]
pub enum Hint {
    #[default]
    None,
    /// A key binding, in the monospace font.
    Keys(SharedString),
    /// Anything else (where a command comes from, a path, a status), dimmer.
    Note(SharedString),
}

/// A state shown as a dot before an item's label.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dot {
    /// Running fine.
    Good,
    /// Starting, restarting or waiting.
    Busy,
    /// Failed or stopped.
    Bad,
    /// A colour from the theme's 16: a swatch.
    Ansi(u8),
    /// An empty ring: no colour.
    Hollow,
}

impl PaletteItem {
    pub fn new(label: impl Into<SharedString>, run: PaletteFn) -> Self {
        Self { label: label.into(), hint: Hint::None, dot: None, section: None, run }
    }

    /// Show this shortcut on the right (none if empty).
    pub fn keys(mut self, keys: impl Into<SharedString>) -> Self {
        let keys = keys.into();
        self.hint = if keys.is_empty() { Hint::None } else { Hint::Keys(keys) };
        self
    }

    /// Show this note on the right (none if empty).
    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        let note = note.into();
        self.hint = if note.is_empty() { Hint::None } else { Hint::Note(note) };
        self
    }

    pub fn dot(mut self, dot: Dot) -> Self {
        self.dot = Some(dot);
        self
    }

    /// List it under this heading.
    pub fn section(mut self, section: &'static str) -> Self {
        self.section = Some(section);
        self
    }
}

pub enum PaletteEvent {
    Dismiss,
}

pub struct CommandPalette {
    focus: FocusHandle,
    query: String,
    selected: usize,
    items: Vec<PaletteItem>,
    /// The items matching the query under their headings (see [`arrange`]).
    filtered: Vec<Row>,
    /// Shown while nothing is typed.
    placeholder: SharedString,
    /// Run when the palette closes without a choice (pickers answer "none").
    on_cancel: Option<PaletteFn>,
    /// An item was chosen.
    pub done: bool,
    /// Text shown under the input, for prompts that need explaining.
    message: Option<SharedString>,
    /// Set for a text prompt: Enter hands it the typed text instead of
    /// choosing an item.
    submit: Option<SubmitFn>,
    /// Keeps the selected row in view when the list scrolls.
    scroll: gpui::UniformListScrollHandle,
    /// Each item's hint, cut to the room its label leaves: measured the first
    /// time its row is drawn, since the items don't change.
    hints: Vec<Option<Option<SharedString>>>,
    /// Told whenever a different item is highlighted (a theme previewing).
    on_highlight: Option<HighlightFn>,
    /// The item last reported to `on_highlight`.
    reported: Option<usize>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Case-insensitive match of `query` in `text`: as one run (scored by where
/// it starts), else as a subsequence (scored by the gaps, after every run).
/// Lower is better. Also gives the byte ranges of `text` that matched.
fn fuzzy_match(query: &str, text: &str) -> Option<(usize, Vec<Range<usize>>)> {
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    let query: Vec<char> = query.chars().map(fold).collect();
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<(usize, char)> = text.char_indices().map(|(i, c)| (i, fold(c))).collect();
    let end = |k: usize| chars.get(k).map_or(text.len(), |&(i, _)| i);
    // One run.
    if let Some(start) = (0..chars.len()).find(|&s| {
        s + query.len() <= chars.len() && chars[s..s + query.len()].iter().zip(&query).all(|((_, c), q)| c == q)
    }) {
        let run = chars[start].0..end(start + query.len());
        return Some((start, std::iter::once(run).collect()));
    }
    // A subsequence: each character at its first place after the last.
    let mut score = 100;
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let (mut k, mut last) = (0, 0);
    for q in &query {
        k += chars[k..].iter().position(|(_, c)| c == q)?;
        score += k - last;
        last = k;
        let range = chars[k].0..end(k + 1);
        match ranges.last_mut() {
            Some(prev) if prev.end == range.start => prev.end = range.end,
            _ => ranges.push(range),
        }
        k += 1;
    }
    Some((score, ranges))
}

impl CommandPalette {
    pub fn new(items: Vec<PaletteItem>, cx: &mut Context<Self>) -> Self {
        Self::picker(items, "Type a command…".into(), None, cx)
    }

    /// A palette offering `items`, for plugins and prompts.
    pub fn picker(
        items: Vec<PaletteItem>,
        placeholder: SharedString,
        on_cancel: Option<PaletteFn>,
        cx: &mut Context<Self>,
    ) -> Self {
        let filtered = arrange(&items, "");
        let selected = first_item(&filtered).unwrap_or(0);
        Self {
            focus: cx.focus_handle(),
            scroll: gpui::UniformListScrollHandle::new(),
            hints: Vec::new(),
            query: String::new(),
            selected,
            items,
            filtered,
            placeholder,
            on_cancel,
            done: false,
            message: None,
            submit: None,
            on_highlight: None,
            reported: None,
        }
    }

    /// Start with item `item` highlighted.
    pub fn with_selected(mut self, item: usize) -> Self {
        if let Some(row) = row_of(&self.filtered, item) {
            self.selected = row;
            self.scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
        self
    }

    /// Tell `f` each item highlighted as the selection moves.
    pub fn with_highlight(mut self, f: HighlightFn) -> Self {
        self.reported = highlighted(&self.filtered, self.selected);
        self.on_highlight = Some(f);
        self
    }

    /// Report the highlighted item if it changed.
    fn report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = highlighted(&self.filtered, self.selected);
        if now == self.reported {
            return;
        }
        self.reported = now;
        if let (Some(f), Some(item)) = (self.on_highlight.clone(), now) {
            f(item, window, cx);
        }
    }

    /// A prompt for a line of text, starting with `initial`.
    pub fn prompt(placeholder: SharedString, initial: String, submit: SubmitFn, cx: &mut Context<Self>) -> Self {
        let mut p = Self::picker(Vec::new(), placeholder, None, cx);
        p.query = initial;
        p.submit = Some(submit);
        p
    }

    /// What to run if the palette closes without a choice.
    pub fn with_cancel(mut self, cancel: PaletteFn) -> Self {
        self.on_cancel = Some(cancel);
        self
    }

    /// Explain what's being asked, under the input.
    pub fn with_message(mut self, message: impl Into<SharedString>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// What to run if the palette closes without a choice.
    pub fn on_cancel(&self) -> Option<PaletteFn> {
        if self.done { None } else { self.on_cancel.clone() }
    }

    fn refilter(&mut self) {
        self.filtered = arrange(&self.items, &self.query);
        self.selected = first_item(&self.filtered).unwrap_or(0);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(submit) = self.submit.clone() {
            let text = self.query.trim().to_string();
            self.done = true;
            cx.emit(PaletteEvent::Dismiss);
            window.defer(cx, move |window, cx| submit(text, window, cx));
            return;
        }
        let Some(Row::Item(idx, _)) = self.filtered.get(self.selected) else { return };
        let run = self.items[*idx].run.clone();
        self.done = true;
        cx.emit(PaletteEvent::Dismiss);
        // Run once this palette is closed and no longer being updated: a choice
        // may open another palette, which reads this one to close it.
        window.defer(cx, move |window, cx| run(window, cx));
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        cx.stop_propagation();
        match ks.key.as_str() {
            "escape" => cx.emit(PaletteEvent::Dismiss),
            "enter" => self.confirm(window, cx),
            "up" => {
                let before = self.selected;
                self.selected = step(&self.filtered, before, false);
                // Up shows the row at the top (with its heading, if it heads a
                // section), unless it wrapped to the end.
                if self.selected > before {
                    self.scroll.scroll_to_item(self.selected, ScrollStrategy::Bottom);
                } else {
                    let heading = self.selected.checked_sub(1).filter(|&r| matches!(self.filtered[r], Row::Header(_)));
                    self.scroll.scroll_to_item(heading.unwrap_or(self.selected), ScrollStrategy::Top);
                }
            }
            "down" | "tab" => {
                let before = self.selected;
                self.selected = step(&self.filtered, before, true);
                // Wrapping to the top shows the first heading too.
                if self.selected < before {
                    self.scroll.scroll_to_item(0, ScrollStrategy::Top);
                } else {
                    self.scroll.scroll_to_item(self.selected, ScrollStrategy::Bottom);
                }
            }
            "backspace" => {
                self.query.pop();
                self.refilter();
            }
            // Clear the line, as in a shell.
            "u" if ks.modifiers.control && !ks.modifiers.shift => {
                self.query.clear();
                self.refilter();
            }
            _ => {
                if ks.modifiers.control || ks.modifiers.platform || ks.modifiers.alt {
                    return;
                }
                if let Some(ch) = &ks.key_char {
                    self.query.push_str(ch);
                    self.refilter();
                }
            }
        }
        if !self.done {
            self.report(window, cx);
        }
        cx.notify();
    }
}

/// Rows are this tall, and this many show before the list scrolls.
const ROW_HEIGHT: f32 = 34.0;
const VISIBLE_ROWS: f32 = 12.0;
/// Inside a row: the palette, its border, the row's margins and padding.
const ROW_ROOM: f32 = 560.0 - 2.0 - 12.0 - 24.0;

/// A text run in `family`, for measuring.
fn run(text: &str, family: SharedString) -> gpui::TextRun {
    gpui::TextRun {
        len: text.len(),
        font: gpui::font(family),
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// `item`'s hint as it fits beside its label: whole, cut with "…", or none
/// when there's hardly room. Measured, because GPUI's own ellipsis is placed
/// before flexbox shrinks the text, so it clips instead.
fn fit_hint(item: &PaletteItem, dots: bool, window: &Window, cx: &App) -> Option<SharedString> {
    let (text, family, size) = match &item.hint {
        Hint::None => return None,
        Hint::Keys(keys) => (keys, SharedString::from(MONO_FONT), 11.5),
        Hint::Note(note) => (note, ui_font(), 12.0),
    };
    let width = |t: &SharedString, family: SharedString, size| {
        f32::from(window.text_system().shape_line(t.clone(), px(size), &[run(t, family)], None).width)
    };
    let label = width(&item.label, ui_font(), 13.5).min(380.0);
    let room = ROW_ROOM - label - 12.0 - if dots { 16.0 } else { 0.0 } - 2.0;
    if room < 40.0 {
        return None;
    }
    if width(text, family.clone(), size) <= room {
        return Some(text.clone());
    }
    let mut runs = vec![run(text, family.clone())];
    Some(cx.text_system().line_wrapper(gpui::font(family), px(size)).truncate_line(
        text.clone(),
        px(room),
        "…",
        &mut runs,
    ))
}

/// One row of the palette.
#[allow(clippy::too_many_arguments)]
fn row_element(
    row: usize,
    item: &PaletteItem,
    matched: &[Range<usize>],
    fitted: Option<SharedString>,
    dots: bool,
    selected: bool,
    theme: &crate::theme::Theme,
    cx: &mut Context<CommandPalette>,
) -> gpui::AnyElement {
    // The letters you typed, in the accent colour.
    let highlight = HighlightStyle { color: Some(theme.accent), ..Default::default() };
    let label = StyledText::new(item.label.clone()).with_highlights(matched.iter().map(|r| (r.clone(), highlight)));
    let dot = item.dot.map(|dot| {
        let d = div().size(px(7.0)).rounded(px(4.0));
        match dot {
            Dot::Good => d.bg(theme.ansi[2]),
            Dot::Busy => d.bg(theme.ansi[3]),
            Dot::Bad => d.bg(theme.ansi[1]),
            Dot::Ansi(i) => d.bg(theme.ansi[usize::from(i) % 16]),
            Dot::Hollow => d.border_1().border_color(theme.muted),
        }
    });
    let hint = fitted.map(|text| match &item.hint {
        Hint::Keys(_) => div().font_family(MONO_FONT).text_size(px(11.5)).text_color(theme.subtext).child(text),
        _ => div().text_size(px(12.0)).text_color(theme.muted).child(text),
    });
    let row_div = div()
        .id(("palette-item", row))
        .flex()
        .flex_1()
        .items_center()
        .justify_between()
        .h(px(ROW_HEIGHT))
        .px(px(12.0))
        .rounded(px(7.0))
        .text_color(if selected { theme.text } else { theme.subtext })
        .when(selected, |d| d.bg(alpha(theme.accent, 0.16)))
        .hover(|d| d.bg(alpha(theme.accent, 0.10)))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.selected = row;
            this.confirm(window, cx);
        }))
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(9.0))
                .max_w(px(380.0))
                .when(dots, |d| d.child(div().flex().flex_none().w(px(7.0)).justify_center().children(dot)))
                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label)),
        )
        .children(hint.map(|h| h.flex_none().whitespace_nowrap()));
    // The list sizes rows to their content; this one spans its width.
    div().flex().w_full().px(px(6.0)).child(row_div).into_any_element()
}

/// A section heading: small capitals at the foot of a row's height, so it
/// sits close to the items it heads.
fn heading_element(name: &str, theme: &crate::theme::Theme) -> gpui::AnyElement {
    div()
        .flex()
        .items_end()
        .h(px(ROW_HEIGHT))
        .px(px(18.0))
        .pb(px(5.0))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.muted)
        .child(name.to_uppercase())
        .into_any_element()
}

/// How far below the top of the window the palette sits.
pub const PALETTE_TOP: f32 = 72.0;

fn caret(color: gpui::Hsla) -> gpui::Div {
    div().w(px(2.0)).h(px(18.0)).mx(px(1.0)).rounded(px(1.0)).bg(color)
}

impl CommandPalette {
    /// The rows `range` of the filtered list.
    fn rows(
        &mut self,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let theme = Settings::get(cx).theme().clone();
        // A column for dots when any item has one, so labels line up.
        let dots = self.items.iter().any(|i| i.dot.is_some());
        if self.hints.len() != self.items.len() {
            self.hints = vec![None; self.items.len()];
        }
        let mut out = Vec::with_capacity(range.len());
        for row in range {
            let Some(shown) = self.filtered.get(row) else { break };
            let (idx, matched) = match shown {
                Row::Header(name) => {
                    out.push(heading_element(name, &theme));
                    continue;
                }
                Row::Item(idx, matched) => (*idx, matched),
            };
            if self.hints[idx].is_none() {
                self.hints[idx] = Some(fit_hint(&self.items[idx], dots, window, cx));
            }
            let item = &self.items[idx];
            let fitted = self.hints[idx].clone().flatten();
            out.push(row_element(row, item, matched, fitted, dots, row == self.selected, &theme, cx));
        }
        out
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Settings::get(cx).theme().clone();
        let has_query = !self.query.is_empty();
        let query: SharedString = if has_query { self.query.clone().into() } else { self.placeholder.clone() };

        div()
            .id("palette")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(560.0))
            // Fit in the window below its top offset, leaving a margin; the
            // list shrinks and scrolls to make room.
            .max_h((window.viewport_size().height - px(PALETTE_TOP + 48.0)).max(px(160.0)))
            .flex()
            .flex_col()
            .pb(px(6.0))
            .font_family(ui_font())
            .text_size(px(13.5))
            .bg(theme.raised)
            .border_1()
            .border_color(alpha(theme.text, 0.08))
            .rounded(px(14.0))
            .shadow(vec![BoxShadow {
                color: alpha(gpui::black(), 0.45),
                offset: point(px(0.0), px(18.0)),
                blur_radius: px(48.0),
                spread_radius: px(0.0),
            }])
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(50.0))
                    .flex_none()
                    .px(px(18.0))
                    .text_size(px(15.0))
                    .border_b_1()
                    .border_color(alpha(theme.text, 0.06))
                    .child(div().text_color(theme.accent).child("›"))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .items_center()
                            .text_color(if has_query { theme.text } else { theme.muted })
                            .when(!has_query, |d| d.child(caret(theme.accent)))
                            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(query))
                            .when(has_query, |d| d.child(caret(theme.accent))),
                    ),
            )
            .when_some(self.message.clone(), |d, message| {
                d.child(
                    div()
                        .flex_none()
                        .px(px(18.0))
                        .pt(px(12.0))
                        .pb(px(4.0))
                        .text_size(px(13.0))
                        .line_height(px(19.0))
                        .text_color(theme.subtext)
                        .child(message),
                )
            })
            .child(
                div().id("palette-list").mt(px(6.0)).flex().flex_col().flex_shrink().min_h_0().child(
                    // Only the rows in view are built and drawn.
                    uniform_list("palette-rows", self.filtered.len(), cx.processor(Self::rows))
                        .track_scroll(self.scroll.clone())
                        .h(px(ROW_HEIGHT * (self.filtered.len() as f32).min(VISIBLE_ROWS)))
                        .flex_shrink()
                        .min_h_0(),
                ),
            )
            .when(first_item(&self.filtered).is_none() && self.submit.is_none(), |d| {
                d.child(div().px(px(18.0)).py(px(10.0)).text_color(theme.muted).child("No matches"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched<'a>(query: &str, text: &'a str) -> Option<Vec<&'a str>> {
        fuzzy_match(query, text).map(|(_, ranges)| ranges.into_iter().map(|r| &text[r]).collect())
    }

    #[test]
    fn matches_say_which_letters_matched() {
        assert_eq!(matched("pomo", "Start Pomodoro"), Some(vec!["Pomo"]));
        assert_eq!(matched("nwt", "New Tab"), Some(vec!["N", "w", "T"]));
        assert_eq!(matched("ntab", "New Tab"), Some(vec!["N", "Tab"]));
        assert_eq!(matched("", "New Tab"), Some(vec![]));
        assert_eq!(matched("xyz", "New Tab"), None);
        // Ranges are bytes of the label as shown, past multi-byte letters.
        assert_eq!(matched("pine", "Theme: Rosé Pine"), Some(vec!["Pine"]));
        assert_eq!(matched("rp", "Theme: Rosé Pine"), Some(vec!["R", "P"]));
        assert_eq!(matched("É", "rosé"), Some(vec!["é"]));
    }

    fn item(section: Option<&'static str>, label: &str) -> PaletteItem {
        let i = PaletteItem::new(label.to_string(), Rc::new(|_, _| {}));
        match section {
            Some(s) => i.section(s),
            None => i,
        }
    }

    fn shape(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Header(h) => format!("# {h}"),
                Row::Item(i, _) => i.to_string(),
            })
            .collect()
    }

    #[test]
    fn sections_group_matches_in_order_of_first_appearance() {
        let items =
            [item(Some("Lumen"), "Next Pane"), item(Some("Plugins"), "Pane Log"), item(Some("Lumen"), "Pane Split")];
        // Within a section: best match first ("Pane Split" starts with it).
        assert_eq!(shape(&arrange(&items, "pane")), ["# Lumen", "2", "0", "# Plugins", "1"]);
        // Nothing typed: the order given, under headings.
        assert_eq!(shape(&arrange(&items, "")), ["# Lumen", "0", "2", "# Plugins", "1"]);
    }

    #[test]
    fn the_section_with_the_best_match_comes_first() {
        let items = [item(Some("Lumen"), "Reset Font Size"), item(Some("Plugins"), "RSS Reader")];
        // "rss" is a run in RSS Reader but scattered in Reset Font Size:
        // Enter must take RSS Reader.
        assert_eq!(shape(&arrange(&items, "rss")), ["# Plugins", "1", "# Lumen", "0"]);
        // Nothing typed: the sections in their own order.
        assert_eq!(shape(&arrange(&items, "")), ["# Lumen", "0", "# Plugins", "1"]);
    }

    #[test]
    fn a_section_without_matches_has_no_heading() {
        let items = [item(Some("Lumen"), "New Tab"), item(Some("Plugins"), "Start Pomodoro")];
        assert_eq!(shape(&arrange(&items, "pomo")), ["# Plugins", "1"]);
        assert_eq!(shape(&arrange(&items, "zzz")), Vec::<String>::new());
    }

    #[test]
    fn items_without_sections_have_no_headings() {
        let items = [item(None, "Yes"), item(None, "No")];
        assert_eq!(shape(&arrange(&items, "")), ["0", "1"]);
    }

    #[test]
    fn the_highlighted_item_is_the_selected_row_s() {
        let items = [item(Some("Lumen"), "A"), item(Some("Plugins"), "B")];
        let rows = arrange(&items, "");
        assert_eq!(highlighted(&rows, 1), Some(0));
        assert_eq!(highlighted(&rows, 3), Some(1));
        assert_eq!(highlighted(&rows, 0), None, "a heading is no item");
        assert_eq!(row_of(&rows, 1), Some(3));
        assert_eq!(row_of(&rows, 7), None);
    }

    #[test]
    fn moving_skips_headings_and_wraps() {
        let items = [item(Some("Lumen"), "A"), item(Some("Plugins"), "B"), item(Some("Plugins"), "C")];
        let rows = arrange(&items, "");
        // # Lumen, A, # Plugins, B, C
        assert_eq!(first_item(&rows), Some(1));
        assert_eq!(step(&rows, 1, true), 3);
        assert_eq!(step(&rows, 3, true), 4);
        assert_eq!(step(&rows, 4, true), 1, "wraps past the first heading");
        assert_eq!(step(&rows, 1, false), 4, "wraps up to the last item");
        assert_eq!(step(&rows, 3, false), 1);
        assert_eq!(first_item(&[]), None);
    }

    #[test]
    fn a_run_beats_scattered_letters_and_earlier_beats_later() {
        let score = |q, t| fuzzy_match(q, t).unwrap().0;
        assert!(score("tab", "New Tab") < score("tab", "Toggle Area Box"));
        assert!(score("pane", "Pane Log") < score("pane", "Next Pane"));
    }
}
