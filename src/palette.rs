//! Keyboard-first command palette.

use std::rc::Rc;

use gpui::{
    App, BoxShadow, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, div, point, prelude::FluentBuilder, px,
};

use crate::settings::{MONO_FONT, Settings, UI_FONT};
use crate::theme::alpha;

#[derive(Clone)]
pub struct PaletteItem {
    pub label: SharedString,
    pub detail: SharedString,
    pub run: Rc<dyn Fn(&mut Window, &mut App)>,
}

pub enum PaletteEvent {
    Dismiss,
}

pub struct CommandPalette {
    focus: FocusHandle,
    query: String,
    selected: usize,
    items: Vec<PaletteItem>,
    filtered: Vec<usize>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Case-insensitive subsequence match; lower score is better.
fn fuzzy_score(query: &str, text: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    let text = text.to_lowercase();
    if let Some(pos) = text.find(query) {
        return Some(pos);
    }
    let mut score = 100;
    let mut chars = text.char_indices();
    let mut last = 0;
    for q in query.chars() {
        let (i, _) = chars.by_ref().find(|(_, c)| *c == q)?;
        score += i - last;
        last = i;
    }
    Some(score)
}

impl CommandPalette {
    pub fn new(items: Vec<PaletteItem>, cx: &mut Context<Self>) -> Self {
        let filtered = (0..items.len()).collect();
        Self { focus: cx.focus_handle(), query: String::new(), selected: 0, items, filtered }
    }

    fn refilter(&mut self) {
        let q = self.query.to_lowercase();
        let mut scored: Vec<(usize, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| fuzzy_score(&q, &item.label).map(|s| (s, i)))
            .collect();
        scored.sort();
        self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = 0;
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&idx) = self.filtered.get(self.selected) else { return };
        let run = self.items[idx].run.clone();
        cx.emit(PaletteEvent::Dismiss);
        run(window, cx);
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        cx.stop_propagation();
        match ks.key.as_str() {
            "escape" => cx.emit(PaletteEvent::Dismiss),
            "enter" => self.confirm(window, cx),
            "up" => {
                self.selected = self.selected.checked_sub(1).unwrap_or(self.filtered.len().saturating_sub(1));
            }
            "down" | "tab" => {
                if !self.filtered.is_empty() {
                    self.selected = (self.selected + 1) % self.filtered.len();
                }
            }
            "backspace" => {
                self.query.pop();
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

fn caret(color: gpui::Hsla) -> gpui::Div {
    div().w(px(2.0)).h(px(18.0)).mx(px(1.0)).rounded(px(1.0)).bg(color)
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Settings::get(cx).theme().clone();
        let has_query = !self.query.is_empty();
        let query: SharedString = if has_query {
            self.query.clone().into()
        } else {
            "Type a command…".into()
        };

        let rows = self.filtered.iter().take(12).enumerate().map(|(row, &idx)| {
            let item = &self.items[idx];
            let selected = row == self.selected;
            div()
                .id(("palette-item", row))
                .flex()
                .items_center()
                .justify_between()
                .h(px(34.0))
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
                .child(item.label.clone())
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_family(MONO_FONT)
                        .text_color(theme.muted)
                        .child(item.detail.clone()),
                )
        });

        div()
            .id("palette")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(560.0))
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
                    .px(px(18.0))
                    .text_size(px(15.0))
                    .border_b_1()
                    .border_color(alpha(theme.text, 0.06))
                    .child(div().text_color(theme.accent).child("›"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .text_color(if has_query { theme.text } else { theme.muted })
                            .when(!has_query, |d| d.child(caret(theme.accent)))
                            .child(query)
                            .when(has_query, |d| d.child(caret(theme.accent))),
                    ),
            )
            .child(div().pt(px(6.0)).flex().flex_col().children(rows))
            .when(self.filtered.is_empty(), |d| {
                d.child(div().px(px(18.0)).py(px(10.0)).text_color(theme.muted).child("No matches"))
            })
    }
}
