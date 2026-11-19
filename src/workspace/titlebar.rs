//! The title bar: the tab strip, tab colours and badges, dragging tabs, and the window controls.

use super::*;

/// Narrowest a tab gets before the tab strip scrolls instead.
const TAB_MIN_WIDTH: f32 = 56.0;

/// Below this share of the bar, inactive tabs drop their close button and
/// pane count to leave room for titles.
const COMPACT_TAB_WIDTH: f32 = 140.0;

/// A tab's close button and the gap before it.
const CLOSE_WIDTH: f32 = 26.0;

/// Width of the split-window icon beside a tab's pane count.
const PANES_ICON_WIDTH: f32 = 10.0;

/// Widest a tab gets.
const TAB_MAX_WIDTH: f32 = 220.0;

/// A tab being dragged along the tab bar, by its position.
#[derive(Clone)]
struct TabDrag {
    index: usize,
    title: SharedString,
    colors: (gpui::Hsla, gpui::Hsla, gpui::Hsla),
}

/// The tab drawn under the pointer while you drag one.
struct TabGhost(TabDrag);

impl Render for TabGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (bg, fg, border) = self.0.colors;
        div()
            .h(px(28.0))
            .max_w(px(220.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .rounded(px(8.0))
            .bg(bg)
            .border_1()
            .border_color(border)
            .text_color(fg)
            .text_size(px(12.5))
            .font_family(ui_font())
            .whitespace_nowrap()
            .overflow_hidden()
            .child(self.0.title.clone())
    }
}

/// Where a tab moved from `from` lands, given the slot it was dropped on.
pub fn reorder<T>(items: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from >= items.len() || to >= items.len() || from == to {
        return false;
    }
    let item = items.remove(from);
    items.insert(to, item);
    true
}

/// A tab's tint: one of the theme's colours, so it suits every theme, or an
/// exact one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabColor {
    Ansi(u8),
    Rgb(u8, u8, u8),
}

impl TabColor {
    pub const NAMES: [(&'static str, u8); 7] =
        [("red", 1), ("green", 2), ("yellow", 3), ("blue", 4), ("magenta", 5), ("purple", 5), ("cyan", 6)];

    /// A name from [`TabColor::NAMES`], or `#rrggbb`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim().to_ascii_lowercase();
        if let Some(&(_, i)) = Self::NAMES.iter().find(|(n, _)| *n == text) {
            return Ok(TabColor::Ansi(i));
        }
        let hex = text.strip_prefix('#').filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()));
        match hex {
            Some(h) => {
                let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
                Ok(TabColor::Rgb(byte(0), byte(2), byte(4)))
            }
            None => {
                Err(format!("`color` should be red, green, yellow, blue, magenta, cyan or #rrggbb, not \"{text}\""))
            }
        }
    }

    /// The name or `#rrggbb` this colour is told as over the socket API.
    pub fn name(self) -> String {
        match self {
            TabColor::Ansi(i) => Self::NAMES.iter().find(|(_, n)| *n == i).map_or("", |(name, _)| name).to_string(),
            TabColor::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }

    pub fn resolve(self, theme: &Theme) -> Hsla {
        match self {
            TabColor::Ansi(i) => theme.ansi[i as usize],
            TabColor::Rgb(r, g, b) => {
                Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }.into()
            }
        }
    }
}

/// Room a badge takes in a tab beside its text: the gap before it and its
/// padding.
const BADGE_CHROME: f32 = 8.0 + 12.0;

/// Room the title keeps beside a badge: its whole width when it's short,
/// else enough for a couple of letters and "…".
const MIN_TITLE_WIDTH: f32 = 32.0;

/// Which badge to show in `room` (the tab's width left for title and badge),
/// leaving the title `title_room`: the badge whole; else its short form; else
/// shortened with "…" when at least three characters fit; else none.
/// `measure` gives a text's width.
fn fit_badge(
    badge: &str,
    short: Option<&str>,
    room: f32,
    title_room: f32,
    measure: impl Fn(&str) -> f32,
) -> Option<String> {
    let fits = |text: &str| BADGE_CHROME + measure(text) + title_room <= room;
    if fits(badge) {
        return Some(badge.to_string());
    }
    if let Some(short) = short {
        return fits(short).then(|| short.to_string());
    }
    let chars: Vec<char> = badge.chars().collect();
    (2..chars.len()).rev().map(|n| chars[..n].iter().chain(['…'].iter()).collect::<String>()).find(|cut| fits(cut))
}

impl Workspace {
    pub(super) fn render_titlebar(
        &mut self,
        theme: &Theme,
        decorations: Decorations,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let client = matches!(decorations, Decorations::Client { .. });
        let controls = client && !cfg!(target_os = "macos");
        // Tabs share the bar equally, up to TAB_MAX_WIDTH each; a title longer
        // than its tab's share ends in an ellipsis. When space is tight,
        // inactive tabs drop their close button (shown only on hover anyway)
        // and pane count. Below TAB_MIN_WIDTH the strip scrolls instead.
        // (Measured here because GPUI places a text's ellipsis before flexbox
        // shrinks it.)
        let bar = f32::from(window.viewport_size().width)
            - 2.0 * SHADOW_INSET
            - if cfg!(target_os = "macos") { 84.0 } else { 10.0 }
            - 10.0
            - 28.0
            - if controls { 96.0 } else { 0.0 }
            - 3.0 * 4.0;
        let n = self.tabs.len().max(1) as f32;
        let gaps = 4.0 * (n - 1.0);
        let compact = (bar - gaps) / n < COMPACT_TAB_WIDTH;
        // In compact tabs only the active one keeps its close button.
        let active_extra = if compact { CLOSE_WIDTH } else { 0.0 };
        let share = ((bar - gaps - active_extra) / n).clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH);
        // Keep the active tab in view when the strip scrolls.
        if self.tab_scrolled_to != Some(self.active_tab) {
            self.tab_scrolled_to = Some(self.active_tab);
            self.tab_scroll.scroll_to_item(self.active_tab);
        }
        let accent = theme.accent;
        // Real widths, from the fonts as drawn: an estimate per character
        // clipped titles, and emoji in badges are wider than letters.
        let text_system = window.text_system().clone();
        let measure = move |text: &str, family: SharedString, size: f32| -> f32 {
            if text.is_empty() {
                return 0.0;
            }
            let run = gpui::TextRun {
                len: text.len(),
                font: gpui::font(family),
                color: gpui::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            f32::from(text_system.shape_line(text.to_string().into(), px(size), &[run], None).width)
        };
        let tabs = self.tabs.iter().enumerate().map(|(i, tab)| {
            let active = i == self.active_tab;
            let title: SharedString = tab.title.clone().unwrap_or_else(|| tab.active.read(cx).title.clone()).into();
            let panes = tab.leaves().len();
            let show_close = active || !compact;
            let width = share + if active { active_extra } else { 0.0 };
            // Room left for the title after the tab's padding, number, close
            // button, and dot or pane count if shown.
            let number = measure(&(i + 1).to_string(), MONO_FONT.into(), 11.0);
            let pane_count = panes.to_string();
            let fixed = 16.0 + number + 8.0 + if show_close { CLOSE_WIDTH } else { 0.0 };
            // A long command finished in a pane you haven't looked at since.
            // Red (a failed command) beats yellow (a bell) beats green.
            let leaves = tab.leaves();
            let finished = leaves.iter().filter_map(|l| l.read(cx).attention).reduce(|a, b| a && b);
            let rang = leaves.iter().any(|l| l.read(cx).rang);
            let attention = match (finished, rang) {
                (Some(false), _) => Some(theme.ansi[1]),
                (_, true) => Some(theme.ansi[3]),
                (Some(true), false) => Some(theme.ansi[2]),
                (None, false) => None,
            };
            let fixed = fixed + if attention.is_some() { 15.0 } else { 0.0 };
            let tint = shown_tab_color(tab.own_color, tab.color).map(|c| c.resolve(theme));
            // Monospace, so a ticking countdown doesn't jiggle the title. In a
            // narrow tab the title gives way first, down to a few letters;
            // then the badge does: its short form, shortened with an
            // ellipsis, or left out.
            let title_room = measure(&title, ui_font(), 12.5).min(MIN_TITLE_WIDTH);
            let badge = tab.badge.as_deref().and_then(|b| {
                fit_badge(b, tab.short_badge.as_deref(), width - fixed, title_room, |t| {
                    measure(t, MONO_FONT.into(), 10.5)
                })
            });
            let fixed = fixed + badge.as_ref().map_or(0.0, |b| BADGE_CHROME + measure(b, MONO_FONT.into(), 10.5));
            // The pane count goes before the badge does: a badge was put
            // there on purpose.
            let panes_width = 8.0 + PANES_ICON_WIDTH + 3.0 + measure(&pane_count, MONO_FONT.into(), 10.5);
            let show_panes = panes > 1 && !compact && width - fixed - panes_width >= title_room;
            let fixed = fixed + if show_panes { panes_width } else { 0.0 };
            let title_width = (width - fixed).max(16.0);
            let title = truncate_title(title, title_width, cx);
            div()
                .id(("tab", i))
                .group("tab")
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.0))
                .h(px(28.0))
                .pl(px(10.0))
                .pr(px(6.0))
                .rounded(px(8.0))
                .max_w(px(width))
                .cursor_pointer()
                .text_color(if active { theme.text } else { theme.muted })
                // The active tab's fill is lighter than any other tab's, and
                // inactive tints stay faint, so a tinted tab can't look like
                // the one you're on.
                .when(active, |d| {
                    let fill = mix(theme.raised, theme.text, 0.1);
                    d.bg(tint.map_or(fill, |t| mix(fill, t, 0.18))).shadow(vec![BoxShadow {
                        color: alpha(gpui::black(), 0.18),
                        offset: point(px(0.0), px(1.0)),
                        blur_radius: px(3.0),
                        spread_radius: px(0.0),
                    }])
                })
                .when(!active, |d| match tint {
                    Some(t) => d.bg(alpha(t, 0.07)).hover(move |d| d.bg(alpha(t, 0.13))),
                    None => d.hover(|d| d.bg(alpha(theme.raised, 0.55))),
                })
                .on_drag(
                    TabDrag {
                        index: i,
                        title: title.clone(),
                        colors: (theme.raised, theme.text, alpha(theme.accent, 0.5)),
                    },
                    |drag, _, _, cx| cx.new(|_| TabGhost(drag.clone())),
                )
                .drag_over::<TabDrag>(
                    move |style, drag, _, _| {
                        if drag.index == i { style } else { style.bg(alpha(accent, 0.22)) }
                    },
                )
                .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                    this.move_tab(drag.index, i, window, cx);
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.activate_tab(i, window, cx);
                        if ev.click_count == 2 {
                            this.rename_tab(i, window, cx);
                        }
                    }),
                )
                // Items are centred by their boxes, but the fonts differ: nudge
                // the number onto the title's baseline, and the dot and × onto
                // the middle of its capitals (measured; these fonts are bundled).
                .child(
                    div()
                        .flex_none()
                        .relative()
                        .top(px(1.0))
                        .font_family(MONO_FONT)
                        .text_size(px(11.0))
                        .text_color(tint.unwrap_or(if active { theme.accent } else { theme.muted }))
                        .child(format!("{}", i + 1)),
                )
                .when_some(attention, |d, color| {
                    d.child(div().relative().top(px(-1.0)).flex_none().size(px(7.0)).rounded(px(4.0)).bg(color))
                })
                .child(
                    div()
                        .max_w(px(title_width))
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(title),
                )
                .when_some(badge, |d, badge| {
                    let color = tint.unwrap_or(theme.subtext);
                    d.child(
                        div()
                            .flex_none()
                            .font_family(MONO_FONT)
                            .text_size(px(10.5))
                            .px(px(6.0))
                            .rounded(px(4.0))
                            .bg(alpha(tint.unwrap_or(theme.muted), 0.18))
                            .text_color(color)
                            .child(badge),
                    )
                })
                // The pane count: a little split-window icon and the number,
                // without a pill, so it doesn't read as another badge.
                .when(show_panes, |d| {
                    let color = if active { theme.subtext } else { theme.muted };
                    d.child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(3.0))
                            .font_family(MONO_FONT)
                            .text_size(px(10.5))
                            .text_color(color)
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .w(px(PANES_ICON_WIDTH))
                                    .h(px(9.0))
                                    .border_1()
                                    .border_color(color)
                                    .rounded(px(2.0))
                                    .child(div().w(px(4.0)).h_full().border_r_1().border_color(color)),
                            )
                            .child(pane_count),
                    )
                })
                .when(show_close, |d| {
                    d.child(
                        div()
                            .id(("tab-close", i))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(18.0))
                            .rounded(px(5.0))
                            .text_size(px(13.0))
                            .text_color(theme.muted)
                            .opacity(if active { 0.8 } else { 0.0 })
                            .group_hover("tab", |s| s.opacity(0.8))
                            .hover(|s| s.bg(alpha(theme.muted, 0.2)).text_color(theme.text))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    let ids: Vec<_> = this.tabs[i].leaves().iter().map(|l| l.entity_id()).collect();
                                    for id in ids {
                                        this.close_pane_entity(id, window, cx);
                                    }
                                }),
                            )
                            .child(div().relative().top(px(-1.0)).child("×")),
                    )
                })
        });

        let new_tab = div()
            .id("new-tab")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(28.0))
            .rounded(px(8.0))
            .text_size(px(16.0))
            .text_color(theme.muted)
            .cursor_pointer()
            .hover(|s| s.bg(alpha(theme.raised, 0.6)).text_color(theme.text))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.new_tab(&NewTab, window, cx);
                }),
            )
            .child(div().relative().top(px(-1.0)).child("+"));

        div()
            .id("titlebar")
            .flex()
            .flex_none()
            .items_center()
            .h(px(TITLEBAR_HEIGHT))
            .pl(px(if cfg!(target_os = "macos") { 84.0 } else { 10.0 }))
            .pr(px(10.0))
            .gap(px(4.0))
            .font_family(ui_font())
            .text_size(px(12.5))
            .when(!self.quick, |d| {
                d.on_mouse_down(MouseButton::Left, |ev, window, _| {
                    if ev.click_count >= 2 {
                        window.titlebar_double_click();
                    } else {
                        window.start_window_move();
                    }
                })
            })
            .child(
                div()
                    .id("tab-strip")
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .min_w_0()
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll)
                    .children(tabs),
            )
            .child(new_tab)
            .child(div().flex_1())
            .when(client && !self.quick && !cfg!(target_os = "macos"), |d| d.child(window_controls(theme)))
    }
}

/// `title` shortened with an ellipsis to fit `width` in the tab font. (GPUI's
/// own text ellipsis doesn't apply to text measured before its box shrank.)
fn truncate_title(title: SharedString, width: f32, cx: &App) -> SharedString {
    let font = gpui::font(ui_font());
    let mut runs = vec![gpui::TextRun {
        len: title.len(),
        font: font.clone(),
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    let mut wrapper = cx.text_system().line_wrapper(font, px(12.5));
    wrapper.truncate_line(title, px(width), "…", &mut runs)
}

fn window_controls(theme: &Theme) -> impl IntoElement {
    let (hover_bg, hover_fg) = (alpha(theme.raised, 0.9), theme.text);
    let button = |id: &'static str, bg: gpui::Hsla, fg: gpui::Hsla| {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(28.0))
            .rounded_full()
            .cursor_pointer()
            .text_color(theme.subtext)
            .hover(move |s| s.bg(bg).text_color(fg))
    };
    let stroke = theme.subtext;
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            button("win-min", hover_bg, hover_fg)
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    cx.stop_propagation();
                    window.minimize_window();
                })
                .child(div().w(px(10.0)).h(px(1.5)).bg(stroke)),
        )
        .child(
            button("win-max", hover_bg, hover_fg)
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    cx.stop_propagation();
                    window.zoom_window();
                })
                .child(div().size(px(9.0)).border_1().border_color(stroke).rounded(px(2.0))),
        )
        .child(
            button("win-close", gpui::rgb(0xe5484d).into(), gpui::white())
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    cx.stop_propagation();
                    window.remove_window();
                })
                .text_size(px(15.0))
                .child("×"),
        )
}

/// The colour a tab shows: the one you picked, else a script's or plugin's.
pub(super) fn shown_tab_color(own: Option<TabColor>, set: Option<TabColor>) -> Option<TabColor> {
    own.or(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_tabs_land_in_the_slot_they_were_dropped_on() {
        let mut tabs = vec!['a', 'b', 'c', 'd'];
        assert!(reorder(&mut tabs, 0, 2));
        assert_eq!(tabs, ['b', 'c', 'a', 'd']);
        assert!(reorder(&mut tabs, 3, 0));
        assert_eq!(tabs, ['d', 'b', 'c', 'a']);
        assert!(!reorder(&mut tabs, 1, 1));
        assert!(!reorder(&mut tabs, 1, 4));
        assert_eq!(tabs, ['d', 'b', 'c', 'a']);
    }

    #[test]
    fn badges_shorten_or_go_in_narrow_tabs() {
        // Monospace at 6 px a character, emoji twice that.
        let measure = |t: &str| t.chars().map(|c| if c == '🍅' { 12.0 } else { 6.0 }).sum::<f32>();
        let room = |chars: f32| BADGE_CHROME + 6.0 * chars + MIN_TITLE_WIDTH;
        let fit = |b, short, chars| fit_badge(b, short, room(chars), MIN_TITLE_WIDTH, measure);
        assert_eq!(fit("19:42", None, 20.0).as_deref(), Some("19:42"));
        assert_eq!(fit("web-1.staging", None, 6.0).as_deref(), Some("web-1…"));
        assert_eq!(fit("web-1.staging", None, 2.0), None);
        // The emoji counts double: "🍅 18:42" needs 8 characters' room.
        assert_eq!(fit("🍅 18:42", Some("18m"), 8.0).as_deref(), Some("🍅 18:42"));
        assert_eq!(fit("🍅 18:42", Some("18m"), 7.0).as_deref(), Some("18m"));
        // A short form that doesn't fit either means no badge, not a cut one.
        assert_eq!(fit("🍅 18:42", Some("18m"), 2.0), None);
        // The title keeps its room: with less, the badge gives way.
        assert_eq!(fit_badge("19:42", None, room(5.0), MIN_TITLE_WIDTH + 1.0, measure).as_deref(), Some("19:…"));
    }

    #[test]
    fn a_colour_you_pick_wins_over_one_a_script_sets() {
        let (mine, script) = (Some(TabColor::Ansi(1)), Some(TabColor::Ansi(4)));
        assert_eq!(shown_tab_color(mine, script), mine);
        assert_eq!(shown_tab_color(None, script), script);
        assert_eq!(shown_tab_color(None, None), None);
    }

    #[test]
    fn tab_colors_are_theme_names_or_hex() {
        assert_eq!(TabColor::parse("Red"), Ok(TabColor::Ansi(1)));
        assert_eq!(TabColor::parse("purple"), TabColor::parse("magenta"));
        assert_eq!(TabColor::parse("#FF8000"), Ok(TabColor::Rgb(255, 128, 0)));
        assert!(TabColor::parse("#ff80").is_err());
        assert!(TabColor::parse("orange").unwrap_err().contains("not \"orange\""));
    }
}
