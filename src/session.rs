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
    /// A name you gave the tab, shown instead of the program's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
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

/// What's saved: every open window, in the order they opened.
#[derive(Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    windows: Vec<Session>,
}

/// Version of the file that lists windows. Version 1 held a single window
/// (a bare [`Session`]) and still loads.
const FILE_VERSION: u32 = 2;

/// The saved windows, repaired; empty when there's nothing to restore.
pub fn load() -> Vec<Session> {
    let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    parse(&text)
}

fn parse(text: &str) -> Vec<Session> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    let windows = match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(1) => serde_json::from_value::<Session>(value).map(|s| vec![s]).unwrap_or_default(),
        Some(2) => serde_json::from_value::<SessionFile>(value).map(|f| f.windows).unwrap_or_default(),
        _ => Vec::new(),
    };
    windows.into_iter().filter_map(clean).collect()
}

/// Repair one window's saved state; None if it has no panes left.
fn clean(session: Session) -> Option<Session> {
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

/// The windows open now, by window id in the order they opened, with their
/// latest state (`None` until a window first saves).
static OPEN: std::sync::Mutex<Vec<(u64, Option<Session>)>> = std::sync::Mutex::new(Vec::new());

/// A new window: it's saved in this position from now on.
pub fn register(window: u64) {
    OPEN.lock().unwrap().push((window, None));
}

/// Record a window's state, writing the file if anything changed. A window
/// with no tabs left is dropped from the file.
pub fn record(window: u64, session: Session) {
    let mut open = OPEN.lock().unwrap();
    let Some(entry) = open.iter_mut().find(|(id, _)| *id == window) else { return };
    let session = (!session.tabs.is_empty()).then_some(session);
    if entry.1 == session {
        return;
    }
    entry.1 = session;
    write(&open);
}

/// A window closed. The last one stays in the file, to come back next launch
/// (unless its shells all exited, which [`record`] already noted).
pub fn forget(window: u64) {
    let mut open = OPEN.lock().unwrap();
    if open.len() > 1 {
        open.retain(|(id, _)| *id != window);
        write(&open);
    }
}

/// Write every window atomically; with none left, remove the file so the
/// next launch starts fresh.
/// Whether the session file is written (`restore-session`), set at launch.
static SAVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Turn writing the session file on or off. Off leaves the file as it is.
pub fn set_saving(on: bool) {
    SAVING.store(on, std::sync::atomic::Ordering::Relaxed);
}

fn write(open: &[(u64, Option<Session>)]) {
    write_to(path(), open, SAVING.load(std::sync::atomic::Ordering::Relaxed));
}

fn write_to(path: Option<PathBuf>, open: &[(u64, Option<Session>)], saving: bool) {
    if !saving {
        return;
    }
    let Some(path) = path else { return };
    let windows: Vec<&Session> = open.iter().filter_map(|(_, s)| s.as_ref()).collect();
    if windows.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let file = serde_json::json!({ "version": FILE_VERSION, "windows": windows });
    let Ok(json) = serde_json::to_string_pretty(&file) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn no_restore_leaves_the_session_file() {
        let dir = std::env::temp_dir().join(format!("lumen-session-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("session.json");
        std::fs::write(&file, "old").unwrap();
        let open = vec![(1, Some(super::Session::new(None, 0, Vec::new())))];
        super::write_to(Some(file.clone()), &open, false);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");
        super::write_to(Some(file.clone()), &[], false);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old", "not even removed");
        std::fs::remove_dir_all(dir).unwrap();
    }

    use super::*;

    fn window(cwd: &str) -> Session {
        Session::new(None, 0, vec![TabState {
            root: NodeState::Pane { cwd: Some(cwd.into()) },
            active: 0,
            zoomed: false,
            title: None,
        }])
    }

    #[test]
    fn old_single_window_files_still_load() {
        let old = serde_json::to_string(&window("/")).unwrap();
        assert_eq!(parse(&old).len(), 1);
    }

    #[test]
    fn every_window_is_saved_and_empty_ones_are_skipped() {
        let mut empty = window("/");
        empty.tabs.clear();
        let file = serde_json::json!({ "version": 2, "windows": [window("/"), empty, window("/tmp")] });
        let windows = parse(&file.to_string());
        assert_eq!(windows.len(), 2);
        assert!(parse("{\"version\": 3, \"windows\": []}").is_empty());
        assert!(parse("not json").is_empty());
    }
}
