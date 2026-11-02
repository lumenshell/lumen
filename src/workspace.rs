//! The window: custom title bar with tabs, a padded canvas of split panes, and the palette.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, AnyView, App, AppContext, Bounds, BoxShadow, Context, CursorStyle, Decorations,
    DragMoveEvent, ElementId, Entity, EntityId, Focusable, HitboxBehavior, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, ResizeEdge,
    SharedString, Size, StatefulInteractiveElement, StyleRefinement, Styled, Subscription, Task,
    Window,
    WindowBounds, canvas, div,
    point, prelude::FluentBuilder, px, relative,
};

use serde_json::{Value, json};

use crate::api::{self, ApiError, ApiResult};
use crate::keymap;
use crate::palette::{CommandPalette, PaletteEvent, PaletteItem};
use crate::session::{self, Axis, NodeState, Session, TabState, WindowState};
use crate::settings::{MONO_FONT, Settings, UI_FONT};
use crate::terminal_view::{TerminalEvent, TerminalView};
use crate::theme::{Theme, alpha};
use crate::*;

const GAP: f32 = 8.0;
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

    fn to_state(&self, cx: &App) -> NodeState {
        match self {
            PaneNode::Leaf(term) => NodeState::Pane { cwd: term.read(cx).cwd() },
            PaneNode::Split { axis, children, ratios } => NodeState::Split {
                axis: *axis,
                children: children.iter().map(|c| c.to_state(cx)).collect(),
                ratios: ratios.clone(),
            },
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
}

impl Tab {
    fn leaves(&self) -> Vec<Entity<TerminalView>> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }
}

pub struct Workspace {
    tabs: Vec<Tab>,
    active_tab: usize,
    palette: Option<Entity<CommandPalette>>,
    /// Axis of the divider being dragged, for the window-wide resize cursor.
    dragging: Option<Axis>,
    /// Last layout written to disk, so autosave only writes on change.
    last_saved: Option<Session>,
    saved_on_quit: bool,
    window_state: Option<WindowState>,
    _autosave: Task<()>,
    _subs: Vec<Subscription>,
}

impl Workspace {
    pub fn new(saved: Option<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            tabs: Vec::new(),
            active_tab: 0,
            palette: None,
            dragging: None,
            last_saved: None,
            saved_on_quit: false,
            window_state: saved.as_ref().and_then(|s| s.window),
            _autosave: Task::ready(()),
            _subs: Vec::new(),
        };
        match saved {
            Some(saved) => this.restore(saved, window, cx),
            None => this.new_tab(&NewTab, window, cx),
        }
        this.track_window(window);
        this._subs.push(cx.observe_window_bounds(window, |this, window, _| this.track_window(window)));

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
            }
        }));
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
            self.tabs.push(Tab { root, active, zoomed, title: None });
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
            NodeState::Pane { cwd } => PaneNode::Leaf(self.spawn_terminal(cwd, window, cx)),
            NodeState::Split { axis, children, ratios } => PaneNode::Split {
                axis,
                children: children.into_iter().map(|c| self.build_node(c, window, cx)).collect(),
                ratios,
            },
        }
    }

    fn snapshot(&self, cx: &App) -> Session {
        let tabs = self
            .tabs
            .iter()
            .map(|tab| TabState {
                root: tab.root.to_state(cx),
                active: tab.leaves().iter().position(|l| *l == tab.active).unwrap_or(0),
                zoomed: tab.zoomed,
            })
            .collect();
        Session::new(self.window_state, self.active_tab, tabs)
    }

    fn save_session(&mut self, cx: &App) {
        let snapshot = self.snapshot(cx);
        if self.last_saved.as_ref() != Some(&snapshot) {
            session::save(&snapshot);
            self.last_saved = Some(snapshot);
        }
    }

    fn spawn_terminal(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        let term = cx.new(|cx| TerminalView::new(cwd, window, cx));
        api::emit("pane-opened", json!({ "pane": term.entity_id().as_u64() }));
        let sub = cx.subscribe_in(&term, window, |this, term, event, window, cx| match event {
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

    fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(t) = self.active_terminal() {
            window.focus(&t.focus_handle(cx));
        }
    }

    fn current_cwd(&self, cx: &App) -> Option<std::path::PathBuf> {
        self.active_terminal().and_then(|t| t.read(cx).cwd())
    }

    // ---- actions --------------------------------------------------------

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.current_cwd(cx);
        self.open_tab(cwd, window, cx);
    }

    /// Open a tab after the current one and switch to it.
    fn open_tab(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        let term = self.spawn_terminal(cwd, window, cx);
        let insert_at = if self.tabs.is_empty() { 0 } else { self.active_tab + 1 };
        self.tabs.insert(
            insert_at,
            Tab { root: PaneNode::Leaf(term.clone()), active: term.clone(), zoomed: false, title: None },
        );
        self.active_tab = insert_at;
        self.focus_active(window, cx);
        cx.notify();
        term
    }

    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.active_terminal().map(|t| t.entity_id()) else { return };
        let cwd = self.current_cwd(cx);
        self.split_pane(target, axis, cwd, true, window, cx);
    }

    /// Split the pane `target` (in any tab), putting a new pane after it. With
    /// `focus`, the new pane becomes active and its tab is shown.
    fn split_pane(
        &mut self,
        target: EntityId,
        axis: Axis,
        cwd: Option<std::path::PathBuf>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalView>> {
        let tab_idx = self.tabs.iter().position(|t| t.leaves().iter().any(|l| l.entity_id() == target))?;
        let new = self.spawn_terminal(cwd, window, cx);
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
                cx.quit();
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

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len();
        self.activate_tab((self.active_tab + 1) % n, window, cx);
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len();
        self.activate_tab((self.active_tab + n - 1) % n, window, cx);
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
        if self.palette.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let items = palette_items(window, cx);
        let palette = cx.new(|cx| CommandPalette::new(items, cx));
        let sub = cx.subscribe_in(&palette, window, |this, _, event, window, cx| match event {
            PaletteEvent::Dismiss => {
                this.palette = None;
                this.focus_active(window, cx);
                cx.notify();
            }
        });
        self._subs.push(sub);
        window.focus(&palette.focus_handle(cx));
        self.palette = Some(palette);
        cx.notify();
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
                pane_frame(term, focused, multi, theme, Settings::get(cx).padding)
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
        &self,
        theme: &Theme,
        decorations: Decorations,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tabs = self.tabs.iter().enumerate().map(|(i, tab)| {
            let active = i == self.active_tab;
            let title: SharedString =
                tab.title.clone().unwrap_or_else(|| tab.active.read(cx).title.clone()).into();
            let panes = tab.leaves().len();
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
                .max_w(px(220.0))
                .cursor_pointer()
                .text_color(if active { theme.text } else { theme.muted })
                .when(active, |d| {
                    d.bg(theme.raised).shadow(vec![BoxShadow {
                        color: alpha(gpui::black(), 0.18),
                        offset: point(px(0.0), px(1.0)),
                        blur_radius: px(3.0),
                        spread_radius: px(0.0),
                    }])
                })
                .when(!active, |d| d.hover(|d| d.bg(alpha(theme.raised, 0.55))))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.activate_tab(i, window, cx);
                    }),
                )
                .child(
                    div()
                        .font_family(MONO_FONT)
                        .text_size(px(11.0))
                        .text_color(if active { theme.accent } else { theme.muted })
                        .child(format!("{}", i + 1)),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(title),
                )
                .when(panes > 1, |d| {
                    d.child(
                        div()
                            .text_size(px(10.5))
                            .px(px(5.0))
                            .rounded(px(4.0))
                            .bg(alpha(theme.muted, 0.18))
                            .text_color(theme.subtext)
                            .child(format!("{panes}")),
                    )
                })
                .child(
                    div()
                        .id(("tab-close", i))
                        .flex()
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
                        .child("×"),
                )
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
            .child("+");

        let client = matches!(decorations, Decorations::Client { .. });

        div()
            .id("titlebar")
            .flex()
            .flex_none()
            .items_center()
            .h(px(44.0))
            .pl(px(if cfg!(target_os = "macos") { 84.0 } else { 10.0 }))
            .pr(px(10.0))
            .gap(px(4.0))
            .font_family(UI_FONT)
            .text_size(px(12.5))
            .on_mouse_down(MouseButton::Left, |ev, window, _| {
                if ev.click_count >= 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .min_w_0()
                    .overflow_hidden()
                    .children(tabs)
                    .child(new_tab),
            )
            .child(div().flex_1())
            .when(client && !cfg!(target_os = "macos"), |d| d.child(window_controls(theme)))
    }
}

/// Problems in the config file, shown at the bottom of the window until fixed.
fn problem_banner(problems: &[String], theme: &Theme) -> impl IntoElement {
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
                .child(
                    div()
                        .text_color(theme.ansi[1])
                        .child(format!("Problem in {path} (other settings still apply)")),
                )
                .children(problems.iter().take(4).map(|p| div().text_color(theme.subtext).child(p.clone()))),
        )
}

fn pane_frame(
    term: &Entity<TerminalView>,
    focused: bool,
    multi: bool,
    theme: &Theme,
    padding: f32,
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
    div()
        .relative()
        .size_full()
        .rounded(px(RADIUS))
        .bg(theme.pane)
        .border_1()
        .border_color(if focused && multi { alpha(theme.accent, 0.6) } else { theme.border })
        .shadow(shadows)
        .overflow_hidden()
        .child(
            div().size_full().p(px(padding)).child(
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

    fn api_list(&self, cx: &App) -> Value {
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
                        })
                    })
                    .collect();
                json!({
                    "index": i,
                    "title": tab.title.clone().unwrap_or_else(|| tab.active.read(cx).title.clone()),
                    "custom_title": tab.title.is_some(),
                    "active": i == self.active_tab,
                    "zoomed": tab.zoomed,
                    "panes": panes,
                })
            })
            .collect();
        json!({ "tabs": tabs, "focused_pane": focused.map(|f| f.as_u64()) })
    }

    /// Run one socket API request. Methods and their parameters are described
    /// in the README.
    pub fn handle_api(&mut self, method: &str, params: &Value, window: &mut Window, cx: &mut Context<Self>) -> ApiResult {
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
                let dir = cwd()?.or_else(|| self.current_cwd(cx));
                string("run")?;
                let term = self.open_tab(dir, window, cx);
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
                string("run")?;
                let (_, target) = self.target_pane(params)?;
                let dir = cwd()?.or_else(|| target.read(cx).cwd());
                let new = self
                    .split_pane(target.entity_id(), axis, dir, focus, window, cx)
                    .ok_or_else(|| ApiError::failed("that pane just closed"))?;
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
            "set-title" => {
                let title = string("title")?.filter(|t| !t.trim().is_empty());
                let (tab, _) = self.target_pane(params)?;
                self.tabs[tab].title = title;
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
        action::<SplitRight>("Split Right"),
        action::<SplitDown>("Split Down"),
        action::<ClosePane>("Close Pane"),
        action::<ToggleZoom>("Toggle Focus Mode"),
        action::<NextPane>("Focus Next Pane"),
        action::<NextTab>("Next Tab"),
        action::<PrevTab>("Previous Tab"),
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
        action::<Quit>("Quit"),
    ];
    let mut items: Vec<PaletteItem> = actions
        .into_iter()
        .map(|(label, act)| {
            let detail = shortcut_label(act.as_ref(), window);
            let act: Rc<dyn gpui::Action> = act.into();
            PaletteItem {
                label,
                detail,
                run: Rc::new(move |window, cx| window.dispatch_action(act.boxed_clone(), cx)),
            }
        })
        .collect();
    if let Some(path) = settings::config_path() {
        items.push(PaletteItem {
            label: "Open Config File".into(),
            detail: path.display().to_string().replace(&std::env::var("HOME").unwrap_or_default(), "~").into(),
            run: Rc::new(move |_, cx| cx.open_url(&format!("file://{}", path.display()))),
        });
    }
    let settings = Settings::get(cx);
    for (i, theme) in settings.themes.iter().enumerate() {
        items.push(PaletteItem {
            label: format!("Theme: {}", theme.name).into(),
            detail: if i == settings.theme_index { "current".into() } else { "".into() },
            run: Rc::new(move |_, cx| Settings::update(cx, |s| s.theme_index = i)),
        });
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
        let theme = Settings::get(cx).theme().clone();
        let decorations = window.window_decorations();
        let inset = px(SHADOW_INSET);

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
        let content = self.tabs.get(self.active_tab).map(|tab| {
            let multi = tab.leaves().len() > 1;
            if tab.zoomed {
                pane_frame(&tab.active, true, false, &theme, Settings::get(cx).padding)
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
            .on_action(cx.listener(Self::split_right))
            .on_action(cx.listener(Self::split_down))
            .on_action(cx.listener(Self::close_pane))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::activate_tab_action))
            .on_action(cx.listener(Self::next_pane))
            .on_action(cx.listener(Self::toggle_zoom))
            .on_action(cx.listener(Self::toggle_palette))
            .on_action(cx.listener(|this, _: &FocusLeft, w, cx| this.focus_pane(-1.0, 0.0, w, cx)))
            .on_action(cx.listener(|this, _: &FocusRight, w, cx| this.focus_pane(1.0, 0.0, w, cx)))
            .on_action(cx.listener(|this, _: &FocusUp, w, cx| this.focus_pane(0.0, -1.0, w, cx)))
            .on_action(cx.listener(|this, _: &FocusDown, w, cx| this.focus_pane(0.0, 1.0, w, cx)))
            .child(self.render_titlebar(&theme, decorations, cx))
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
                        .pt(px(72.0))
                        .bg(alpha(gpui::black(), 0.25))
                        .when_some(tiling, |d, t| d.when(!t.is_tiled(), |d| d.rounded(px(WINDOW_RADIUS))))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.palette = None;
                                this.focus_active(window, cx);
                                cx.notify();
                            }),
                        )
                        .child(palette),
                )
            })
            .when(!problems.is_empty(), |d| d.child(problem_banner(&problems, &theme)));

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
