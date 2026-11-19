//! The tree of split panes in a tab: its layout, the dividers you drag, and each pane's frame.

use super::*;

const RADIUS: f32 = 10.0;

/// Smallest a pane may be dragged to, in pixels along the split axis.
const MIN_PANE: f32 = 80.0;

pub(super) enum PaneNode {
    Leaf(Entity<TerminalView>),
    /// `ratios[i]` is child i's share of the space; they need not sum to 1.
    Split {
        axis: Axis,
        children: Vec<PaneNode>,
        ratios: Vec<f32>,
    },
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
pub(super) struct Rect {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) w: f32,
    pub(super) h: f32,
}

impl PaneNode {
    pub(super) fn leaves(&self, out: &mut Vec<Entity<TerminalView>>) {
        match self {
            PaneNode::Leaf(e) => out.push(e.clone()),
            PaneNode::Split { children, .. } => children.iter().for_each(|c| c.leaves(out)),
        }
    }

    /// What the session file keeps. Plugin panes aren't kept (their plugin
    /// opens them again if it wants), nor is the plugin manager, so a split
    /// may lose some sides, or all.
    pub(super) fn to_state(&self, cx: &App) -> Option<NodeState> {
        match self {
            PaneNode::Leaf(term) => {
                let view = term.read(cx);
                view.kept_in_session().then(|| NodeState::Pane { cwd: view.cwd() })
            }
            PaneNode::Split { axis, children, ratios } => {
                let (children, ratios): (Vec<_>, Vec<_>) =
                    children.iter().zip(ratios).filter_map(|(c, r)| Some((c.to_state(cx)?, *r))).unzip();
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

    pub(super) fn rects(&self, r: Rect, out: &mut Vec<(EntityId, Rect)>) {
        match self {
            PaneNode::Leaf(e) => out.push((e.entity_id(), r)),
            PaneNode::Split { axis, children, ratios } => {
                let total: f32 = ratios.iter().sum();
                let mut start = 0.0;
                for (child, ratio) in children.iter().zip(ratios) {
                    let share = ratio / total;
                    let cr = match axis {
                        Axis::Row => Rect { x: r.x + r.w * start, y: r.y, w: r.w * share, h: r.h },
                        Axis::Column => Rect { x: r.x, y: r.y + r.h * start, w: r.w, h: r.h * share },
                    };
                    child.rects(cr, out);
                    start += share;
                }
            }
        }
    }

    pub(super) fn split(&mut self, target: EntityId, new: &Entity<TerminalView>, axis: Axis) -> bool {
        match self {
            PaneNode::Leaf(e) => {
                if e.entity_id() != target {
                    return false;
                }
                let old = PaneNode::Leaf(e.clone());
                *self =
                    PaneNode::Split { axis, children: vec![old, PaneNode::Leaf(new.clone())], ratios: vec![1.0, 1.0] };
                true
            }
            PaneNode::Split { axis: a, children, ratios } => {
                if *a == axis
                    && let Some(i) =
                        children.iter().position(|c| matches!(c, PaneNode::Leaf(e) if e.entity_id() == target))
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
    pub(super) fn set_share(&mut self, new: EntityId, share: f32) -> bool {
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
    pub(super) fn remove(&mut self, target: EntityId) -> bool {
        let PaneNode::Split { children, ratios, .. } = self else { return false };
        let found =
            if let Some(i) = children.iter().position(|c| matches!(c, PaneNode::Leaf(e) if e.entity_id() == target)) {
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

impl Workspace {
    /// Move divider `drag.index` so it sits under the mouse. `bounds` are the
    /// split container's bounds.
    fn drag_divider(&mut self, drag: &SplitDrag, mouse: Point<Pixels>, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        if drag.tab != self.active_tab {
            return;
        }
        let gap = Settings::get(cx).pane_gap;
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
        let axis = *axis;
        if drag_ratios(ratios, i, f32::from(pos), f32::from(size), gap) {
            self.dragging = Some(axis);
            cx.notify();
        }
    }

    fn equalize_split(&mut self, path: &[usize], cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        if let Some(PaneNode::Split { ratios, .. }) = tab.root.at_path_mut(path) {
            ratios.iter_mut().for_each(|r| *r = 1.0);
            cx.notify();
        }
    }

    pub(super) fn render_node(
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
                        items.push(self.render_divider(
                            row,
                            path.clone(),
                            i - 1,
                            theme,
                            Settings::get(cx).pane_gap,
                            cx,
                        ));
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
                    .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<SplitDrag>, _window, cx| {
                        let drag = e.drag(cx).clone();
                        if drag.path == path {
                            this.drag_divider(&drag, e.event.position, e.bounds, cx);
                        }
                    }))
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
        gap: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab = self.active_tab;
        // The handle stays grabbable even when the gap is narrow or zero: it
        // overhangs the panes on both sides.
        let grab = gap.max(8.0);
        let overhang = (grab - gap) / 2.0;
        let name: SharedString = format!("divider-{tab}-{path:?}-{index}").into();
        let pill = alpha(theme.accent, 0.75);
        let equalize_path = path.clone();
        // Negative margins: it takes up only `gap` in the layout, but its
        // hitbox is `grab` wide, over the edges of the panes on either side.
        let handle = div()
            .id(ElementId::Name(name.clone()))
            .group(name.clone())
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .when(row, |d| d.w(px(grab)).mx(px(-overhang)).h_full().cursor(CursorStyle::ResizeLeftRight))
            .when(!row, |d| d.h(px(grab)).my(px(-overhang)).w_full().cursor(CursorStyle::ResizeUpDown))
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
            // Takes the mouse from the panes it overhangs.
            .occlude();
        // Drawn after the panes, so the overhang sits on top of them.
        gpui::deferred(handle).into_any_element()
    }
}

/// Move the divider after child `i` of a split to `pos` along a split `size`
/// long whose children are `gap` apart. Children are flex-basis 0 with
/// flex-grow = ratio, so the space left after the gaps is shared in
/// proportion to the ratios. False when there's no room to move.
fn drag_ratios(ratios: &mut [f32], i: usize, pos: f32, size: f32, gap: f32) -> bool {
    let free = size - gap * (ratios.len() - 1) as f32;
    if free <= 0.0 || i + 1 >= ratios.len() {
        return false;
    }
    let total: f32 = ratios.iter().sum();
    let before: f32 = ratios[..i].iter().sum();
    let pair = ratios[i] + ratios[i + 1];
    let target = (pos - i as f32 * gap - gap / 2.0) / free * total - before;
    let min = (MIN_PANE / free * total).min(pair / 2.0);
    let left = target.clamp(min, pair - min);
    ratios[i] = left;
    ratios[i + 1] = pair - left;
    true
}

/// Height of the strip that names a split pane.
const PANE_LABEL_HEIGHT: f32 = 22.0;

pub(super) fn pane_frame(
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
    let dim = Settings::get(cx).unfocused_dim.unwrap_or(if theme.is_light { 0.25 } else { 0.35 });
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
                    .font_family(ui_font())
                    .text_size(px(11.5))
                    .text_color(if focused { theme.text } else { theme.subtext })
                    .overflow_hidden()
                    .child(div().flex_none().size(px(7.0)).rounded_full().bg(tint.unwrap_or(theme.muted)))
                    .child(div().min_w_0().truncate().child(label)),
            )
        })
        .child(div().flex_1().min_h_0().w_full().p(px(padding)).child(
            // Reuse the pane's last frame unless it changed, so output in one
            // pane (or hovering a tab) doesn't redraw every other pane.
            AnyView::from(term.clone()).cached(StyleRefinement::default().size_full()),
        ))
        .when(multi && !focused && dim > 0.0, |d| {
            d.child(div().absolute().top_0().left_0().size_full().bg(alpha(theme.shell, dim)))
        })
        .into_any_element()
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
    let content = (f32::from(window.width) - 2.0 * EDGE, f32::from(window.height) - TITLEBAR_HEIGHT - EDGE);
    match saved {
        Some(saved) => layout_panes(&saved.tabs, content, settings.pane_gap, &grid),
        None => vec![(None, grid(content.0, content.1))],
    }
}

/// Each pane of `tabs` with the grid `grid` gives the room it gets when the
/// tab's content is `content` (width, height) pixels.
fn layout_panes(
    tabs: &[TabState],
    content: (f32, f32),
    gap: f32,
    grid: &dyn Fn(f32, f32) -> GridSize,
) -> Vec<(Option<PathBuf>, GridSize)> {
    fn walk(
        node: &NodeState,
        (w, h): (f32, f32),
        gap: f32,
        grid: &dyn Fn(f32, f32) -> GridSize,
        out: &mut Vec<(Option<PathBuf>, GridSize)>,
    ) {
        match node {
            NodeState::Pane { cwd } => out.push((cwd.clone(), grid(w, h))),
            NodeState::Split { axis, children, ratios } => {
                let along = if *axis == Axis::Row { w } else { h };
                let room = (along - gap * children.len().saturating_sub(1) as f32).max(0.0);
                let total: f32 = ratios.iter().sum::<f32>().max(f32::EPSILON);
                for (child, r) in children.iter().zip(ratios) {
                    let share = room * r / total;
                    walk(child, if *axis == Axis::Row { (share, h) } else { (w, share) }, gap, grid, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    for tab in tabs {
        let first = out.len();
        walk(&tab.root, content, gap, grid, &mut out);
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
    fn layout_with_no_gap() {
        let grid = |w: f32, h: f32| GridSize { cols: w as u16, rows: h as u16, cell_width: 1, cell_height: 1 };
        let pane = || NodeState::Pane { cwd: Some("/x".into()) };
        let tabs = vec![TabState {
            root: NodeState::Split {
                axis: Axis::Row,
                children: vec![pane(), pane(), pane()],
                ratios: vec![1.0, 1.0, 1.0],
            },
            active: 0,
            zoomed: false,
            title: None,
            color: None,
        }];
        let widths: Vec<u16> = layout_panes(&tabs, (900.0, 100.0), 0.0, &grid).iter().map(|(_, g)| g.cols).collect();
        assert_eq!(widths, [300, 300, 300]);
        let mut ratios: Vec<f32> = vec![1.0, 1.0, 1.0];
        assert!(drag_ratios(&mut ratios, 0, 450.0, 900.0, 0.0));
        assert!(ratios.iter().all(|r| r.is_finite() && *r > 0.0), "{ratios:?}");
        assert!((ratios[0] - 1.5).abs() < 0.01, "{ratios:?}");
    }

    #[test]
    fn startup_panes_get_their_share_of_the_window() {
        // One "cell" per pixel, so sizes read as pixels.
        let grid = |w: f32, h: f32| GridSize { cols: w as u16, rows: h as u16, cell_width: 1, cell_height: 1 };
        let pane = |cwd: &str| NodeState::Pane { cwd: Some(cwd.into()) };
        let tab = |root, active, zoomed| TabState { root, active, zoomed, title: None, color: None };
        let tabs = vec![
            tab(
                NodeState::Split {
                    axis: Axis::Row,
                    children: vec![
                        pane("/a"),
                        NodeState::Split {
                            axis: Axis::Column,
                            children: vec![pane("/b"), pane("/c")],
                            ratios: vec![1.0, 3.0],
                        },
                    ],
                    ratios: vec![1.0, 1.0],
                },
                0,
                false,
            ),
            tab(
                NodeState::Split { axis: Axis::Row, children: vec![pane("/d"), pane("/e")], ratios: vec![1.0, 1.0] },
                1,
                true,
            ),
        ];
        let got: Vec<(String, u16, u16)> = layout_panes(&tabs, (408.0, 208.0), 8.0, &grid)
            .into_iter()
            .map(|(cwd, g)| (cwd.unwrap().display().to_string(), g.cols, g.rows))
            .collect();
        let want = [("/a", 200, 208), ("/b", 200, 50), ("/c", 200, 150), ("/d", 200, 208), ("/e", 408, 208)];
        assert_eq!(got, want.map(|(c, w, h)| (c.to_string(), w, h)));
    }
}
