//! The window: custom title bar with tabs, a padded canvas of split panes, and the palette.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, AnyView, App, AppContext, Bounds, BoxShadow, Context, CursorStyle, Decorations, DragMoveEvent,
    ElementId, Entity, EntityId, Focusable, HitboxBehavior, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Point, Render, ResizeEdge, Rgba, SharedString, Size,
    StatefulInteractiveElement, StyleRefinement, Styled, Subscription, Task, Window, WindowBackgroundAppearance,
    WindowBounds, canvas, div, font, point, prelude::FluentBuilder, px, relative,
};

use serde_json::{Value, json};

use crate::api::{self, ApiError, ApiResult};
use crate::keymap;
use crate::palette::{CommandPalette, Dot, PaletteEvent, PaletteFn, PaletteItem};
use crate::pty::{GridSize, Owner};
use crate::session::{self, Axis, NodeState, Session, TabState, WindowState};
use crate::settings::{MONO_FONT, Settings, WorkingDirectory, ui_font};
use crate::terminal_view::{TerminalEvent, TerminalView};
use crate::theme::{Theme, alpha};
use crate::*;
use std::path::PathBuf;

mod palettes;
mod panes;
mod plugin_host;
mod socket_api;
mod titlebar;

pub use panes::startup_panes;
use panes::*;
use plugin_host::*;
use socket_api::*;
pub use titlebar::TabColor;
use titlebar::*;

/// Space between the panes and the window's edges.
const EDGE: f32 = 8.0;
/// Height of the title bar with the tabs.
const TITLEBAR_HEIGHT: f32 = 44.0;
const WINDOW_RADIUS: f32 = 12.0;
pub const SHADOW_INSET: f32 = 10.0;

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
    /// A colour you picked for the tab in the palette. Saved, and shown
    /// over `color`; a plugin stopping leaves it be.
    own_color: Option<TabColor>,
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
    /// A program run directly, without a shell (`argv`).
    Program(Vec<String>),
}

type PaneParts = (Option<std::path::PathBuf>, Option<Owner>, Option<Vec<String>>);

impl NewPane {
    fn into_parts(self) -> PaneParts {
        match self {
            NewPane::Shell(cwd) => (cwd, None, None),
            NewPane::Plugin(owner) => (None, Some(owner), None),
            NewPane::Program(argv) => (None, None, Some(argv)),
        }
    }
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
    /// Wrong values in `[plugin.NAME]` tables, shown with config.toml's
    /// own problems.
    plugin_setting_problems: Vec<String>,
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
        cx.observe_window_activation(window, |_, window, cx| {
            crate::quick::activation_changed(window.is_window_active(), cx)
        })
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
            plugin_setting_problems: Vec::new(),
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
        // A light/dark theme pair follows the system (read now too, in case
        // the desktop answered after startup).
        Settings::set_appearance(cx, window.appearance());
        this._subs.push(cx.observe_window_appearance(window, |_, window, cx| {
            Settings::set_appearance(cx, window.appearance());
        }));
        // Built-in plugins turned on or off (in Plugins or config.toml).
        // A plugin installed, updated or approved by `lumen plugins`.
        this._subs.push(cx.observe_global_in::<crate::plugins::PluginFiles>(window, |this, window, cx| {
            this.sync_plugins(window, cx);
        }));
        this._subs.push(cx.observe_global_in::<Settings>(window, |this, window, cx| {
            if Settings::get(cx).plugins != crate::plugins::builtins_running() {
                this.sync_plugins(window, cx);
            } else {
                this.push_plugin_settings(cx);
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
                own_color: tab.color.as_deref().and_then(|c| TabColor::parse(c).ok()),
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
            (Some(b), _) => {
                (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height))
            }
            (None, Some(p)) => (p.x, p.y, p.width, p.height),
            (None, None) => {
                let b = window.inner_window_bounds().get_bounds();
                (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height))
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
            let kept: Vec<_> = tab.leaves().into_iter().filter(|l| l.read(cx).kept_in_session()).collect();
            tabs.push(TabState {
                root,
                active: kept.iter().position(|l| *l == tab.active).unwrap_or(0),
                zoomed: tab.zoomed && kept.len() > 1,
                title: tab.title.clone(),
                color: tab.own_color.map(TabColor::name),
            });
        }
        Session::new(self.window_state, active_tab, tabs)
    }

    fn save_session(&mut self, cx: &App) {
        session::record(self.window_id, self.snapshot(cx));
    }

    fn spawn_terminal(&mut self, what: NewPane, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalView> {
        let (cwd, owner, program) = what.into_parts();
        let term = TerminalView::open(cwd, owner, program, window, cx);
        api::emit("pane-opened", json!({ "pane": term.entity_id().as_u64() }));
        let sub = cx.subscribe_in(&term, window, |this, term, event, window, cx| match event {
            TerminalEvent::Attention => cx.notify(),
            TerminalEvent::TitleChanged => {
                api::emit("title-changed", json!({ "pane": term.entity_id().as_u64(), "title": term.read(cx).title }));
                cx.notify()
            }
            TerminalEvent::Focused => {
                api::emit("focus-changed", json!({ "pane": term.entity_id().as_u64() }));
                // Only track the active pane within its own tab; switching tabs is
                // driven explicitly so a late focus event can't snap back.
                if let Some(tab) = this.tabs.iter_mut().find(|t| t.leaves().iter().any(|l| l == term)) {
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

    /// Where a new tab, split or window starts (`working-directory`).
    fn current_cwd(&self, cx: &App) -> Option<std::path::PathBuf> {
        let inherited = self.active_terminal().and_then(|t| t.read(cx).cwd());
        let home = std::env::var_os("HOME").map(PathBuf::from);
        start_dir(&Settings::get(cx).working_directory, inherited, home)
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
    fn open_tab(&mut self, what: NewPane, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalView> {
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
                own_color: None,
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
        let Some(tab_idx) = self.tabs.iter().position(|t| t.leaves().iter().any(|l| l.entity_id() == id)) else {
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
}

fn problem_banner(problems: &[String], plugin_problems: &[String], theme: &Theme) -> impl IntoElement {
    let path = settings::config_path()
        .map(|p| p.display().to_string().replace(&std::env::var("HOME").unwrap_or_default(), "~"))
        .unwrap_or_else(|| "config.toml".into());
    div().absolute().bottom(px(EDGE * 2.0)).left_0().right_0().flex().justify_center().child(
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
            .font_family(ui_font())
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
                    div().text_color(theme.ansi[1]).child(format!("Problem in {path} (other settings still apply)")),
                )
                .children(problems.iter().take(4).map(|p| div().text_color(theme.subtext).child(p.clone())))
            })
            .when(!plugin_problems.is_empty(), |d| {
                d.child(div().text_color(theme.ansi[1]).child("Plugins that couldn't be loaded"))
                    .children(plugin_problems.iter().take(4).map(|p| div().text_color(theme.subtext).child(p.clone())))
            }),
    )
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

/// Where a new pane starts, given the `working-directory` setting, the
/// focused pane's folder and home. A folder that's gone falls back to home.
fn start_dir(setting: &WorkingDirectory, inherited: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    match setting {
        WorkingDirectory::Inherit => inherited,
        WorkingDirectory::Home => home,
        WorkingDirectory::Path(p) if p.is_dir() => Some(p.clone()),
        WorkingDirectory::Path(_) => home,
    }
}

/// A fresh window's session when `working-directory` names a folder (home or
/// a path): one tab, one pane, there. `None` for "inherit", which keeps the
/// usual start (Lumen's own folder, or home).
pub fn fresh_session(setting: &WorkingDirectory, home: Option<PathBuf>) -> Option<Session> {
    if *setting == WorkingDirectory::Inherit {
        return None;
    }
    let cwd = start_dir(setting, None, home)?;
    Some(Session::new(
        None,
        0,
        vec![TabState { root: NodeState::Pane { cwd: Some(cwd) }, active: 0, zoomed: false, title: None, color: None }],
    ))
}

fn resize_edge(pos: Point<Pixels>, inset: Pixels, size: Size<Pixels>) -> Option<ResizeEdge> {
    let corner = inset * 2.0;
    let (l, r) = (pos.x < corner, pos.x > size.width - corner);
    let (t, b) = (pos.y < corner, pos.y > size.height - corner);
    let edge = if pos.y < inset {
        if l {
            ResizeEdge::TopLeft
        } else if r {
            ResizeEdge::TopRight
        } else {
            ResizeEdge::Top
        }
    } else if pos.y > size.height - inset {
        if l {
            ResizeEdge::BottomLeft
        } else if r {
            ResizeEdge::BottomRight
        } else {
            ResizeEdge::Bottom
        }
    } else if pos.x < inset {
        if t {
            ResizeEdge::TopLeft
        } else if b {
            ResizeEdge::BottomLeft
        } else {
            ResizeEdge::Left
        }
    } else if pos.x > size.width - inset {
        if t {
            ResizeEdge::TopRight
        } else if b {
            ResizeEdge::BottomRight
        } else {
            ResizeEdge::Right
        }
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

        let mut problems = Settings::get(cx).problems.clone();
        problems.extend(self.plugin_setting_problems.iter().cloned());
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
            .font_family(ui_font())
            .cursor(CursorStyle::Arrow)
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .when_some(tiling, |d, tiling| {
                d.border_1().border_color(alpha(theme.text, if theme.is_light { 0.12 } else { 0.07 })).when(
                    !tiling.is_tiled(),
                    |d| {
                        d.rounded(px(WINDOW_RADIUS)).shadow(vec![BoxShadow {
                            color: alpha(gpui::black(), 0.45),
                            offset: point(px(0.0), px(2.0)),
                            blur_radius: inset / 1.5,
                            spread_radius: px(0.0),
                        }])
                    },
                )
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
                    canvas(|_, _, _| {}, move |_, _, window, _| window.set_window_cursor_style(style))
                        .absolute()
                        .size_0(),
                )
            })
            .child(div().flex_1().min_h_0().px(px(EDGE)).pb(px(EDGE)).children(content))
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
                                gpui::Bounds::new(point(px(0.0), px(0.0)), window.window_bounds().get_bounds().size),
                                HitboxBehavior::Normal,
                            )
                        },
                        move |_bounds, hitbox, window, _cx| {
                            let mouse = window.mouse_position();
                            let size = window.window_bounds().get_bounds().size;
                            if let Some(edge) = resize_edge(mouse, inset, size) {
                                window.set_cursor_style(
                                    match edge {
                                        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                                        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_dir_follows_setting() {
        let inherited = Some(PathBuf::from("/inherited"));
        let home = Some(PathBuf::from("/home/me"));
        assert_eq!(start_dir(&WorkingDirectory::Inherit, inherited.clone(), home.clone()), inherited);
        assert_eq!(start_dir(&WorkingDirectory::Home, inherited.clone(), home.clone()), home);
        let tmp = std::env::temp_dir();
        assert_eq!(start_dir(&WorkingDirectory::Path(tmp.clone()), inherited.clone(), home.clone()), Some(tmp));
        let gone = WorkingDirectory::Path(PathBuf::from("/nonexistent-lumen-test"));
        assert_eq!(start_dir(&gone, inherited, home.clone()), home);
    }

    #[test]
    fn fresh_windows_follow_working_directory() {
        let home = Some(PathBuf::from("/home/me"));
        assert!(fresh_session(&WorkingDirectory::Inherit, home.clone()).is_none());
        let cwd = |s: Option<Session>| match s.as_ref().map(|s| &s.tabs[0].root) {
            Some(NodeState::Pane { cwd }) => cwd.clone(),
            _ => None,
        };
        assert_eq!(cwd(fresh_session(&WorkingDirectory::Home, home.clone())), home);
        let tmp = std::env::temp_dir();
        assert_eq!(cwd(fresh_session(&WorkingDirectory::Path(tmp.clone()), home)), Some(tmp));
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
