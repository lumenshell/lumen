//! The saved pane layout: tabs, split trees with their ratios, and each pane's
//! working directory. Stored as JSON in the platform's state directory.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    /// Children side by side.
    Row,
    /// Children stacked.
    Column,
}

#[derive(Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub version: u32,
    /// Absent in files written before window state was saved.
    #[serde(default)]
    pub window: Option<WindowState>,
    pub active_tab: usize,
    pub tabs: Vec<TabState>,
}

/// Window placement in logical pixels. `x`/`y`/`width`/`height` are always the
/// normal (un-maximized) bounds, so un-maximizing after a restore lands right.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Debug)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub fullscreen: bool,
}

#[derive(Serialize, Deserialize, PartialEq)]
pub struct TabState {
    pub root: NodeState,
    /// Index of the focused pane in depth-first order.
    pub active: usize,
    pub zoomed: bool,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NodeState {
    Pane { cwd: Option<PathBuf> },
    Split { axis: Axis, children: Vec<NodeState>, ratios: Vec<f32> },
}

impl Session {
    pub fn new(window: Option<WindowState>, active_tab: usize, tabs: Vec<TabState>) -> Self {
        Self { version: VERSION, window, active_tab, tabs }
    }
}

impl NodeState {
    /// Repair anything a hand-edited or older file could get wrong, so the
    /// workspace can trust the tree. Returns None for an empty split.
    fn sanitize(self) -> Option<NodeState> {
        match self {
            NodeState::Pane { cwd } => Some(NodeState::Pane { cwd: cwd.filter(|p| p.is_dir()) }),
            NodeState::Split { axis, children, ratios } => {
                let (mut kept, mut kept_ratios) = (Vec::new(), Vec::new());
                for (i, child) in children.into_iter().enumerate() {
                    if let Some(child) = child.sanitize() {
                        let r = ratios.get(i).copied().unwrap_or(1.0);
                        kept_ratios.push(if r.is_finite() && r > 0.0 { r } else { 1.0 });
                        kept.push(child);
                    }
                }
                match kept.len() {
                    0 => None,
                    1 => kept.pop(),
                    _ => Some(NodeState::Split { axis, children: kept, ratios: kept_ratios }),
                }
            }
        }
    }
}

fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home?.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".local/state")))?
    };
    Some(base.join("lumen").join("session.json"))
}

pub fn load() -> Option<Session> {
    let text = std::fs::read_to_string(path()?).ok()?;
    let session: Session = serde_json::from_str(&text).ok()?;
    if session.version != VERSION {
        return None;
    }
    let tabs: Vec<TabState> = session
        .tabs
        .into_iter()
        .filter_map(|t| Some(TabState { root: t.root.sanitize()?, ..t }))
        .collect();
    if tabs.is_empty() {
        return None;
    }
    let active_tab = session.active_tab.min(tabs.len() - 1);
    let window = session.window.filter(|w| {
        [w.x, w.y, w.width, w.height].iter().all(|v| v.is_finite())
            && w.width > 0.0
            && w.height > 0.0
    });
    Some(Session::new(window, active_tab, tabs))
}

/// Write the session atomically. An empty session (every shell exited) removes
/// the file so the next launch starts fresh.
pub fn save(session: &Session) {
    let Some(path) = path() else { return };
    if session.tabs.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let Ok(json) = serde_json::to_string_pretty(session) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}
