//! Keyboard-first command palette.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, BoxShadow, Context, EventEmitter, FocusHandle, Focusable, HighlightStyle, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, StyledText, Window, div, point, prelude::FluentBuilder, px,
};

use crate::settings::{MONO_FONT, Settings, UI_FONT};
use crate::theme::alpha;

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
    pub run: PaletteFn,
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
    /// Turned on, or current.
    On,
    /// Turned off: an empty ring.
    Off,
    /// Running fine.
    Good,
    /// Starting, restarting or waiting.
    Busy,
    /// Failed or stopped.
    Bad,
}

impl PaletteItem {
    pub fn new(label: impl Into<SharedString>, run: PaletteFn) -> Self {
        Self { label: label.into(), hint: Hint::None, dot: None, run }
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
}

pub enum PaletteEvent {
    Dismiss,
}

pub struct CommandPalette {
    focus: FocusHandle,
    query: String,
    selected: usize,
    items: Vec<PaletteItem>,
    /// The items matching the query, best first, with the byte ranges of
    /// their labels that matched.
    filtered: Vec<(usize, Vec<Range<usize>>)>,
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
    scroll: gpui::ScrollHandle,
    /// Each item's hint, cut to the room its label leaves: measured once,
    /// on the first draw, since the items don't change.
    hints: Option<Vec<Option<SharedString>>>,
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
        let filtered = (0..items.len()).map(|i| (i, Vec::new())).collect();
        Self {
            focus: cx.focus_handle(),
            scroll: gpui::ScrollHandle::new(),
            hints: None,
            query: String::new(),
            selected: 0,
            items,
            filtered,
            placeholder,
            on_cancel,
            done: false,
            message: None,
            submit: None,
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
        let mut scored: Vec<(usize, usize, Vec<Range<usize>>)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| fuzzy_match(&self.query, &item.label).map(|(s, m)| (s, i, m)))
            .collect();
        scored.sort_by_key(|&(s, i, _)| (s, i));
        self.filtered = scored.into_iter().map(|(_, i, m)| (i, m)).collect();
        self.selected = 0;
        self.scroll.scroll_to_item(0);
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(submit) = self.submit.clone() {
            let text = self.query.trim().to_string();
            self.done = true;
            cx.emit(PaletteEvent::Dismiss);
            window.defer(cx, move |window, cx| submit(text, window, cx));
            return;
        }
        let Some(&(idx, _)) = self.filtered.get(self.selected) else { return };
        let run = self.items[idx].run.clone();
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
                self.selected = self.selected.checked_sub(1).unwrap_or(self.filtered.len().saturating_sub(1));
                self.scroll.scroll_to_item(self.selected);
            }
            "down" | "tab" => {
                if !self.filtered.is_empty() {
                    self.selected = (self.selected + 1) % self.filtered.len();
                    self.scroll.scroll_to_item(self.selected);
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
        cx.notify();
    }
}

/// Rows are this tall, and this many show before the list scrolls.
const ROW_HEIGHT: f32 = 34.0;
const VISIBLE_ROWS: f32 = 12.0;
/// Inside a row: the palette, its border, the row's margins and padding.
const ROW_ROOM: f32 = 560.0 - 2.0 - 12.0 - 24.0;

/// A text run in `family`, for measuring.
fn run(text: &str, family: &'static str) -> gpui::TextRun {
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
        Hint::Keys(keys) => (keys, MONO_FONT, 11.5),
        Hint::Note(note) => (note, UI_FONT, 12.0),
    };
    let width = |t: &SharedString, family, size| {
        f32::from(window.text_system().shape_line(t.clone(), px(size), &[run(t, family)], None).width)
    };
    let label = width(&item.label, UI_FONT, 13.5).min(380.0);
    let room = ROW_ROOM - label - 12.0 - if dots { 16.0 } else { 0.0 } - 2.0;
    if room < 40.0 {
        return None;
    }
    if width(text, family, size) <= room {
        return Some(text.clone());
    }
    let mut runs = vec![run(text, family)];
    Some(cx.text_system().line_wrapper(gpui::font(family), px(size)).truncate_line(text.clone(), px(room), "…", &mut runs))
}

/// How far below the top of the window the palette sits.
pub const PALETTE_TOP: f32 = 72.0;

fn caret(color: gpui::Hsla) -> gpui::Div {
    div().w(px(2.0)).h(px(18.0)).mx(px(1.0)).rounded(px(1.0)).bg(color)
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Settings::get(cx).theme().clone();
        let has_query = !self.query.is_empty();
        let query: SharedString = if has_query {
            self.query.clone().into()
        } else {
            self.placeholder.clone()
        };

        // A column for dots when any item has one, so labels line up.
        let dots = self.items.iter().any(|i| i.dot.is_some());
        if self.hints.is_none() {
            self.hints = Some(self.items.iter().map(|item| fit_hint(item, dots, window, cx)).collect());
        }
        let hints = self.hints.as_ref().expect("measured above");
        let rows = self.filtered.iter().enumerate().map(|(row, (idx, matched))| {
            let item = &self.items[*idx];
            let selected = row == self.selected;
            // The letters you typed, in the accent colour.
            let highlight = HighlightStyle { color: Some(theme.accent), ..Default::default() };
            let label = StyledText::new(item.label.clone())
                .with_highlights(matched.iter().map(|r| (r.clone(), highlight)));
            let dot = item.dot.map(|dot| {
                let d = div().size(px(7.0)).rounded(px(4.0));
                match dot {
                    Dot::Off => d.border_1().border_color(theme.muted),
                    Dot::On => d.bg(theme.accent),
                    Dot::Good => d.bg(theme.ansi[2]),
                    Dot::Busy => d.bg(theme.ansi[3]),
                    Dot::Bad => d.bg(theme.ansi[1]),
                }
            });
            let hint = hints[*idx].clone().map(|text| match &item.hint {
                Hint::Keys(_) => div().font_family(MONO_FONT).text_size(px(11.5)).text_color(theme.subtext).child(text),
                _ => div().text_size(px(12.0)).text_color(theme.muted).child(text),
            });
            div()
                .id(("palette-item", row))
                .flex()
                .items_center()
                .justify_between()
                .h(px(ROW_HEIGHT))
                .flex_none()
                .px(px(12.0))
                .mx(px(6.0))
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
                        .when(dots, |d| {
                            d.child(div().flex().flex_none().w(px(7.0)).justify_center().children(dot))
                        })
                        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(label)),
                )
                .children(hint.map(|h| h.flex_none().whitespace_nowrap()))
        });

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
            .font_family(UI_FONT)
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
                div()
                    .id("palette-list")
                    .mt(px(6.0))
                    .flex()
                    .flex_col()
                    .flex_shrink()
                    .min_h_0()
                    .max_h(px(ROW_HEIGHT * VISIBLE_ROWS))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows),
            )
            .when(self.filtered.is_empty() && self.submit.is_none(), |d| {
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

    #[test]
    fn a_run_beats_scattered_letters_and_earlier_beats_later() {
        let score = |q, t| fuzzy_match(q, t).unwrap().0;
        assert!(score("tab", "New Tab") < score("tab", "Toggle Area Box"));
        assert!(score("pane", "Pane Log") < score("pane", "Next Pane"));
    }
}
