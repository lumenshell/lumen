//! The window: custom title bar with tabs, a padded canvas of split panes, and the palette.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, AnyView, App, AppContext, Bounds, font, BoxShadow, Context, CursorStyle, Decorations, Hsla, Rgba,
    DragMoveEvent, ElementId, Entity, EntityId, Focusable, HitboxBehavior, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, ResizeEdge,
    SharedString, Size, StatefulInteractiveElement, StyleRefinement, Styled, Subscription, Task,
    Window,
    WindowBackgroundAppearance, WindowBounds, canvas, div,
    point, prelude::FluentBuilder, px, relative,
};

use serde_json::{Value, json};

use crate::api::{self, ApiError, ApiResult};
use crate::keymap;
use crate::palette::{CommandPalette, Dot, PaletteEvent, PaletteFn, PaletteItem};
use crate::session::{self, Axis, NodeState, Session, TabState, WindowState};
use crate::settings::{MONO_FONT, Settings, UI_FONT};
use crate::pty::{GridSize, Owner};
use std::path::PathBuf;
use crate::terminal_view::{TerminalEvent, TerminalView};
use crate::theme::{Theme, alpha};
use crate::*;

const GAP: f32 = 8.0;
/// Height of the title bar with the tabs.
const TITLEBAR_HEIGHT: f32 = 44.0;
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
const RADIUS: f32 = 10.0;
const WINDOW_RADIUS: f32 = 12.0;
pub const SHADOW_INSET: f32 = 10.0;
/// Smallest a pane may be dragged to, in pixels along the split axis.
const MIN_PANE: f32 = 80.0;

enum PaneNode {
    Leaf(Entity<TerminalView>),
    /// `ratios[i]` is child i's share of the space; they need not sum to 1.
    Split { axis: Axis, children: Vec<PaneNode>, ratios: Vec<f32> },
}

/// Drag payload for a split divider: the split (by child-index path from the tab
/// root) and the divider index (between children `index` and `index + 1`).
#[derive(Clone)]
struct SplitDrag {
    tab: usize,
    path: Vec<usize>,
    index: usize,
}

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
            .font_family(UI_FONT)
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

/// GPUI needs a view to show under the cursor while dragging; ours draws nothing.
struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl PaneNode {
    fn leaves(&self, out: &mut Vec<Entity<TerminalView>>) {
        match self {
            PaneNode::Leaf(e) => out.push(e.clone()),
            PaneNode::Split { children, .. } => children.iter().for_each(|c| c.leaves(out)),
        }
    }

    /// What the session file keeps. Plugin panes aren't kept (their plugin
    /// opens them again if it wants), so a split may lose some sides, or all.
    fn to_state(&self, cx: &App) -> Option<NodeState> {
        match self {
            PaneNode::Leaf(term) => {
                let view = term.read(cx);
                (!view.is_plugin_pane()).then(|| NodeState::Pane { cwd: view.cwd() })
            }
            PaneNode::Split { axis, children, ratios } => {
                let (children, ratios): (Vec<_>, Vec<_>) = children
                    .iter()
                    .zip(ratios)
                    .filter_map(|(c, r)| Some((c.to_state(cx)?, *r)))
                    .unzip();
                match children.len() {
                    0 => None,
                    1 => children.into_iter().next(),
                    _ => {
                        let total: f32 = ratios.iter().sum();
                        let ratios = ratios.iter().map(|r| r / total).collect();
                        Some(NodeState::Split { axis: *axis, children, ratios })
                    }
                }
            }
        }
    }

    fn at_path_mut(&mut self, path: &[usize]) -> Option<&mut PaneNode> {
        match path.split_first() {
            None => Some(self),
            Some((&i, rest)) => match self {
                PaneNode::Split { children, .. } => children.get_mut(i)?.at_path_mut(rest),
                PaneNode::Leaf(_) => None,
            },
        }
    }

    fn rects(&self, r: Rect, out: &mut Vec<(EntityId, Rect)>) {
        match self {
            PaneNode::Leaf(e) => out.push((e.entity_id(), r)),
            PaneNode::Split { axis, children, ratios } => {
                let total: f32 = ratios.iter().sum();
                let mut start = 0.0;
                for (child, ratio) in children.iter().zip(ratios) {
                    let share = ratio / total;
                    let cr = match axis {
                        Axis::Row => Rect { x: r.x + r.w * start, y: r.y, w: r.w * share, h: r.h },
                        Axis::Column => {
                            Rect { x: r.x, y: r.y + r.h * start, w: r.w, h: r.h * share }
                        }
                    };
                    child.rects(cr, out);
                    start += share;
                }
            }
        }
    }

    fn split(&mut self, target: EntityId, new: &Entity<TerminalView>, axis: Axis) -> bool {
        match self {
            PaneNode::Leaf(e) => {
                if e.entity_id() != target {
                    return false;
                }
                let old = PaneNode::Leaf(e.clone());
                *self = PaneNode::Split {
                    axis,
                    children: vec![old, PaneNode::Leaf(new.clone())],
                    ratios: vec![1.0, 1.0],
                };
                true
            }
            PaneNode::Split { axis: a, children, ratios } => {
                if *a == axis
                    && let Some(i) = children
                        .iter()
                        .position(|c| matches!(c, PaneNode::Leaf(e) if e.entity_id() == target))
                {
                    // The new pane takes half of the pane it was split from.
                    let half = ratios[i] / 2.0;
                    ratios[i] = half;
                    ratios.insert(i + 1, half);
                    children.insert(i + 1, PaneNode::Leaf(new.clone()));
                    return true;
                }
                children.iter_mut().any(|c| c.split(target, new, axis))
            }
        }
    }

    /// Give `new`, just split off its neighbour before it, `share` of the
    /// space the two had.
    fn set_share(&mut self, new: EntityId, share: f32) -> bool {
        let PaneNode::Split { children, ratios, .. } = self else { return false };
        if let Some(i) = children.iter().position(|c| matches!(c, PaneNode::Leaf(e) if e.entity_id() == new))
            && i > 0
        {
            let both = ratios[i - 1] + ratios[i];
            ratios[i] = both * share;
            ratios[i - 1] = both - ratios[i];
            return true;
        }
        children.iter_mut().any(|c| c.set_share(new, share))
    }

    /// Remove a leaf. Returns true if found. Collapses single-child splits.
    fn remove(&mut self, target: EntityId) -> bool {
        let PaneNode::Split { children, ratios, .. } = self else { return false };
        let found = if let Some(i) = children
            .iter()
            .position(|c| matches!(c, PaneNode::Leaf(e) if e.entity_id() == target))
        {
            children.remove(i);
            // Hand the freed space to the neighbour that grows into it.
            let freed = ratios.remove(i);
            let neighbour = i.saturating_sub(1).min(ratios.len() - 1);
            ratios[neighbour] += freed;
            true
        } else {
            children.iter_mut().any(|c| c.remove(target))
        };
        if found && children.len() == 1 {
            *self = children.pop().unwrap();
        }
        found
    }
}

struct Tab {
    root: PaneNode,
    active: Entity<TerminalView>,
    zoomed: bool,
    /// Title set over the socket API, shown instead of the program's.
    title: Option<String>,
    /// A short label beside the title (a countdown, a count), set over the
    /// socket API. Not saved.
    badge: Option<String>,
    /// What to show instead of the badge when the tab is too narrow for it
    /// ("18m" for "🍅 18:42"). Set with the badge, and cleared with it.
    short_badge: Option<String>,
    /// A colour tinting the tab, set over the socket API. Not saved.
    color: Option<TabColor>,
    /// The plugin connection that set the badge and colour: they go when it
    /// does, so a stopped plugin leaves no stale countdown. (A script's stay:
    /// `lumen msg` disconnects straight away.)
    style_owner: Option<u64>,
}

/// What a new pane runs: your shell (in a folder), or nothing, for a plugin
/// to draw.
pub enum NewPane {
    Shell(Option<std::path::PathBuf>),
    Plugin(Owner),
}

impl NewPane {
    fn into_parts(self) -> (Option<std::path::PathBuf>, Option<Owner>) {
        match self {
            NewPane::Shell(cwd) => (cwd, None),
            NewPane::Plugin(owner) => (None, Some(owner)),
        }
    }
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
            None => Err(format!(
                "`color` should be red, green, yellow, blue, magenta, cyan or #rrggbb, not \"{text}\""
            )),
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
            TabColor::Rgb(r, g, b) => Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }.into(),
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
    (2..chars.len())
        .rev()
        .map(|n| chars[..n].iter().chain(['…'].iter()).collect::<String>())
        .find(|cut| fits(cut))
}

/// `a` with `t` of `b` mixed in.
fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (a.into(), b.into());
    let m = |x: f32, y: f32| x + (y - x) * t;
    Rgba { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: m(a.a, b.a) }.into()
}

impl Tab {
    fn leaves(&self) -> Vec<Entity<TerminalView>> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }
}

pub struct Workspace {
    /// What the window shows behind its contents, last set from the
    /// background-opacity setting.
    background_appearance: Option<WindowBackgroundAppearance>,
    /// The tab strip, which scrolls sideways when tabs don't fit.
    tab_scroll: gpui::ScrollHandle,
    /// Active tab when the strip last scrolled to it.
    tab_scrolled_to: Option<usize>,
    tabs: Vec<Tab>,
    active_tab: usize,
    palette: Option<Entity<CommandPalette>>,
    /// Plugins waiting for you to allow their permissions, asked one at a time.
    pending_approvals: Vec<crate::plugins::Manifest>,
    /// Problems reading plugin manifests, shown in the banner.
    plugin_problems: Vec<String>,
    /// Axis of the divider being dragged, for the window-wide resize cursor.
    dragging: Option<Axis>,
    /// This window's id, which the session file keys its state by.
    window_id: u64,
    saved_on_quit: bool,
    window_state: Option<WindowState>,
    /// This is the quick terminal: drawn flush to the screen's edges, with
    /// no window buttons, and not saved in the session.
    quick: bool,
    _autosave: Task<()>,
    _subs: Vec<Subscription>,
}

impl Workspace {
    pub fn new(saved: Option<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with(saved, false, window, cx)
    }

    /// The quick terminal's workspace: one fresh tab.
    pub fn new_quick(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = Self::with(None, true, window, cx);
        cx.observe_window_activation(window, |_, window, cx| crate::quick::activation_changed(window.is_window_active(), cx))
            .detach();
        this
    }

    fn with(saved: Option<Session>, quick: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            background_appearance: None,
            tab_scroll: gpui::ScrollHandle::new(),
            tab_scrolled_to: None,
            tabs: Vec::new(),
            active_tab: 0,
            palette: None,
            pending_approvals: Vec::new(),
            plugin_problems: Vec::new(),
            dragging: None,
            window_id: window.window_handle().window_id().as_u64(),
            saved_on_quit: false,
            window_state: saved.as_ref().and_then(|s| s.window),
            quick,
            _autosave: Task::ready(()),
            _subs: Vec::new(),
        };
        // The quick terminal isn't saved (unregistered windows aren't written).
        if !quick {
            session::register(this.window_id);
        }
        match saved {
            Some(saved) => this.restore(saved, window, cx),
            None => this.new_tab(&NewTab, window, cx),
        }
        this.track_window(window);
        this._subs.push(cx.observe_window_bounds(window, |this, window, _| this.track_window(window)));
        // Built-in plugins turned on or off (in the gallery or config.toml).
        this._subs.push(cx.observe_global_in::<Settings>(window, |this, window, cx| {
            if Settings::get(cx).plugins != crate::plugins::builtins_running() {
                this.reload_plugins(window, cx);
            }
        }));

        // Save on quit, when the window is closed, and every few seconds while
        // running (so a crash or logout loses little and cwds stay current).
        this._subs.push(cx.on_app_quit(|this, cx| {
            this.save_session(cx);
            this.saved_on_quit = true;
            async {}
        }));
        this._subs.push(cx.on_release(|this, cx| {
            if !this.saved_on_quit {
                this.save_session(cx);
                session::forget(this.window_id);
            }
        }));
        // Record this window now, so the file never lists the others without it.
        this.save_session(cx);
        this._autosave = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this.update(cx, |this, cx| this.save_session(cx)).is_err() {
                    break;
                }
            }
        });
        this
    }

    // ---- session --------------------------------------------------------

    fn restore(&mut self, saved: Session, window: &mut Window, cx: &mut Context<Self>) {
        for tab in saved.tabs {
            let root = self.build_node(tab.root, window, cx);
            let mut leaves = Vec::new();
            root.leaves(&mut leaves);
            let active = leaves[tab.active.min(leaves.len() - 1)].clone();
            let zoomed = tab.zoomed && leaves.len() > 1;
            self.tabs.push(Tab {
                root,
                active,
                zoomed,
                title: tab.title.filter(|t| !t.trim().is_empty()),
                badge: None,
                short_badge: None,
                color: None,
                style_owner: None,
            });
        }
        self.active_tab = saved.active_tab.min(self.tabs.len() - 1);
        self.focus_active(window, cx);
    }

    /// Record the window's placement. While maximized or fullscreen, keep the
    /// last normal bounds rather than the screen-sized ones.
    fn track_window(&mut self, window: &Window) {
        // Inner bounds exclude the transparent shadow margin around client-side
        // decorations, matching what `open_window` expects back.
        let (bounds, maximized, fullscreen) = match window.inner_window_bounds() {
            WindowBounds::Windowed(b) => (Some(b), false, false),
            WindowBounds::Maximized(_) => (None, true, false),
            WindowBounds::Fullscreen(_) => (None, false, true),
        };
        let prev = self.window_state;
        let (x, y, width, height) = match (bounds, prev) {
            (Some(b), _) => (
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            ),
            (None, Some(p)) => (p.x, p.y, p.width, p.height),
            (None, None) => {
                let b = window.inner_window_bounds().get_bounds();
                (
                    f32::from(b.origin.x),
                    f32::from(b.origin.y),
                    f32::from(b.size.width),
                    f32::from(b.size.height),
                )
            }
        };
        self.window_state = Some(WindowState { x, y, width, height, maximized, fullscreen });
    }

    fn build_node(&mut self, node: NodeState, window: &mut Window, cx: &mut Context<Self>) -> PaneNode {
        match node {
            NodeState::Pane { cwd } => PaneNode::Leaf(self.spawn_terminal(NewPane::Shell(cwd), window, cx)),
            NodeState::Split { axis, children, ratios } => PaneNode::Split {
                axis,
                children: children.into_iter().map(|c| self.build_node(c, window, cx)).collect(),
                ratios,
            },
        }
    }

    fn snapshot(&self, cx: &App) -> Session {
        let mut tabs = Vec::new();
        let mut active_tab = 0;
        for (i, tab) in self.tabs.iter().enumerate() {
            let Some(root) = tab.root.to_state(cx) else { continue };
            if i <= self.active_tab {
                active_tab = tabs.len();
            }
            let kept: Vec<_> = tab.leaves().into_iter().filter(|l| !l.read(cx).is_plugin_pane()).collect();
            tabs.push(TabState {
                root,
                active: kept.iter().position(|l| *l == tab.active).unwrap_or(0),
                zoomed: tab.zoomed && kept.len() > 1,
                title: tab.title.clone(),
            });
        }
        Session::new(self.window_state, active_tab, tabs)
    }

    fn save_session(&mut self, cx: &App) {
        session::record(self.window_id, self.snapshot(cx));
    }

    fn spawn_terminal(
        &mut self,
        what: NewPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        let (cwd, owner) = what.into_parts();
        let term = TerminalView::open(cwd, owner, window, cx);
        api::emit("pane-opened", json!({ "pane": term.entity_id().as_u64() }));
        let sub = cx.subscribe_in(&term, window, |this, term, event, window, cx| match event {
            TerminalEvent::Attention => cx.notify(),
            TerminalEvent::TitleChanged => {
                api::emit(
                    "title-changed",
                    json!({ "pane": term.entity_id().as_u64(), "title": term.read(cx).title }),
                );
                cx.notify()
            }
            TerminalEvent::Focused => {
                api::emit("focus-changed", json!({ "pane": term.entity_id().as_u64() }));
                // Only track the active pane within its own tab; switching tabs is
                // driven explicitly so a late focus event can't snap back.
                if let Some(tab) =
                    this.tabs.iter_mut().find(|t| t.leaves().iter().any(|l| l == term))
                {
                    tab.active = term.clone();
                }
                cx.notify();
            }
            TerminalEvent::Exited => this.close_pane_entity(term.entity_id(), window, cx),
        });
        self._subs.push(sub);
        term
    }

    fn active_terminal(&self) -> Option<&Entity<TerminalView>> {
        self.tabs.get(self.active_tab).map(|t| &t.active)
    }

    /// Give the active pane the keyboard.
    pub fn focus_active_pane(&self, window: &mut Window, cx: &mut App) {
        self.focus_active(window, cx);
    }

    fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(t) = self.active_terminal() {
            window.focus(&t.focus_handle(cx));
        }
    }

    fn current_cwd(&self, cx: &App) -> Option<std::path::PathBuf> {
        self.active_terminal().and_then(|t| t.read(cx).cwd())
    }

    // ---- actions --------------------------------------------------------

    /// Open another window, starting in this pane's folder, the same size as
    /// this one.
    fn new_window(&mut self, _: &NewWindow, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.current_cwd(cx);
        let size = window.inner_window_bounds().get_bounds().size;
        cx.defer(move |cx| crate::new_window(cwd, Some(size), cx));
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.current_cwd(cx);
        self.open_tab(NewPane::Shell(cwd), window, cx);
    }

    /// Open a tab after the current one and switch to it.
    fn open_tab(
        &mut self,
        what: NewPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        let term = self.spawn_terminal(what, window, cx);
        let insert_at = if self.tabs.is_empty() { 0 } else { self.active_tab + 1 };
        self.tabs.insert(
            insert_at,
            Tab {
                root: PaneNode::Leaf(term.clone()),
                active: term.clone(),
                zoomed: false,
                title: None,
                badge: None,
                short_badge: None,
                color: None,
                style_owner: None,
            },
        );
        self.active_tab = insert_at;
        self.focus_active(window, cx);
        cx.notify();
        term
    }

    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.active_terminal().map(|t| t.entity_id()) else { return };
        let cwd = self.current_cwd(cx);
        self.split_pane(target, axis, NewPane::Shell(cwd), true, window, cx);
    }

    /// Split the pane `target` (in any tab), putting a new pane after it. With
    /// `focus`, the new pane becomes active and its tab is shown.
    fn split_pane(
        &mut self,
        target: EntityId,
        axis: Axis,
        what: NewPane,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalView>> {
        let tab_idx = self.tabs.iter().position(|t| t.leaves().iter().any(|l| l.entity_id() == target))?;
        let new = self.spawn_terminal(what, window, cx);
        let tab = &mut self.tabs[tab_idx];
        tab.root.split(target, &new, axis);
        tab.zoomed = false;
        if focus {
            tab.active = new.clone();
            self.active_tab = tab_idx;
            self.focus_active(window, cx);
        }
        cx.notify();
        Some(new)
    }

    fn split_right(&mut self, _: &SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Row, window, cx);
    }

    fn split_down(&mut self, _: &SplitDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Column, window, cx);
    }

    fn close_pane(&mut self, _: &ClosePane, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_terminal().map(|t| t.entity_id()) {
            self.close_pane_entity(id, window, cx);
        }
    }

    fn close_pane_entity(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab_idx) =
            self.tabs.iter().position(|t| t.leaves().iter().any(|l| l.entity_id() == id))
        else {
            return;
        };
        api::emit("pane-closed", json!({ "pane": id.as_u64() }));
        let tab = &mut self.tabs[tab_idx];
        if matches!(tab.root, PaneNode::Leaf(_)) {
            self.tabs.remove(tab_idx);
            if self.tabs.is_empty() {
                // The window's last shell exited: close the window (Lumen
                // quits after its last window).
                self.save_session(cx);
                window.remove_window();
                return;
            }
            if self.active_tab >= tab_idx && self.active_tab > 0 {
                self.active_tab -= 1;
            }
        } else {
            let leaves = tab.leaves();
            let pos = leaves.iter().position(|l| l.entity_id() == id).unwrap_or(0);
            tab.root.remove(id);
            tab.zoomed = false;
            if tab.active.entity_id() == id {
                let leaves = tab.leaves();
                tab.active = leaves[pos.min(leaves.len() - 1)].clone();
            }
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active_tab = index;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Move tab `from` to position `to`, keeping it active.
    fn move_tab(&mut self, from: usize, to: usize, window: &mut Window, cx: &mut Context<Self>) {
        if reorder(&mut self.tabs, from, to) {
            self.active_tab = to;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len();
        self.activate_tab((self.active_tab + 1) % n, window, cx);
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len();
        self.activate_tab((self.active_tab + n - 1) % n, window, cx);
    }

    fn rename_tab_action(&mut self, _: &RenameTab, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_tab(self.active_tab, window, cx);
    }

    /// Ask for a new name for tab `index`; an empty name goes back to the
    /// program's title.
    fn rename_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index) else { return };
        let current = tab.title.clone().unwrap_or_default();
        let automatic = tab.active.read(cx).title.clone();
        let target = tab.active.downgrade();
        let ws = cx.entity().downgrade();
        let submit: crate::palette::SubmitFn = Rc::new(move |name, _window, cx| {
            let _ = ws.update(cx, |ws, cx| {
                // Find the tab again by its pane: tabs may have moved meanwhile.
                let Some(tab) = ws.tabs.iter_mut().find(|t| t.leaves().iter().any(|l| l.downgrade() == target)) else {
                    return;
                };
                tab.title = if name.is_empty() { None } else { Some(name) };
                cx.notify();
            });
        });
        let palette = cx.new(|cx| {
            CommandPalette::prompt(format!("Name tab {}", index + 1).into(), current, submit, cx).with_message(format!(
                "Enter to rename. Leave it empty to go back to the automatic title (\"{automatic}\")."
            ))
        });
        self.open_palette(palette, window, cx);
    }

    fn activate_tab_action(&mut self, a: &ActivateTab, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_tab(a.0, window, cx);
    }

    fn focus_pane(&mut self, dx: f32, dy: f32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        let mut rects = Vec::new();
        tab.root.rects(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }, &mut rects);
        let active_id = tab.active.entity_id();
        let Some(&(_, cur)) = rects.iter().find(|(id, _)| *id == active_id) else { return };
        let (cx0, cy0) = (cur.x + cur.w / 2.0, cur.y + cur.h / 2.0);
        let best = rects
            .iter()
            .filter(|(id, _)| *id != active_id)
            .filter_map(|(id, r)| {
                let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                let along = (x - cx0) * dx + (y - cy0) * dy;
                if along <= 0.001 {
                    return None;
                }
                let across = ((x - cx0) * dy).abs() + ((y - cy0) * dx).abs();
                Some((along + across * 2.0, *id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, id)) = best
            && let Some(leaf) = tab.leaves().into_iter().find(|l| l.entity_id() == id)
        {
            tab.active = leaf;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn next_pane(&mut self, _: &NextPane, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        let leaves = tab.leaves();
        let i = leaves.iter().position(|l| *l == tab.active).unwrap_or(0);
        tab.active = leaves[(i + 1) % leaves.len()].clone();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn toggle_zoom(&mut self, _: &ToggleZoom, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.zoomed = !tab.zoomed && tab.leaves().len() > 1;
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn toggle_palette(&mut self, _: &TogglePalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.dismiss_palette(window, cx);
            return;
        }
        let items = palette_items(window, cx);
        let palette = cx.new(|cx| CommandPalette::new(items, cx));
        self.open_palette(palette, window, cx);
    }

    /// Show `palette`, replacing (and cancelling) any open one.
    fn open_palette(&mut self, palette: Entity<CommandPalette>, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_palette(window, cx);
        let sub = cx.subscribe_in(&palette, window, |this, palette, event, window, cx| match event {
            // Only if it's still the open one: a choice may have opened another.
            PaletteEvent::Dismiss if this.palette.as_ref() == Some(palette) => this.dismiss_palette(window, cx),
            PaletteEvent::Dismiss => {}
        });
        self._subs.push(sub);
        window.focus(&palette.focus_handle(cx));
        self.palette = Some(palette);
        cx.notify();
    }

    /// Close the palette; one closed without a choice runs its cancel hook.
    fn dismiss_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.take() else { return };
        let cancel = palette.read(cx).on_cancel();
        drop(palette);
        self.focus_active(window, cx);
        cx.notify();
        // Run after this update: the hook may update the workspace itself.
        if let Some(cancel) = cancel {
            window.defer(cx, move |window, cx| cancel(window, cx));
        }
    }

    // ---- plugins --------------------------------------------------------

    /// Load the plugins folder: start what's allowed, and ask about the rest.
    pub fn start_plugins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::plugins::{self, Decision};
        let enabled = Settings::get(cx).plugins.clone();
        plugins::set_builtins_running(enabled.clone());
        let (manifests, problems) = plugins::discover(&enabled);
        self.plugin_problems = problems;
        self.pending_approvals.clear();
        for m in manifests {
            match plugins::decision(&m) {
                Decision::Allowed => plugins::start(m),
                Decision::Refused => plugins::add_stopped(m, "not allowed"),
                Decision::Ask => {
                    plugins::add_stopped(m.clone(), "waiting for your approval");
                    self.pending_approvals.push(m);
                }
            }
        }
        self.ask_next_approval(window, cx);
        cx.notify();
    }

    fn reload_plugins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::plugins::stop_all();
        crate::plugins::clear();
        keymap::apply(cx);
        self.start_plugins(window, cx);
    }

    /// The Plugin Gallery: Lumen's built-in plugins, to turn on and off.
    fn open_gallery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = Settings::get(cx).plugins.clone();
        let ws = cx.entity().downgrade();
        let items = crate::plugins::BUILTINS
            .iter()
            .map(|b| {
                let on = enabled.iter().any(|n| n == b.name);
                let note = if b.overridden() { "your own copy runs instead".to_string() } else { b.summary() };
                let ws = ws.clone();
                let name = b.name;
                PaletteItem::new(
                    b.title,
                    Rc::new(move |window, cx| {
                        let _ = ws.update(cx, |ws, cx| ws.open_gallery_item(name, window, cx));
                    }),
                )
                .dot(if on { Dot::On } else { Dot::Off })
                .note(note)
            })
            .collect();
        let palette = cx.new(|cx| {
            CommandPalette::picker(items, "Plugin Gallery: choose one to turn it on or off".into(), None, cx)
                .with_message("Built-in plugins. Each runs as its own program, once you turn it on.")
        });
        self.open_palette(palette, window, cx);
    }

    /// One built-in plugin: what it does and needs, and turning it on or off.
    fn open_gallery_item(&mut self, name: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = crate::plugins::builtin(name) else { return };
        let m = match b.manifest() {
            Ok(m) => m,
            Err(e) => {
                self.plugin_problems.push(format!("built-in plugin `{name}`: {e}"));
                cx.notify();
                return;
            }
        };
        let on = Settings::get(cx).plugins.iter().any(|n| n == name);
        let needs = if m.permissions.is_empty() {
            "It needs no permissions.".to_string()
        } else {
            let wants: Vec<String> = m.permissions.iter().map(|p| p.describe().to_string()).collect();
            format!("Turning it on lets it {}.", join_and(&wants))
        };
        let note = if b.overridden() {
            format!(" You have your own \"{name}\" in the plugins folder, which runs instead.")
        } else {
            String::new()
        };
        let message = format!("{}. {needs}{note}", m.description.trim_end_matches('.'));
        let ws = cx.entity().downgrade();
        let toggle: PaletteFn = {
            let ws = ws.clone();
            Rc::new(move |_window, cx| {
                if !on {
                    // Choosing it here, with its permissions shown, approves it.
                    crate::plugins::record_decision(&m, true);
                }
                Settings::update(cx, |s| {
                    s.plugins.retain(|n| n != name);
                    if !on {
                        s.plugins.push(name.to_string());
                    }
                });
                let _ = ws.update(cx, |_, cx| cx.notify());
            })
        };
        let back: PaletteFn = Rc::new(move |window, cx| {
            let _ = ws.update(cx, |ws, cx| ws.open_gallery(window, cx));
        });
        let mut items = vec![PaletteItem::new(if on { "Turn off" } else { "Turn on" }, toggle).note(if on {
            "stop it and forget the setting"
        } else {
            "start it now and whenever Lumen opens"
        })];
        if on {
            let ws = cx.entity().downgrade();
            items.push(PaletteItem::new(
                "Open its log",
                Rc::new(move |window, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.open_plugin_log(name, window, cx));
                }),
            ));
        }
        items.push(PaletteItem::new("Back to the gallery", back));
        let palette = cx.new(|cx| CommandPalette::picker(items, b.title.into(), None, cx).with_message(message));
        self.open_palette(palette, window, cx);
    }

    /// Ask about the next plugin waiting for approval, in the palette.
    fn ask_next_approval(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_approvals.is_empty() {
            return;
        }
        let m = self.pending_approvals.remove(0);
        let wants: Vec<String> = m.permissions.iter().map(|p| p.describe().to_string()).collect();
        let what = if m.description.is_empty() { String::new() } else { format!(" ({})", m.description) };
        let question: SharedString = format!("Allow the plugin \"{}\"?", m.name).into();
        let home = std::env::var("HOME").unwrap_or_default();
        let message = format!(
            "\"{}\"{what} wants to {}. It runs `{}` in {}.",
            m.name,
            join_and(&wants),
            m.command.join(" "),
            m.dir.display().to_string().replacen(&home, "~", 1),
        );
        let ws = cx.entity().downgrade();
        let choice = |allow: bool, m: crate::plugins::Manifest, ws: gpui::WeakEntity<Workspace>| -> PaletteFn {
            Rc::new(move |window, cx| {
                crate::plugins::record_decision(&m, allow);
                if allow {
                    crate::plugins::start(m.clone());
                } else {
                    crate::plugins::add_stopped(m.clone(), "not allowed");
                }
                let _ = ws.update(cx, |ws, cx| ws.ask_next_approval(window, cx));
            })
        };
        let items = vec![
            PaletteItem::new(format!("Allow and start \"{}\"", m.name), choice(true, m.clone(), ws.clone()))
                .note(m.command.join(" ")),
            PaletteItem::new("Don't allow", choice(false, m.clone(), ws.clone()))
                .note("it won't run; you can allow it later from the palette"),
        ];
        // Esc: decide later (it's asked again next launch).
        let later: PaletteFn = Rc::new(move |window, cx| {
            let _ = ws.update(cx, |ws, cx| ws.ask_next_approval(window, cx));
        });
        let palette = cx.new(|cx| CommandPalette::picker(items, question, Some(later), cx).with_message(message));
        self.open_palette(palette, window, cx);
    }

    /// `show-prompt`: let a plugin or script ask for a line of text. The
    /// answer is `{"text": ...}` with what you typed (trimmed), or
    /// `{"text": null}` if you close it.
    pub fn show_prompt(
        &mut self,
        params: &Value,
        reply: futures::channel::oneshot::Sender<ApiResult>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (placeholder, initial, message) = match parse_prompt(params) {
            Ok(p) => p,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        // Sent once: on Enter, or null on any way of closing (the drop covers
        // a palette that goes away without running its cancel hook).
        let answer = Rc::new(Answer::new("text", reply));
        let submit_answer = answer.clone();
        let submit: crate::palette::SubmitFn = Rc::new(move |text, _, _| submit_answer.send(json!(text)));
        let cancel: PaletteFn = Rc::new(move |_, _| answer.send(Value::Null));
        let palette = cx.new(|cx| {
            let prompt = CommandPalette::prompt(placeholder.into(), initial, submit, cx).with_cancel(cancel);
            match message {
                Some(m) => prompt.with_message(m),
                None => prompt,
            }
        });
        self.open_palette(palette, window, cx);
    }

    /// `show-picker`: let a plugin or script offer a choice in the palette.
    /// The answer is `{"id": ...}`, or `{"id": null}` if you close it.
    pub fn show_picker(
        &mut self,
        params: &Value,
        reply: futures::channel::oneshot::Sender<ApiResult>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(list) = params.get("items").and_then(Value::as_array).filter(|l| !l.is_empty()) else {
            let _ = reply.send(Err(ApiError::invalid_params("`items` should be a non-empty list of {id, label}")));
            return;
        };
        let mut parsed = Vec::new();
        for item in list.iter().take(5000) {
            let id = item.get("id").cloned().filter(|v| v.is_string() || v.is_number());
            let label = item.get("label").and_then(Value::as_str);
            match (id, label) {
                (Some(id), Some(label)) => parsed.push((
                    id,
                    label.to_string(),
                    item.get("detail").and_then(Value::as_str).unwrap_or_default().to_string(),
                )),
                _ => {
                    let _ = reply.send(Err(ApiError::invalid_params("each item needs an `id` and a `label`")));
                    return;
                }
            }
        }
        let placeholder: SharedString =
            params.get("placeholder").and_then(Value::as_str).unwrap_or("Choose…").to_string().into();
        let answer = Rc::new(Answer::new("id", reply));
        let items = parsed
            .into_iter()
            .map(|(id, label, detail)| {
                let answer = answer.clone();
                PaletteItem::new(label, Rc::new(move |_, _| answer.send(id.clone()))).note(detail)
            })
            .collect();
        let cancel_answer = answer.clone();
        drop(answer);
        let cancel: PaletteFn = Rc::new(move |_, _| cancel_answer.send(Value::Null));
        let message = params.get("message").and_then(Value::as_str).map(str::trim).filter(|m| !m.is_empty());
        let palette = cx.new(|cx| {
            let picker = CommandPalette::picker(items, placeholder, Some(cancel), cx);
            match message {
                Some(m) => picker.with_message(m.to_string()),
                None => picker,
            }
        });
        self.open_palette(palette, window, cx);
    }

    fn run_plugin_command(&mut self, a: &RunPluginCommand, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.active_terminal().map(|t| t.entity_id().as_u64());
        if let Err(e) = crate::plugins::run_command(&a.plugin, &a.id, pane) {
            eprintln!("lumen: {e}");
        }
        cx.notify();
    }

    /// Show a plugin's log in a new tab, following it.
    /// Pick a plugin, by how it's doing, to open its log.
    fn open_plugin_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws = cx.entity().downgrade();
        let items = crate::plugins::list()
            .into_iter()
            .map(|info| {
                let ws = ws.clone();
                let name = info.manifest.name.clone();
                // How it's doing as a dot; the words only when something's up.
                let dot = plugin_dot(&info.status);
                let item = PaletteItem::new(
                    name.clone(),
                    Rc::new(move |window, cx| {
                        let _ = ws.update(cx, |ws, cx| ws.open_plugin_log(&name, window, cx));
                    }),
                )
                .dot(dot);
                if dot == Dot::Good { item } else { item.note(info.status.clone()) }
            })
            .collect();
        let palette = cx.new(|cx| CommandPalette::picker(items, "Plugin Logs: choose a plugin".into(), None, cx));
        self.open_palette(palette, window, cx);
    }

    fn open_plugin_log(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = crate::plugins::log_path(name) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&path);
        let quoted = format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
        let term = self.open_tab(NewPane::Shell(path.parent().map(std::path::Path::to_path_buf)), window, cx);
        term.read(cx).send_text(&format!("tail -n 200 -F {quoted}\r"));
        self.tabs[self.active_tab].title = Some(format!("{name} log"));
    }

    // ---- split resizing -------------------------------------------------

    /// Move divider `drag.index` so it sits under the mouse. `bounds` are the
    /// split container's bounds.
    fn drag_divider(
        &mut self,
        drag: &SplitDrag,
        mouse: Point<Pixels>,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if drag.tab != self.active_tab {
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        let Some(PaneNode::Split { axis, ratios, .. }) = tab.root.at_path_mut(&drag.path) else {
            return;
        };
        let i = drag.index;
        if i + 1 >= ratios.len() {
            return;
        }
        let (pos, size) = match axis {
            Axis::Row => (mouse.x - bounds.origin.x, bounds.size.width),
            Axis::Column => (mouse.y - bounds.origin.y, bounds.size.height),
        };
        // Children are flex-basis 0 with flex-grow = ratio, so the space left over
        // after the gaps is shared in proportion to the ratios.
        let free = f32::from(size) - GAP * (ratios.len() - 1) as f32;
        if free <= 0.0 {
            return;
        }
        let total: f32 = ratios.iter().sum();
        let before: f32 = ratios[..i].iter().sum();
        let pair = ratios[i] + ratios[i + 1];
        let target = (f32::from(pos) - i as f32 * GAP - GAP / 2.0) / free * total - before;
        let min = (MIN_PANE / free * total).min(pair / 2.0);
        let left = target.clamp(min, pair - min);
        ratios[i] = left;
        ratios[i + 1] = pair - left;
        self.dragging = Some(*axis);
        cx.notify();
    }

    fn equalize_split(&mut self, path: &[usize], cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        if let Some(PaneNode::Split { ratios, .. }) = tab.root.at_path_mut(path) {
            ratios.iter_mut().for_each(|r| *r = 1.0);
            cx.notify();
        }
    }

    // ---- rendering ------------------------------------------------------

    fn render_node(
        &self,
        node: &PaneNode,
        path: Vec<usize>,
        tab: &Tab,
        multi: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            PaneNode::Leaf(term) => {
                let focused = *term == tab.active;
                pane_frame(term, focused, multi, theme, Settings::get(cx).padding, cx)
            }
            PaneNode::Split { axis, children, ratios } => {
                let row = *axis == Axis::Row;
                let mut items = Vec::with_capacity(children.len() * 2);
                for (i, (child, ratio)) in children.iter().zip(ratios).enumerate() {
                    if i > 0 {
                        items.push(self.render_divider(row, path.clone(), i - 1, theme, cx));
                    }
                    let mut child_path = path.clone();
                    child_path.push(i);
                    let mut cell = div()
                        .flex_basis(relative(0.0))
                        .min_w_0()
                        .min_h_0()
                        .when(row, |d| d.h_full())
                        .when(!row, |d| d.w_full())
                        .child(self.render_node(child, child_path, tab, multi, theme, cx));
                    cell.style().flex_grow = Some(*ratio);
                    cell.style().flex_shrink = Some(1.0);
                    items.push(cell.into_any_element());
                }
                div()
                    .flex()
                    .size_full()
                    .when(row, |d| d.flex_row())
                    .when(!row, |d| d.flex_col())
                    .on_drag_move(cx.listener(
                        move |this, e: &DragMoveEvent<SplitDrag>, _window, cx| {
                            let drag = e.drag(cx).clone();
                            if drag.path == path {
                                this.drag_divider(&drag, e.event.position, e.bounds, cx);
                            }
                        },
                    ))
                    .children(items)
                    .into_any_element()
            }
        }
    }

    /// The gap between two panes, which doubles as a drag handle. A small accent
    /// pill appears on hover; double-click evens out the split.
    fn render_divider(
        &self,
        row: bool,
        path: Vec<usize>,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab = self.active_tab;
        let name: SharedString = format!("divider-{tab}-{path:?}-{index}").into();
        let pill = alpha(theme.accent, 0.75);
        let equalize_path = path.clone();
        div()
            .id(ElementId::Name(name.clone()))
            .group(name.clone())
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .when(row, |d| d.w(px(GAP)).h_full().cursor(CursorStyle::ResizeLeftRight))
            .when(!row, |d| d.h(px(GAP)).w_full().cursor(CursorStyle::ResizeUpDown))
            .child(
                div()
                    .rounded_full()
                    .when(row, |d| d.w(px(3.0)).h(px(36.0)))
                    .when(!row, |d| d.h(px(3.0)).w(px(36.0)))
                    .group_hover(name, move |s| s.bg(pill)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _window, cx| {
                    if e.click_count >= 2 {
                        this.equalize_split(&equalize_path, cx);
                    }
                }),
            )
            .on_drag(SplitDrag { tab, path, index }, |_, _, _, cx| cx.new(|_| DragGhost))
            .into_any_element()
    }

    fn render_titlebar(
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
        let measure = move |text: &str, family: &'static str, size: f32| -> f32 {
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
            let title: SharedString =
                tab.title.clone().unwrap_or_else(|| tab.active.read(cx).title.clone()).into();
            let panes = tab.leaves().len();
            let show_close = active || !compact;
            let width = share + if active { active_extra } else { 0.0 };
            // Room left for the title after the tab's padding, number, close
            // button, and dot or pane count if shown.
            let number = measure(&(i + 1).to_string(), MONO_FONT, 11.0);
            let pane_count = panes.to_string();
            let fixed = 16.0 + number + 8.0 + if show_close { CLOSE_WIDTH } else { 0.0 };
            // A long command finished in a pane you haven't looked at since.
            let attention = tab
                .leaves()
                .iter()
                .filter_map(|l| l.read(cx).attention)
                .reduce(|a, b| a && b)
                .map(|ok| if ok { theme.ansi[2] } else { theme.ansi[1] });
            let fixed = fixed + if attention.is_some() { 15.0 } else { 0.0 };
            let tint = tab.color.map(|c| c.resolve(theme));
            // Monospace, so a ticking countdown doesn't jiggle the title. In a
            // narrow tab the title gives way first, down to a few letters;
            // then the badge does: its short form, shortened with an
            // ellipsis, or left out.
            let title_room = measure(&title, UI_FONT, 12.5).min(MIN_TITLE_WIDTH);
            let badge = tab.badge.as_deref().and_then(|b| {
                fit_badge(b, tab.short_badge.as_deref(), width - fixed, title_room, |t| measure(t, MONO_FONT, 10.5))
            });
            let fixed = fixed + badge.as_ref().map_or(0.0, |b| BADGE_CHROME + measure(b, MONO_FONT, 10.5));
            // The pane count goes before the badge does: a badge was put
            // there on purpose.
            let panes_width = 8.0 + PANES_ICON_WIDTH + 3.0 + measure(&pane_count, MONO_FONT, 10.5);
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
                    TabDrag { index: i, title: title.clone(), colors: (theme.raised, theme.text, alpha(theme.accent, 0.5)) },
                    |drag, _, _, cx| cx.new(|_| TabGhost(drag.clone())),
                )
                .drag_over::<TabDrag>(move |style, drag, _, _| {
                    if drag.index == i { style } else { style.bg(alpha(accent, 0.22)) }
                })
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
                .when(show_close, |d| d.child(
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
                                let ids: Vec<_> =
                                    this.tabs[i].leaves().iter().map(|l| l.entity_id()).collect();
                                for id in ids {
                                    this.close_pane_entity(id, window, cx);
                                }
                            }),
                        )
                        .child(div().relative().top(px(-1.0)).child("×")),
                ))
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
            .font_family(UI_FONT)
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

/// Problems in the config file, shown at the bottom of the window until fixed.
fn problem_banner(problems: &[String], plugin_problems: &[String], theme: &Theme) -> impl IntoElement {
    let path = settings::config_path()
        .map(|p| p.display().to_string().replace(&std::env::var("HOME").unwrap_or_default(), "~"))
        .unwrap_or_else(|| "config.toml".into());
    div()
        .absolute()
        .bottom(px(GAP * 2.0))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .max_w(px(720.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .px(px(14.0))
                .py(px(9.0))
                .rounded(px(10.0))
                .bg(theme.raised)
                .border_1()
                .border_color(alpha(theme.ansi[1], 0.5))
                .font_family(UI_FONT)
                .text_size(px(12.5))
                .text_color(theme.text)
                .shadow(vec![BoxShadow {
                    color: alpha(gpui::black(), 0.35),
                    offset: point(px(0.0), px(6.0)),
                    blur_radius: px(18.0),
                    spread_radius: px(0.0),
                }])
                .when(!problems.is_empty(), |d| {
                    d.child(
                        div()
                            .text_color(theme.ansi[1])
                            .child(format!("Problem in {path} (other settings still apply)")),
                    )
                    .children(problems.iter().take(4).map(|p| div().text_color(theme.subtext).child(p.clone())))
                })
                .when(!plugin_problems.is_empty(), |d| {
                    d.child(div().text_color(theme.ansi[1]).child("Plugins that couldn't be loaded (fix, then Reload Plugins)"))
                        .children(plugin_problems.iter().take(4).map(|p| div().text_color(theme.subtext).child(p.clone())))
                }),
        )
}

/// `title` shortened with an ellipsis to fit `width` in the tab font. (GPUI's
/// own text ellipsis doesn't apply to text measured before its box shrank.)
fn truncate_title(title: SharedString, width: f32, cx: &App) -> SharedString {
    let font = gpui::font(UI_FONT);
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

/// Alphas for the window frame and the panes on it, so that with
/// `background-opacity` below 1 the frame is that see-through and panes,
/// whose fill is drawn over the frame's, are only a little more solid.
/// At 1 both are opaque.
pub fn backdrop_alphas(opacity: f32) -> (f32, f32) {
    (opacity, opacity.powi(8))
}

/// The window background for a `background-opacity`: blurred behind a
/// translucent window where the platform can. Fully opaque, Linux stays
/// transparent anyway, for its rounded corners and shadow.
pub fn window_background(opacity: f32) -> WindowBackgroundAppearance {
    if opacity < 1.0 {
        WindowBackgroundAppearance::Blurred
    } else if cfg!(target_os = "macos") {
        WindowBackgroundAppearance::Opaque
    } else {
        WindowBackgroundAppearance::Transparent
    }
}

/// Answers a request that waits on the palette once: with the choice, or
/// with null when dropped unanswered.
struct Answer {
    key: &'static str,
    tx: std::cell::RefCell<Option<futures::channel::oneshot::Sender<ApiResult>>>,
}

impl Answer {
    fn new(key: &'static str, tx: futures::channel::oneshot::Sender<ApiResult>) -> Self {
        Self { key, tx: std::cell::RefCell::new(Some(tx)) }
    }

    fn send(&self, value: Value) {
        if let Some(tx) = self.tx.borrow_mut().take() {
            let _ = tx.send(Ok(json!({ self.key: value })));
        }
    }
}

impl Drop for Answer {
    fn drop(&mut self) {
        self.send(Value::Null);
    }
}

/// The `placeholder`, `initial` text and `message` of a `show-prompt`: each
/// a string, or left out (null too).
fn parse_prompt(params: &Value) -> Result<(String, String, Option<String>), ApiError> {
    let get = |key: &str| match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(ApiError::invalid_params(format!("`{key}` should be a string"))),
    };
    let placeholder = get("placeholder")?.unwrap_or("Type here…").to_string();
    let initial = get("initial")?.unwrap_or_default().to_string();
    let message = get("message")?.map(str::trim).filter(|m| !m.is_empty()).map(str::to_string);
    Ok((placeholder, initial, message))
}

/// A pane's label and colour after a `set-pane-style` request: fields left
/// out stay as they are; null (or a blank label) clears them.
fn parse_pane_style(
    params: &Value,
    label: Option<String>,
    color: Option<TabColor>,
) -> Result<(Option<String>, Option<TabColor>), ApiError> {
    let label = match params.get("label") {
        None => label,
        Some(Value::Null) => None,
        Some(Value::String(l)) => {
            let l = l.trim();
            if l.chars().count() > 32 {
                return Err(ApiError::invalid_params("`label` should be at most 32 characters"));
            }
            (!l.is_empty()).then(|| l.to_string())
        }
        Some(_) => return Err(ApiError::invalid_params("`label` should be a string or null")),
    };
    let color = match params.get("color") {
        None => color,
        Some(Value::Null) => None,
        Some(Value::String(c)) => Some(TabColor::parse(c).map_err(ApiError::invalid_params)?),
        Some(_) => return Err(ApiError::invalid_params("`color` should be a string or null")),
    };
    Ok((label, color))
}

/// Who owns a pane's style after a `set-pane-style`: the caller's plugin
/// connection (`None` for a socket client), or nobody once both fields are
/// cleared. The last writer wins.
fn pane_style_owner(label: &Option<String>, color: &Option<TabColor>, caller: Option<u64>) -> Option<u64> {
    if label.is_none() && color.is_none() { None } else { caller }
}

/// Whether a closed connection's disconnect clears this pane's style: only
/// styles that connection set.
fn clears_on_disconnect(style_owner: Option<u64>, conn: Option<u64>) -> bool {
    conn.is_some() && style_owner == conn
}

/// Height of the strip that names a split pane.
const PANE_LABEL_HEIGHT: f32 = 22.0;

fn pane_frame(
    term: &Entity<TerminalView>,
    focused: bool,
    multi: bool,
    theme: &Theme,
    padding: f32,
    cx: &App,
) -> AnyElement {
    let mut shadows = vec![BoxShadow {
        color: alpha(gpui::black(), if theme.is_light { 0.08 } else { 0.30 }),
        offset: point(px(0.0), px(2.0)),
        blur_radius: px(10.0),
        spread_radius: px(0.0),
    }];
    if focused && multi {
        shadows.push(BoxShadow {
            color: alpha(theme.accent, 0.22),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(16.0),
            spread_radius: px(1.0),
        });
    }
    // A split pane with a label gets a strip above its terminal.
    let label = term.read(cx).label.clone().filter(|_| multi);
    let tint = term.read(cx).label_color.map(|c| c.resolve(theme));
    div()
        .relative()
        .size_full()
        .rounded(px(RADIUS))
        .bg(theme.pane)
        .border_1()
        .border_color(if focused && multi { alpha(theme.accent, 0.6) } else { theme.border })
        .shadow(shadows)
        .overflow_hidden()
        .flex()
        .flex_col()
        .when_some(label, |d, label| {
            d.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(PANE_LABEL_HEIGHT))
                    .px(px(padding.max(8.0)))
                    .bg(tint.map_or(alpha(theme.shell, 0.4), |t| mix(theme.pane, t, 0.16)))
                    .border_b_1()
                    .border_color(theme.border)
                    .font_family(UI_FONT)
                    .text_size(px(11.5))
                    .text_color(if focused { theme.text } else { theme.subtext })
                    .overflow_hidden()
                    .child(div().flex_none().size(px(7.0)).rounded_full().bg(tint.unwrap_or(theme.muted)))
                    .child(div().min_w_0().truncate().child(label)),
            )
        })
        .child(
            div().flex_1().min_h_0().w_full().p(px(padding)).child(
                // Reuse the pane's last frame unless it changed, so output in one
                // pane (or hovering a tab) doesn't redraw every other pane.
                AnyView::from(term.clone()).cached(StyleRefinement::default().size_full()),
            ),
        )
        .when(multi && !focused, |d| {
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(alpha(theme.shell, if theme.is_light { 0.25 } else { 0.35 })),
            )
        })
        .into_any_element()
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

/// A plugin's status (from `plugins::list`) as a palette dot.
fn plugin_dot(status: &str) -> Dot {
    match status {
        "running" => Dot::Good,
        "starting" | "waiting for your approval" => Dot::Busy,
        s if s.contains("restarting") => Dot::Busy,
        _ => Dot::Bad,
    }
}

fn shortcut_label(action: &dyn gpui::Action, window: &Window) -> SharedString {
    window
        .highest_precedence_binding_for_action(action)
        .map(|b| {
            b.keystrokes()
                .iter()
                .map(|k| pretty_keystroke(&k.to_string()))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
        .into()
}

fn pretty_keystroke(ks: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let ks = ks.replace("--", "-minus");
    ks.split('-')
        .map(|part| match (part, mac) {
            ("cmd", true) => "⌘".to_string(),
            ("shift", true) => "⇧".to_string(),
            ("alt", true) => "⌥".to_string(),
            ("ctrl", true) => "⌃".to_string(),
            ("ctrl", false) => "Ctrl".to_string(),
            ("shift", false) => "Shift".to_string(),
            ("alt", false) => "Alt".to_string(),
            ("enter", _) => "Enter".to_string(),
            ("minus", _) => "-".to_string(),
            ("tab", _) => "Tab".to_string(),
            ("pageup", _) => "PgUp".to_string(),
            ("pagedown", _) => "PgDn".to_string(),
            (k, _) => k.to_uppercase(),
        })
        .collect::<Vec<_>>()
        .join(if mac { "" } else { "+" })
}

impl Workspace {
    // ---- socket API -----------------------------------------------------

    fn find_pane(&self, id: u64) -> Option<(usize, Entity<TerminalView>)> {
        self.tabs.iter().enumerate().find_map(|(i, t)| {
            t.leaves().into_iter().find(|l| l.entity_id().as_u64() == id).map(|l| (i, l))
        })
    }

    /// The pane named by `params.pane`, or the focused one.
    fn target_pane(&self, params: &Value) -> Result<(usize, Entity<TerminalView>), ApiError> {
        match params.get("pane") {
            None | Some(Value::Null) => self
                .active_terminal()
                .map(|t| (self.active_tab, t.clone()))
                .ok_or_else(|| ApiError::failed("there are no panes")),
            Some(v) => {
                let id = v.as_u64().ok_or_else(|| ApiError::invalid_params("`pane` should be a pane id"))?;
                self.find_pane(id)
                    .ok_or_else(|| ApiError::failed(format!("no pane {id}; `lumen msg list` shows them")))
            }
        }
    }

    /// Whether pane `id` is in this window.
    pub fn has_pane(&self, id: u64) -> bool {
        self.find_pane(id).is_some()
    }

    pub fn api_list(&self, cx: &App) -> Value {
        let focused = self.active_terminal().map(|t| t.entity_id());
        let tabs: Vec<Value> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let panes: Vec<Value> = tab
                    .leaves()
                    .iter()
                    .map(|p| {
                        let view = p.read(cx);
                        let (cols, rows) = view.grid_size();
                        json!({
                            "id": p.entity_id().as_u64(),
                            "title": view.title,
                            "cwd": view.cwd(),
                            "active": *p == tab.active,
                            "focused": Some(p.entity_id()) == focused,
                            "cols": cols,
                            "rows": rows,
                            "last_exit": view.last_exit(),
                            "plugin_pane": view.is_plugin_pane(),
                            "label": view.label,
                            "color": view.label_color.map(TabColor::name),
                        })
                    })
                    .collect();
                json!({
                    "index": i,
                    "title": tab.title.clone().unwrap_or_else(|| tab.active.read(cx).title.clone()),
                    "custom_title": tab.title.is_some(),
                    "active": i == self.active_tab,
                    "zoomed": tab.zoomed,
                    "badge": tab.badge,
                    "short_badge": tab.short_badge,
                    "color": tab.color.map(TabColor::name),
                    "panes": panes,
                })
            })
            .collect();
        json!({ "tabs": tabs, "focused_pane": focused.map(|f| f.as_u64()) })
    }

    /// Run one socket API request. Methods and their parameters are described
    /// in docs/scripting.md.
    pub fn handle_api(
        &mut self,
        caller: &api::Caller,
        method: &str,
        params: &Value,
        origin: Option<&api::Origin>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ApiResult {
        let string = |name: &str| -> Result<Option<String>, ApiError> {
            match params.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) => Ok(Some(s.clone())),
                Some(_) => Err(ApiError::invalid_params(format!("`{name}` should be a string"))),
            }
        };
        let required = |name: &str| string(name)?.ok_or_else(|| ApiError::invalid_params(format!("`{name}` is required")));
        let cwd = || -> Result<Option<std::path::PathBuf>, ApiError> {
            match string("cwd")? {
                None => Ok(None),
                Some(dir) => {
                    let path = std::path::PathBuf::from(&dir);
                    if path.is_dir() { Ok(Some(path)) } else { Err(ApiError::failed(format!("{dir} isn't a directory"))) }
                }
            }
        };
        // `plugin_pane`: a pane the caller draws, instead of a shell.
        let owner = || -> Result<Option<Owner>, ApiError> {
            match params.get("plugin_pane") {
                None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(None),
                Some(Value::Bool(true)) => {
                    if params.get("run").is_some_and(|r| !r.is_null()) || params.get("cwd").is_some_and(|c| !c.is_null()) {
                        return Err(ApiError::invalid_params("a plugin pane runs no program, so takes no `run` or `cwd`"));
                    }
                    let origin = origin.ok_or_else(|| ApiError::failed("plugin panes need a connection to draw them"))?;
                    let name = match caller {
                        api::Caller::Plugin { name, .. } => name.clone(),
                        api::Caller::Socket => "script".into(),
                    };
                    Ok(Some(Owner { conn: origin.conn, out: origin.out.clone(), name }))
                }
                Some(_) => Err(ApiError::invalid_params("`plugin_pane` should be true or false")),
            }
        };
        // Typed into a new pane's shell, which reads it once it's ready.
        let run = |term: &Entity<TerminalView>, cx: &App| -> Result<(), ApiError> {
            if let Some(cmd) = string("run")? {
                term.read(cx).send_text(&format!("{cmd}\r"));
            }
            Ok(())
        };

        match method {
            "hello" => Ok(json!({
                "protocol": api::PROTOCOL,
                "version": env!("CARGO_PKG_VERSION"),
                "pid": std::process::id(),
                "events": api::EVENTS,
                "actions": keymap::ACTIONS,
            })),
            "list" => Ok(self.api_list(cx)),
            "new-tab" => {
                let what = match owner()? {
                    Some(owner) => NewPane::Plugin(owner),
                    None => NewPane::Shell(cwd()?.or_else(|| self.current_cwd(cx))),
                };
                string("run")?;
                let term = self.open_tab(what, window, cx);
                run(&term, cx)?;
                Ok(json!({ "pane": term.entity_id().as_u64(), "tab": self.active_tab }))
            }
            "split" => {
                let axis = match string("direction")?.as_deref() {
                    None | Some("right") => Axis::Row,
                    Some("down") => Axis::Column,
                    Some(other) => {
                        return Err(ApiError::invalid_params(format!(
                            "`direction` should be \"right\" or \"down\", not \"{other}\""
                        )));
                    }
                };
                let focus = match params.get("focus") {
                    None | Some(Value::Null) => true,
                    Some(v) => v.as_bool().ok_or_else(|| ApiError::invalid_params("`focus` should be true or false"))?,
                };
                // The new pane's share of the space: a fraction, or a number
                // of rows (down) or columns (right) of the pane split.
                let share = match params.get("size") {
                    None | Some(Value::Null) => None,
                    Some(v) => {
                        let n = v.as_f64().filter(|n| *n > 0.0).ok_or_else(|| {
                            ApiError::invalid_params("`size` should be a fraction (like 0.3) or a number of rows or columns")
                        })?;
                        Some(n)
                    }
                };
                let owner = owner()?;
                string("run")?;
                let (_, target) = self.target_pane(params)?;
                let what = match owner {
                    Some(owner) => NewPane::Plugin(owner),
                    None => NewPane::Shell(cwd()?.or_else(|| target.read(cx).cwd())),
                };
                let (cols, rows) = target.read(cx).grid_size();
                let new = self
                    .split_pane(target.entity_id(), axis, what, focus, window, cx)
                    .ok_or_else(|| ApiError::failed("that pane just closed"))?;
                if let Some(n) = share {
                    let whole = if axis == Axis::Row { cols } else { rows } as f64;
                    // The new pane's frame and the divider take about two
                    // rows (or columns) of what it's given.
                    let fraction = if n < 1.0 { n } else { (n + 2.0) / whole.max(1.0) };
                    let id = new.entity_id();
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.leaves().iter().any(|l| l.entity_id() == id)) {
                        tab.root.set_share(id, fraction.clamp(0.1, 0.9) as f32);
                    }
                }
                run(&new, cx)?;
                Ok(json!({ "pane": new.entity_id().as_u64() }))
            }
            "focus" => {
                let (tab, pane) = self.target_pane(params)?;
                self.tabs[tab].active = pane;
                self.active_tab = tab;
                self.focus_active(window, cx);
                window.activate_window();
                cx.notify();
                Ok(json!({}))
            }
            "close" => {
                let (_, pane) = self.target_pane(params)?;
                self.close_pane_entity(pane.entity_id(), window, cx);
                Ok(json!({}))
            }
            "send-text" => {
                let text = required("text")?;
                let (_, pane) = self.target_pane(params)?;
                pane.read(cx).send_text(&text);
                Ok(json!({}))
            }
            "read-screen" => {
                let lines = match params.get("lines") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(
                        v.as_u64()
                            .filter(|n| *n > 0)
                            .ok_or_else(|| ApiError::invalid_params("`lines` should be a positive number"))?
                            as usize,
                    ),
                };
                let (_, pane) = self.target_pane(params)?;
                Ok(json!({ "text": pane.read(cx).screen_text(lines) }))
            }
            "move-tab" => {
                let to = params
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| ApiError::invalid_params("`index` (a tab position, from 0) is required"))?
                    as usize;
                let (from, _) = self.target_pane(params)?;
                if to >= self.tabs.len() {
                    return Err(ApiError::invalid_params(format!("there are only {} tabs", self.tabs.len())));
                }
                self.move_tab(from, to, window, cx);
                Ok(json!({}))
            }
            "set-title" => {
                let title = string("title")?.filter(|t| !t.trim().is_empty());
                let (tab, _) = self.target_pane(params)?;
                self.tabs[tab].title = title;
                cx.notify();
                Ok(json!({}))
            }
            "set-tab-style" => {
                let (tab, _) = self.target_pane(params)?;
                // Fields left out stay as they are; null clears them.
                let badge = match params.get("badge") {
                    None => self.tabs[tab].badge.clone(),
                    Some(Value::Null) => None,
                    Some(Value::String(b)) => {
                        let b = b.trim();
                        if b.chars().count() > 24 {
                            return Err(ApiError::invalid_params("`badge` should be at most 24 characters"));
                        }
                        (!b.is_empty()).then(|| b.to_string())
                    }
                    Some(_) => return Err(ApiError::invalid_params("`badge` should be a string or null")),
                };
                // A new badge drops the old short form unless it brings one.
                let short_badge = match params.get("short_badge") {
                    None if params.get("badge").is_some() => None,
                    None => self.tabs[tab].short_badge.clone(),
                    Some(Value::Null) => None,
                    Some(Value::String(b)) => {
                        let b = b.trim();
                        if b.chars().count() > 8 {
                            return Err(ApiError::invalid_params("`short_badge` should be at most 8 characters"));
                        }
                        (!b.is_empty()).then(|| b.to_string())
                    }
                    Some(_) => return Err(ApiError::invalid_params("`short_badge` should be a string or null")),
                };
                let short_badge = short_badge.filter(|_| badge.is_some());
                let color = match params.get("color") {
                    None => self.tabs[tab].color,
                    Some(Value::Null) => None,
                    Some(Value::String(c)) => Some(TabColor::parse(c).map_err(ApiError::invalid_params)?),
                    Some(_) => return Err(ApiError::invalid_params("`color` should be a string or null")),
                };
                let owner = match caller {
                    api::Caller::Plugin { .. } => origin.map(|o| o.conn),
                    api::Caller::Socket => None,
                };
                let t = &mut self.tabs[tab];
                t.style_owner = if badge.is_none() && color.is_none() { None } else { owner };
                t.badge = badge;
                t.short_badge = short_badge;
                t.color = color;
                cx.notify();
                Ok(json!({}))
            }
            "set-pane-style" => {
                let (_, pane) = self.target_pane(params)?;
                let (label, color) = {
                    let view = pane.read(cx);
                    parse_pane_style(params, view.label.clone(), view.label_color)?
                };
                let owner = match caller {
                    api::Caller::Plugin { .. } => origin.map(|o| o.conn),
                    api::Caller::Socket => None,
                };
                pane.update(cx, |view, _| {
                    view.style_owner = pane_style_owner(&label, &color, owner);
                    view.label = label;
                    view.label_color = color;
                });
                cx.notify();
                Ok(json!({}))
            }
            "write" => {
                let text = required("text")?;
                let (_, pane) = self.target_pane(params)?;
                match pane.read(cx).owner_conn() {
                    None => Err(ApiError::failed("that pane runs a program; `write` is for plugin panes (`send-text` types into it)")),
                    Some(conn) if Some(conn) != origin.map(|o| o.conn) => {
                        Err(ApiError::failed("that plugin pane belongs to another connection"))
                    }
                    Some(_) => {
                        pane.update(cx, |view, _| view.feed(&text));
                        Ok(json!({}))
                    }
                }
            }
            // A connection closed: so do the plugin panes it drew.
            "_disconnected" => {
                let conn = params.get("conn").and_then(Value::as_u64);
                let orphans: Vec<EntityId> = self
                    .tabs
                    .iter()
                    .flat_map(|t| t.leaves())
                    .filter(|l| conn.is_some() && l.read(cx).owner_conn() == conn)
                    .map(|l| l.entity_id())
                    .collect();
                for id in orphans {
                    self.close_pane_entity(id, window, cx);
                }
                for tab in &mut self.tabs {
                    if conn.is_some() && tab.style_owner == conn {
                        (tab.badge, tab.short_badge, tab.color, tab.style_owner) = (None, None, None, None);
                    }
                }
                let styled: Vec<_> = self
                    .tabs
                    .iter()
                    .flat_map(|t| t.leaves())
                    .filter(|l| clears_on_disconnect(l.read(cx).style_owner, conn))
                    .collect();
                for pane in styled {
                    pane.update(cx, |view, _| {
                        (view.label, view.label_color, view.style_owner) = (None, None, None);
                    });
                }
                cx.notify();
                Ok(json!({}))
            }
            "run-action" => {
                let name = required("action")?;
                let name = keymap::check_action(&name)
                    .map_err(ApiError::invalid_params)?
                    .ok_or_else(|| ApiError::invalid_params("\"none\" isn't an action"))?;
                let action = keymap::action(&name).expect("checked above");
                if params.get("pane").is_some_and(|p| !p.is_null()) {
                    let (tab, pane) = self.target_pane(params)?;
                    self.tabs[tab].active = pane;
                    self.active_tab = tab;
                    self.focus_active(window, cx);
                }
                window.dispatch_action(action, cx);
                Ok(json!({}))
            }
            "register-command" => {
                let result = crate::plugins::register_command(caller, params)?;
                keymap::apply(cx);
                Ok(result)
            }
            // A plugin stopped; its commands and keys are gone.
            "_plugin-commands-changed" => {
                keymap::apply(cx);
                Ok(json!({}))
            }
            other => Err(ApiError(-32601, format!("unknown method \"{other}\""))),
        }
    }
}

fn palette_items(window: &Window, cx: &App) -> Vec<PaletteItem> {
    fn action<A: gpui::Action + Clone + Default>(label: &str) -> (SharedString, Box<dyn gpui::Action>) {
        (label.to_string().into(), Box::new(A::default()))
    }
    let actions: Vec<(SharedString, Box<dyn gpui::Action>)> = vec![
        action::<NewTab>("New Tab"),
        action::<NewWindow>("New Window"),
        action::<ToggleQuickTerminal>("Toggle Quick Terminal"),
        action::<SplitRight>("Split Right"),
        action::<SplitDown>("Split Down"),
        action::<ClosePane>("Close Pane"),
        action::<ToggleZoom>("Toggle Focus Mode"),
        action::<NextPane>("Focus Next Pane"),
        action::<NextTab>("Next Tab"),
        action::<PrevTab>("Previous Tab"),
        action::<RenameTab>("Rename Tab"),
        action::<NextTheme>("Cycle Theme"),
        action::<IncreaseFont>("Increase Font Size"),
        action::<DecreaseFont>("Decrease Font Size"),
        action::<ResetFont>("Reset Font Size"),
        action::<Copy>("Copy"),
        action::<Paste>("Paste"),
        action::<Find>("Find in Scrollback"),
        action::<PrevPrompt>("Jump to Previous Command"),
        action::<NextPrompt>("Jump to Next Command"),
        action::<CopyLastOutput>("Copy Last Command Output"),
        action::<ToggleSelectMode>("Keyboard Select Mode"),
        action::<Quit>("Quit"),
    ];
    let mut items: Vec<PaletteItem> = actions
        .into_iter()
        .map(|(label, act)| {
            let detail = shortcut_label(act.as_ref(), window);
            let act: Rc<dyn gpui::Action> = act.into();
            PaletteItem::new(label, Rc::new(move |window, cx| window.dispatch_action(act.boxed_clone(), cx)))
                .keys(detail)
        })
        .collect();
    if let Some(path) = settings::config_path() {
        let shown = path.display().to_string().replace(&std::env::var("HOME").unwrap_or_default(), "~");
        items.push(
            PaletteItem::new("Open Config File", Rc::new(move |_, cx| cx.open_url(&format!("file://{}", path.display()))))
                .note(shown),
        );
    }
    // Plugin commands, then a way to allow each plugin waiting for it.
    for info in crate::plugins::list() {
        for c in &info.commands {
            let act = RunPluginCommand { plugin: info.manifest.name.clone(), id: c.id.clone() };
            let shortcut = shortcut_label(&act, window);
            let item = PaletteItem::new(
                c.title.clone(),
                Rc::new(move |window, cx| window.dispatch_action(Box::new(act.clone()), cx)),
            );
            // Its shortcut if it has one, else which plugin it's from.
            items.push(if shortcut.is_empty() { item.note(info.manifest.name.clone()) } else { item.keys(shortcut) });
        }
    }
    let ws = window.root::<Workspace>().flatten().map(|w| w.downgrade());
    for info in crate::plugins::list() {
        let name = info.manifest.name.clone();
        let ws = ws.clone();
        if info.status == "not allowed" || info.status == "waiting for your approval" {
            let m = info.manifest.clone();
            items.push(
                PaletteItem::new(
                    format!("Allow Plugin: {name}"),
                    Rc::new(move |window, cx| {
                        if let Some(ws) = &ws {
                            let _ = ws.update(cx, |ws, cx| {
                                ws.pending_approvals.push(m.clone());
                                ws.ask_next_approval(window, cx);
                            });
                        }
                    }),
                )
                .dot(Dot::Busy)
                .note(info.status.clone()),
            );
        }
    }
    // One entry for every plugin's log, flagged when one of them is in trouble.
    let plugins = crate::plugins::list();
    if let Some(ws) = ws.clone().filter(|_| !plugins.is_empty()) {
        let troubled = plugins.iter().filter(|p| plugin_dot(&p.status) == Dot::Bad).count();
        let item = PaletteItem::new(
            "Plugin Logs…",
            Rc::new(move |window, cx| {
                let _ = ws.update(cx, |ws, cx| ws.open_plugin_logs(window, cx));
            }),
        );
        items.push(match troubled {
            0 => item.note("see what a plugin printed"),
            1 => item.dot(Dot::Bad).note("1 plugin has a problem"),
            n => item.dot(Dot::Bad).note(format!("{n} plugins have a problem")),
        });
    }
    if let Some(ws) = ws.clone() {
        items.push(
            PaletteItem::new(
                "Plugin Gallery",
                Rc::new(move |window, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.open_gallery(window, cx));
                }),
            )
            .note("turn on the plugins that come with Lumen"),
        );
    }
    if let Some(ws) = ws.clone() {
        items.push(
            PaletteItem::new(
                "Reload Plugins",
                Rc::new(move |window, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.reload_plugins(window, cx));
                }),
            )
            .note("restart every plugin and re-read plugin.toml files"),
        );
    }
    let settings = Settings::get(cx);
    for (i, theme) in settings.themes.iter().enumerate() {
        let item = PaletteItem::new(
            format!("Theme: {}", theme.name),
            Rc::new(move |_, cx| Settings::update(cx, |s| s.theme_index = i)),
        );
        items.push(if i == settings.theme_index { item.note("current") } else { item });
    }
    items
}

fn resize_edge(pos: Point<Pixels>, inset: Pixels, size: Size<Pixels>) -> Option<ResizeEdge> {
    let corner = inset * 2.0;
    let (l, r) = (pos.x < corner, pos.x > size.width - corner);
    let (t, b) = (pos.y < corner, pos.y > size.height - corner);
    let edge = if pos.y < inset {
        if l { ResizeEdge::TopLeft } else if r { ResizeEdge::TopRight } else { ResizeEdge::Top }
    } else if pos.y > size.height - inset {
        if l { ResizeEdge::BottomLeft } else if r { ResizeEdge::BottomRight } else { ResizeEdge::Bottom }
    } else if pos.x < inset {
        if t { ResizeEdge::TopLeft } else if b { ResizeEdge::BottomLeft } else { ResizeEdge::Left }
    } else if pos.x > size.width - inset {
        if t { ResizeEdge::TopRight } else if b { ResizeEdge::BottomRight } else { ResizeEdge::Right }
    } else {
        return None;
    };
    Some(edge)
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = Settings::get(cx);
        let mut theme = settings.theme().clone();
        let opacity = settings.background_opacity;
        let (frame_alpha, pane_alpha) = backdrop_alphas(opacity);
        theme.shell = alpha(theme.shell, frame_alpha);
        theme.pane = alpha(theme.pane, pane_alpha);
        let appearance = window_background(opacity);
        if self.background_appearance != Some(appearance) {
            self.background_appearance = Some(appearance);
            window.set_background_appearance(appearance);
        }
        let decorations = match window.window_decorations() {
            // The quick terminal sits flush against the screen's edges, like a
            // tiled window: no shadow margin or rounded corners.
            Decorations::Client { .. } if self.quick => Decorations::Client { tiling: gpui::Tiling::tiled() },
            d => d,
        };
        let inset = if self.quick { px(0.0) } else { px(SHADOW_INSET) };

        if let Some(term) = self.active_terminal() {
            let title = term.read(cx).title.clone();
            window.set_window_title(&title);
        }

        if !cx.has_active_drag() {
            self.dragging = None;
        }
        let drag_cursor = self.dragging.map(|axis| match axis {
            Axis::Row => CursorStyle::ResizeLeftRight,
            Axis::Column => CursorStyle::ResizeUpDown,
        });

        let problems = Settings::get(cx).problems.clone();
        let plugin_problems = self.plugin_problems.clone();
        let content = self.tabs.get(self.active_tab).map(|tab| {
            let multi = tab.leaves().len() > 1;
            if tab.zoomed {
                pane_frame(&tab.active, true, false, &theme, Settings::get(cx).padding, cx)
            } else {
                self.render_node(&tab.root, Vec::new(), tab, multi, &theme, cx)
            }
        });

        let (tiling, client) = match decorations {
            Decorations::Client { tiling } => (Some(tiling), true),
            Decorations::Server => (None, false),
        };
        if client {
            window.set_client_inset(inset);
        }

        let app = div()
            .id("app")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.shell)
            .text_color(theme.text)
            .font_family(UI_FONT)
            .cursor(CursorStyle::Arrow)
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .when_some(tiling, |d, tiling| {
                d.border_1()
                    .border_color(alpha(theme.text, if theme.is_light { 0.12 } else { 0.07 }))
                    .when(!tiling.is_tiled(), |d| {
                        d.rounded(px(WINDOW_RADIUS)).shadow(vec![BoxShadow {
                            color: alpha(gpui::black(), 0.45),
                            offset: point(px(0.0), px(2.0)),
                            blur_radius: inset / 1.5,
                            spread_radius: px(0.0),
                        }])
                    })
            })
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::new_window))
            .on_action(cx.listener(Self::split_right))
            .on_action(cx.listener(Self::split_down))
            .on_action(cx.listener(Self::close_pane))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::rename_tab_action))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::activate_tab_action))
            .on_action(cx.listener(Self::next_pane))
            .on_action(cx.listener(Self::toggle_zoom))
            .on_action(cx.listener(Self::toggle_palette))
            .on_action(cx.listener(Self::run_plugin_command))
            .on_action(cx.listener(|this, _: &FocusLeft, w, cx| this.focus_pane(-1.0, 0.0, w, cx)))
            .on_action(cx.listener(|this, _: &FocusRight, w, cx| this.focus_pane(1.0, 0.0, w, cx)))
            .on_action(cx.listener(|this, _: &FocusUp, w, cx| this.focus_pane(0.0, -1.0, w, cx)))
            .on_action(cx.listener(|this, _: &FocusDown, w, cx| this.focus_pane(0.0, 1.0, w, cx)))
            .child(self.render_titlebar(&theme, decorations, window, cx))
            // Keep the resize cursor while a divider drag strays off the divider.
            .when_some(drag_cursor, |d, style| {
                d.child(
                    canvas(|_, _, _| {}, move |_, _, window, _| {
                        window.set_window_cursor_style(style)
                    })
                    .absolute()
                    .size_0(),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(px(GAP))
                    .pb(px(GAP))
                    .children(content),
            )
            .when_some(self.palette.clone(), |d, palette| {
                d.child(
                    div()
                        .id("palette-backdrop")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .justify_center()
                        .items_start()
                        .pt(px(crate::palette::PALETTE_TOP))
                        // Dark enough that the panes behind don't compete with
                        // the list; lighter on a light theme, which greys fast.
                        .bg(alpha(gpui::black(), if theme.is_light { 0.25 } else { 0.45 }))
                        .when_some(tiling, |d, t| d.when(!t.is_tiled(), |d| d.rounded(px(WINDOW_RADIUS))))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.dismiss_palette(window, cx)),
                        )
                        .child(palette),
                )
            })
            .when(!problems.is_empty() || !plugin_problems.is_empty(), |d| {
                d.child(problem_banner(&problems, &plugin_problems, &theme))
            });

        // Outer frame: transparent margin used for the drop shadow and resize handles.
        div()
            .id("window-backdrop")
            .size_full()
            .bg(gpui::transparent_black())
            .when_some(tiling, |d, tiling| {
                d.child(
                    canvas(
                        |_bounds, window, _cx| {
                            window.insert_hitbox(
                                gpui::Bounds::new(
                                    point(px(0.0), px(0.0)),
                                    window.window_bounds().get_bounds().size,
                                ),
                                HitboxBehavior::Normal,
                            )
                        },
                        move |_bounds, hitbox, window, _cx| {
                            let mouse = window.mouse_position();
                            let size = window.window_bounds().get_bounds().size;
                            if let Some(edge) = resize_edge(mouse, inset, size) {
                                window.set_cursor_style(
                                    match edge {
                                        ResizeEdge::Top | ResizeEdge::Bottom => {
                                            CursorStyle::ResizeUpDown
                                        }
                                        ResizeEdge::Left | ResizeEdge::Right => {
                                            CursorStyle::ResizeLeftRight
                                        }
                                        ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                            CursorStyle::ResizeUpLeftDownRight
                                        }
                                        ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                            CursorStyle::ResizeUpRightDownLeft
                                        }
                                    },
                                    &hitbox,
                                );
                            }
                        },
                    )
                    .size_full()
                    .absolute(),
                )
                .when(!tiling.top, |d| d.pt(inset))
                .when(!tiling.bottom, |d| d.pb(inset))
                .when(!tiling.left, |d| d.pl(inset))
                .when(!tiling.right, |d| d.pr(inset))
                .on_mouse_move(|_, window, _| window.refresh())
                .on_mouse_down(MouseButton::Left, move |e, window, _| {
                    let size = window.window_bounds().get_bounds().size;
                    if let Some(edge) = resize_edge(e.position, inset, size) {
                        window.start_window_resize(edge);
                    }
                })
            })
            .child(app)
    }
}

/// "a", "a and b", "a, b and c".
fn join_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The panes a window will open with, in the order they open: each one's
/// directory and roughly the grid it will get in a window whose content is
/// `window` (logical pixels). `saved` is the window's session, or None for a
/// fresh window with one pane.
pub fn startup_panes(saved: Option<&Session>, window: Size<Pixels>, cx: &App) -> Vec<(Option<PathBuf>, GridSize)> {
    let settings = Settings::get(cx);
    let text_system = cx.text_system();
    let font_size = settings.font_size();
    let cell = text_system
        .advance(text_system.resolve_font(&font(settings.font_family.clone())), font_size, 'm')
        .map(|s| f32::from(s.width))
        .unwrap_or(8.0)
        .max(1.0);
    let line = f32::from((font_size * settings.line_height).round()).max(1.0);
    // Inside a pane's frame: its 1px border and the padding.
    let inset = 2.0 + 2.0 * settings.padding;
    let grid = |w: f32, h: f32| GridSize {
        cols: ((w - inset) / cell).floor().clamp(2.0, u16::MAX as f32) as u16,
        rows: ((h - inset) / line).floor().clamp(1.0, u16::MAX as f32) as u16,
        cell_width: cell as u16,
        cell_height: line as u16,
    };
    // Below the title bar, inside the gaps around the panes.
    let content = (f32::from(window.width) - 2.0 * GAP, f32::from(window.height) - TITLEBAR_HEIGHT - GAP);
    match saved {
        Some(saved) => layout_panes(&saved.tabs, content, &grid),
        None => vec![(None, grid(content.0, content.1))],
    }
}

/// Each pane of `tabs` with the grid `grid` gives the room it gets when the
/// tab's content is `content` (width, height) pixels.
fn layout_panes(
    tabs: &[TabState],
    content: (f32, f32),
    grid: &dyn Fn(f32, f32) -> GridSize,
) -> Vec<(Option<PathBuf>, GridSize)> {
    fn walk(
        node: &NodeState,
        (w, h): (f32, f32),
        grid: &dyn Fn(f32, f32) -> GridSize,
        out: &mut Vec<(Option<PathBuf>, GridSize)>,
    ) {
        match node {
            NodeState::Pane { cwd } => out.push((cwd.clone(), grid(w, h))),
            NodeState::Split { axis, children, ratios } => {
                let along = if *axis == Axis::Row { w } else { h };
                let room = (along - GAP * children.len().saturating_sub(1) as f32).max(0.0);
                let total: f32 = ratios.iter().sum::<f32>().max(f32::EPSILON);
                for (child, r) in children.iter().zip(ratios) {
                    let share = room * r / total;
                    walk(child, if *axis == Axis::Row { (share, h) } else { (w, share) }, grid, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    for tab in tabs {
        let first = out.len();
        walk(&tab.root, content, grid, &mut out);
        // A zoomed tab shows its focused pane alone.
        if tab.zoomed
            && let Some(pane) = out.get_mut(first + tab.active)
        {
            pane.1 = grid(content.0, content.1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_panes_get_their_share_of_the_window() {
        // One "cell" per pixel, so sizes read as pixels.
        let grid = |w: f32, h: f32| GridSize { cols: w as u16, rows: h as u16, cell_width: 1, cell_height: 1 };
        let pane = |cwd: &str| NodeState::Pane { cwd: Some(cwd.into()) };
        let tab = |root, active, zoomed| TabState { root, active, zoomed, title: None };
        let tabs = vec![
            tab(
                NodeState::Split {
                    axis: Axis::Row,
                    children: vec![
                        pane("/a"),
                        NodeState::Split { axis: Axis::Column, children: vec![pane("/b"), pane("/c")], ratios: vec![1.0, 3.0] },
                    ],
                    ratios: vec![1.0, 1.0],
                },
                0,
                false,
            ),
            tab(NodeState::Split { axis: Axis::Row, children: vec![pane("/d"), pane("/e")], ratios: vec![1.0, 1.0] }, 1, true),
        ];
        let got: Vec<(String, u16, u16)> = layout_panes(&tabs, (408.0, 208.0), &grid)
            .into_iter()
            .map(|(cwd, g)| (cwd.unwrap().display().to_string(), g.cols, g.rows))
            .collect();
        let want = [("/a", 200, 208), ("/b", 200, 50), ("/c", 200, 150), ("/d", 200, 208), ("/e", 408, 208)];
        assert_eq!(got, want.map(|(c, w, h)| (c.to_string(), w, h)));
    }

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
    fn pane_styles_keep_clear_and_check() {
        let cur = (Some("db".to_string()), Some(TabColor::Ansi(2)));
        let p = |v: Value| parse_pane_style(&v, cur.0.clone(), cur.1);
        assert_eq!(p(json!({})).unwrap(), cur); // left out stays
        assert_eq!(p(json!({"label": null})).unwrap(), (None, cur.1)); // null clears
        assert_eq!(p(json!({"label": "  "})).unwrap(), (None, cur.1)); // blank clears
        assert_eq!(
            p(json!({"label": " web ", "color": "red"})).unwrap(),
            (Some("web".into()), Some(TabColor::Ansi(1)))
        );
        assert!(p(json!({"label": "x".repeat(33)})).is_err());
        assert!(p(json!({"color": "orange"})).is_err());
        assert!(p(json!({"label": 3})).is_err());
    }

    #[test]
    fn prompt_params_are_strings() {
        let ok = parse_prompt(&json!({})).unwrap();
        assert_eq!(ok, ("Type here…".to_string(), String::new(), None));
        let ok = parse_prompt(&json!({"placeholder": "Command", "initial": "npm ", "message": "  Runs it.  "})).unwrap();
        assert_eq!(ok, ("Command".to_string(), "npm ".to_string(), Some("Runs it.".to_string())));
        // A blank message is none; null counts as left out.
        assert_eq!(parse_prompt(&json!({"message": " ", "initial": null})).unwrap().2, None);
        assert!(parse_prompt(&json!({"placeholder": 3})).is_err());
        assert!(parse_prompt(&json!({"initial": ["a"]})).is_err());
        assert!(parse_prompt(&json!({"message": true})).is_err());
    }

    #[test]
    fn pane_style_ownership_and_disconnect() {
        let (label, color) = (Some("db".to_string()), Some(TabColor::Ansi(2)));
        // A plugin's connection owns what it styled; a socket client owns nothing.
        assert_eq!(pane_style_owner(&label, &None, Some(7)), Some(7));
        assert_eq!(pane_style_owner(&None, &color, Some(7)), Some(7));
        assert_eq!(pane_style_owner(&label, &color, None), None);
        // Clearing both fields resets the owner.
        assert_eq!(pane_style_owner(&None, &None, Some(7)), None);
        // Last writer wins.
        assert_eq!(pane_style_owner(&label, &color, Some(8)), Some(8));
        // Disconnect clears only that connection's panes.
        assert!(clears_on_disconnect(Some(7), Some(7)));
        assert!(!clears_on_disconnect(Some(8), Some(7)));
        assert!(!clears_on_disconnect(None, Some(7))); // socket client's styles stay
        assert!(!clears_on_disconnect(None, None));
        assert!(!clears_on_disconnect(Some(7), None));
    }

    #[test]
    fn tab_colors_are_theme_names_or_hex() {
        assert_eq!(TabColor::parse("Red"), Ok(TabColor::Ansi(1)));
        assert_eq!(TabColor::parse("purple"), TabColor::parse("magenta"));
        assert_eq!(TabColor::parse("#FF8000"), Ok(TabColor::Rgb(255, 128, 0)));
        assert!(TabColor::parse("#ff80").is_err());
        assert!(TabColor::parse("orange").unwrap_err().contains("not \"orange\""));
    }

    #[test]
    fn opacity_sets_the_frame_and_panes() {
        assert_eq!(backdrop_alphas(1.0), (1.0, 1.0));
        let (frame, pane) = backdrop_alphas(0.8);
        assert_eq!(frame, 0.8);
        // Panes are drawn over the frame: together a little more solid than it.
        let combined = pane + frame * (1.0 - pane);
        assert!(combined > 0.8 && combined < 0.9, "{combined}");
    }
}
