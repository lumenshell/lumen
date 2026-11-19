//! Plugins: programs in `~/.config/lumen/plugins/<name>/`, described by a
//! `plugin.toml`, that Lumen starts, supervises and talks to.
//!
//! Each plugin runs as its own process, so a slow or crashing one can't
//! freeze Lumen. It speaks the socket API's JSON-RPC over its stdin and
//! stdout, which tells Lumen which plugin every request comes from: requests
//! are limited to the permissions in its manifest, approved once by you.
//! Plugins can also add palette commands (with key bindings) and show
//! pickers in the palette. What they write to stderr goes to a log file.
//!
//! Permissions limit what a plugin can do through Lumen. They aren't a
//! sandbox: a plugin is a program running as you.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use toml_edit::DocumentMut;

use crate::api::{self, ApiError, ApiResult, Caller};
use crate::keymap;

mod supervisor;

pub use supervisor::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// Open, split, focus and close panes and tabs; set tab titles.
    Panes,
    /// Type into panes and run actions.
    Input,
    /// Read what's on screen, and the commands you run.
    Screen,
}

impl Permission {
    pub const ALL: [Permission; 3] = [Permission::Panes, Permission::Input, Permission::Screen];

    pub fn name(self) -> &'static str {
        match self {
            Permission::Panes => "panes",
            Permission::Input => "input",
            Permission::Screen => "screen",
        }
    }

    /// What it allows, for the approval prompt.
    pub fn describe(self) -> &'static str {
        match self {
            Permission::Panes => "open, close and rearrange panes and tabs",
            Permission::Input => "type into your panes, which can run any command",
            Permission::Screen => "read what's on screen and the commands you run",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    /// The folder name; used in logs, approvals and key bindings.
    pub name: String,
    pub description: String,
    pub version: String,
    /// Program and arguments, run in `dir`.
    pub command: Vec<String>,
    pub permissions: Vec<Permission>,
    /// What `[plugin.NAME]` in config.toml may set.
    pub settings: Vec<crate::plugin_settings::Spec>,
    pub dir: PathBuf,
    /// One of Lumen's built-in plugins, turned on from Plugins in the palette.
    pub builtin: bool,
    /// Installed from the registry: named `owner/name`, approved at install.
    pub registry: bool,
}

impl Manifest {
    /// Its entry in the approvals file. Built-in plugins have their own, so
    /// turning one on never approves a folder of yours with the same name.
    fn approval_key(&self) -> String {
        if self.builtin {
            format!("builtin:{}", self.name)
        } else if self.registry {
            format!("registry:{}", self.name)
        } else {
            self.name.clone()
        }
    }
}

/// Bumped when `installed.json` or `plugin-approvals.json` changes on disk
/// (`lumen plugins` installed, updated or approved something), so windows
/// bring their plugins in line.
#[derive(Default)]
pub struct PluginFiles(pub u64);

impl gpui::Global for PluginFiles {}

/// How many installed registry plugins have updates, for the palette's
/// **Plugins** entry. Kept by [`watch_updates`].
#[derive(Default)]
pub struct PluginUpdates(pub usize);

impl gpui::Global for PluginUpdates {}

/// What the update check does now.
#[derive(Debug, PartialEq)]
pub enum UpdateCheck {
    /// The setting is off: no updates are shown.
    Off,
    Nothing,
    /// Something was installed or updated: count again from the cached index.
    Recount,
    /// Look at the index (downloading it if the copy is a day old).
    Fetch,
}

/// The update check's next step: `last_fetch` is when it last looked at
/// the index, `installs_changed` whether `lumen plugins` changed anything.
pub fn update_check(
    on: bool,
    last_fetch: Option<std::time::Instant>,
    installs_changed: bool,
    now: std::time::Instant,
) -> UpdateCheck {
    const HOURLY: std::time::Duration = std::time::Duration::from_secs(3600);
    match last_fetch {
        _ if !on => UpdateCheck::Off,
        None => UpdateCheck::Fetch,
        Some(t) if now.duration_since(t) > HOURLY => UpdateCheck::Fetch,
        Some(_) if installs_changed => UpdateCheck::Recount,
        Some(_) => UpdateCheck::Nothing,
    }
}

/// When the update check runs: [`update_check`] with what it remembers.
pub struct UpdateTimer {
    last_fetch: Option<std::time::Instant>,
    last_files: Option<u64>,
    was_on: bool,
}

impl UpdateTimer {
    /// As Lumen opens: count from the cached index first, and look at the
    /// index itself an hour later, so starting never touches the network.
    pub fn new(now: std::time::Instant) -> UpdateTimer {
        UpdateTimer { last_fetch: Some(now), last_files: None, was_on: true }
    }

    /// The next step, given the setting and [`PluginFiles`]' count.
    pub fn step(&mut self, on: bool, files: u64, now: std::time::Instant) -> UpdateCheck {
        let changed = self.last_files.replace(files) != Some(files);
        let turned_on = on && !self.was_on;
        self.was_on = on;
        let step = update_check(on, if turned_on { None } else { self.last_fetch }, changed, now);
        if step == UpdateCheck::Fetch {
            self.last_fetch = Some(now);
        }
        step
    }
}

/// Keep [`PluginUpdates`] current, off the main thread: the index is
/// looked at hourly and downloaded at most once a day, and only while
/// registry plugins are installed and `plugin-update-check` is on.
pub fn watch_updates(cx: &mut gpui::App) {
    cx.spawn(async move |cx| {
        let mut timer = UpdateTimer::new(std::time::Instant::now());
        loop {
            let Ok((on, files)) = cx.update(|cx| {
                let on = crate::settings::Settings::get(cx).plugin_update_check;
                (on, cx.try_global::<PluginFiles>().map_or(0, |p| p.0))
            }) else {
                break;
            };
            let step = timer.step(on, files, std::time::Instant::now());
            let count = match step {
                UpdateCheck::Nothing => None,
                UpdateCheck::Off => Some(0),
                UpdateCheck::Recount | UpdateCheck::Fetch => {
                    let fetch = step == UpdateCheck::Fetch;
                    Some(cx.background_executor().spawn(async move { count_updates(fetch) }).await)
                }
            };
            if let Some(n) = count {
                let set = cx.update(|cx| {
                    if cx.try_global::<PluginUpdates>().map_or(0, |u| u.0) != n {
                        cx.set_global(PluginUpdates(n));
                    }
                });
                if set.is_err() {
                    break;
                }
            }
            cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
        }
    })
    .detach();
}

/// Updates for installed registry plugins, from the index downloaded
/// within a day (`fetch`) or whatever copy is cached. 0 with nothing installed.
fn count_updates(fetch: bool) -> usize {
    let installed = crate::registry::load_installed();
    if installed.is_empty() {
        return 0;
    }
    let index = if fetch {
        crate::registry::cache_dir().and_then(|d| {
            let day = std::time::Duration::from_secs(24 * 3600);
            crate::registry::index_within(&crate::registry::index_url(), &d.join("plugin-index.json"), day).ok()
        })
    } else {
        crate::registry::cached_index()
    };
    index.map_or(0, |i| crate::registry::count_updates(&installed, &i))
}

/// Where plugins are installed.
/// The built-in plugins last started, to notice when config.toml changes them
/// (whichever window sees the change first reloads them, once).
static BUILTINS_RUNNING: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn builtins_running() -> Vec<String> {
    BUILTINS_RUNNING.lock().unwrap().clone()
}

pub fn set_builtins_running(names: Vec<String>) {
    *BUILTINS_RUNNING.lock().unwrap() = names;
}

pub fn plugins_dir() -> Option<PathBuf> {
    crate::settings::config_path().and_then(|p| Some(p.parent()?.join("plugins")))
}

fn state_dir() -> Option<PathBuf> {
    // Tests that run a plugin leave your real logs and data alone.
    #[cfg(test)]
    return Some(std::env::temp_dir().join(format!("lumen-test-state-{}", std::process::id())));
    #[cfg(not(test))]
    real_state_dir()
}

#[cfg_attr(test, allow(dead_code))]
fn real_state_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home?.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".local/state")))?
    };
    Some(base.join("lumen"))
}

/// The `lumen_plugin` Python package, built into Lumen so plugins can import
/// it without installing anything, and always matching this Lumen.
const PYTHON_PACKAGE: &str = include_str!("../../python/lumen_plugin/__init__.py");

/// A folder holding an up-to-date `lumen_plugin`, for `PYTHONPATH`; written
/// under the state folder the first time it's asked for.
pub fn python_path() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = state_dir()?.join("python");
        let file = dir.join("lumen_plugin").join("__init__.py");
        if std::fs::read_to_string(&file).ok().as_deref() != Some(PYTHON_PACKAGE) {
            std::fs::create_dir_all(file.parent()?).ok()?;
            let tmp = file.with_extension(format!("py.{}.tmp", std::process::id()));
            std::fs::write(&tmp, PYTHON_PACKAGE).ok()?;
            std::fs::rename(&tmp, &file).ok()?;
        }
        Some(dir)
    })
    .clone()
}

/// `PYTHONPATH` for a plugin: Lumen's package first, then whatever was set.
fn plugin_python_path() -> Option<std::ffi::OsString> {
    let dir = python_path()?;
    let mut paths = vec![dir];
    if let Some(existing) = std::env::var_os("PYTHONPATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).ok()
}

pub fn log_path(name: &str) -> Option<PathBuf> {
    state_dir().map(|d| d.join("plugins").join(format!("{name}.log")))
}

/// Read `dir/plugin.toml`.
pub fn parse_manifest(dir: &Path, text: &str) -> Result<Manifest, String> {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
    let doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.message().trim().to_string())?;
    let string = |key: &str| doc.get(key).and_then(|i| i.as_str()).map(str::to_string);
    let command = match doc.get("command") {
        Some(item) if item.is_str() => item.as_str().unwrap().split_whitespace().map(String::from).collect(),
        Some(item) if item.is_array() => item
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().map(String::from).ok_or("`command` should be a list of strings"))
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("`command` should be a string or a list of strings".into()),
        None => return Err("`command` is missing (what to run, e.g. \"python3 main.py\")".into()),
    };
    if command.is_empty() {
        return Err("`command` is empty".into());
    }
    let mut permissions = Vec::new();
    if let Some(item) = doc.get("permissions") {
        let list = item.as_array().ok_or("`permissions` should be a list, e.g. [\"panes\", \"input\"]")?;
        for v in list {
            let n = v.as_str().unwrap_or_default();
            let p = Permission::parse(n).ok_or_else(|| {
                format!("unknown permission \"{n}\"; permissions are \"panes\", \"input\" and \"screen\"")
            })?;
            if !permissions.contains(&p) {
                permissions.push(p);
            }
        }
    }
    permissions.sort();
    Ok(Manifest {
        name,
        description: string("description").unwrap_or_default(),
        version: string("version").unwrap_or_default(),
        command,
        permissions,
        settings: crate::plugin_settings::parse_schema(&doc)?,
        dir: dir.to_path_buf(),
        builtin: false,
        registry: false,
    })
}

// ---- built-in plugins ----------------------------------------------------

/// A plugin that comes with Lumen, off until you turn it on in Plugins
/// (or list it under `plugins` in config.toml).
pub struct Builtin {
    pub name: &'static str,
    pub title: &'static str,
    files: &'static [(&'static str, &'static str)],
}

macro_rules! builtin {
    ($name:literal, $title:literal, [$($file:literal),*]) => {
        Builtin {
            name: $name,
            title: $title,
            files: &[$(($file, include_str!(concat!("../../plugins/", $name, "/", $file)))),*],
        }
    };
}

pub const BUILTINS: &[Builtin] = &[
    builtin!("recent-dirs", "Recent Directories", ["plugin.toml", "main.py"]),
    builtin!("command-history", "Command History", ["plugin.toml", "main.py"]),
    builtin!("git-branch", "Switch Git Branch", ["plugin.toml", "main.py"]),
    builtin!("ssh-hosts", "SSH to Host", ["plugin.toml", "main.py"]),
];

/// Plugins that used to come with Lumen and now come from the registry: a
/// config.toml naming the old one means the new one.
pub const MOVED: &[(&str, &str)] =
    &[("project-layout", "sn/project-layout"), ("build-status", "sn/build-status"), ("pomodoro", "sn/pomodoro")];

/// The registry plugin a once built-in plugin's name now means.
pub fn moved_to(name: &str) -> Option<&'static str> {
    MOVED.iter().find(|(old, _)| *old == name).map(|(_, new)| *new)
}

pub fn builtin(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

impl Builtin {
    /// Its manifest, with the files written out (or refreshed after a Lumen
    /// update) under the state folder, where it runs from.
    pub fn manifest(&self) -> Result<Manifest, String> {
        let dir = state_dir().ok_or("no state folder")?.join("builtin-plugins").join(self.name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for (file, text) in self.files {
            let path = dir.join(file);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(*text) {
                let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
                std::fs::write(&tmp, text)
                    .and_then(|_| std::fs::rename(&tmp, &path))
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        let toml = self.files.iter().find(|(f, _)| *f == "plugin.toml").map(|(_, t)| *t).unwrap_or_default();
        let mut m = parse_manifest(&dir, toml)?;
        m.builtin = true;
        Ok(m)
    }

    /// Its manifest as written, without writing its files anywhere: what it
    /// does, needs and can be set to, for showing.
    pub fn parsed(&self) -> Result<Manifest, String> {
        let toml = self.files.iter().find(|(f, _)| *f == "plugin.toml").map_or("", |(_, t)| *t);
        let mut m = parse_manifest(Path::new(self.name), toml)?;
        m.builtin = true;
        Ok(m)
    }

    /// Your own plugin folder with the same name, which runs instead.
    pub fn overridden(&self) -> bool {
        plugins_dir().is_some_and(|d| d.join(self.name).join("plugin.toml").is_file())
    }
}

/// Where a plugin keeps its own data: kept across updates, only yours.
fn data_dir(name: &str) -> Option<PathBuf> {
    data_dir_in(&state_dir()?, name)
}

/// [`data_dir`] under `state`. A plugin that moved to the registry takes
/// over its built-in days' folder, the first time.
fn data_dir_in(state: &Path, name: &str) -> Option<PathBuf> {
    let dir = state.join("plugin-data").join(name);
    if !dir.exists()
        && let Some((old, _)) = MOVED.iter().find(|(_, new)| *new == name)
    {
        let before = state.join("plugin-data").join(old);
        if before.is_dir() {
            std::fs::create_dir_all(dir.parent()?).ok()?;
            let _ = std::fs::rename(&before, &dir);
        }
    }
    std::fs::create_dir_all(&dir).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Some(dir)
}

/// Every plugin folder with a `plugin.toml`, then the built-in plugins in
/// `enabled` (unless a folder of yours has the same name), and problems
/// reading them.
pub fn discover(enabled: &[String]) -> (Vec<Manifest>, Vec<String>) {
    let (mut found, mut problems) = discover_folders();
    for name in enabled {
        let Some(b) = builtin(name) else { continue };
        if found.iter().any(|m: &Manifest| m.name == b.name) || b.overridden() {
            continue;
        }
        match b.manifest() {
            Ok(m) => found.push(m),
            Err(e) => problems.push(format!("built-in plugin `{name}`: {e}")),
        }
    }
    let (registry, more) = discover_registry(enabled, &crate::registry::load_installed());
    found.extend(registry);
    problems.extend(more);
    (found, problems)
}

/// The banner line for registry plugins config.toml names that aren't
/// installed (a new machine): nothing is downloaded until you ask.
pub fn missing_message(names: &[String]) -> Option<String> {
    match names.len() {
        0 => None,
        1 => Some("1 plugin in config.toml isn't installed: open Plugins to install it".into()),
        n => Some(format!("{n} plugins in config.toml aren't installed: open Plugins to install them")),
    }
}

/// The registry plugins in `enabled`, from where they're installed, a
/// problem for each that can't be read, and one for those not installed.
fn discover_registry(
    enabled: &[String],
    installed: &BTreeMap<String, crate::registry::Installed>,
) -> (Vec<Manifest>, Vec<String>) {
    let (mut found, mut problems, mut missing) = (Vec::new(), Vec::new(), Vec::new());
    for name in enabled.iter().filter(|n| crate::registry::valid_name(n)) {
        let Some(i) = installed.get(name) else {
            missing.push(name.clone());
            continue;
        };
        let read = std::fs::read_to_string(i.dir.join("plugin.toml"))
            .map_err(|e| e.to_string())
            .and_then(|text| parse_manifest(&i.dir, &text));
        match read {
            Ok(mut m) => {
                m.name = name.clone();
                m.registry = true;
                found.push(m);
            }
            Err(e) => problems.push(format!("plugin `{name}`: {e}")),
        }
    }
    problems.extend(missing_message(&missing));
    (found, problems)
}

/// Your own plugins (folders in the plugins folder), by name, with their
/// versions.
pub fn local_plugins() -> Vec<(String, String)> {
    discover_folders().0.into_iter().map(|m| (m.name, m.version)).collect()
}

/// Read and check `dir/plugin.toml`, for `lumen plugins check`.
pub fn check_folder(dir: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(dir.join("plugin.toml"))
        .map_err(|e| format!("{}: {e}", dir.join("plugin.toml").display()))?;
    parse_manifest(dir, &text)
}

fn discover_folders() -> (Vec<Manifest>, Vec<String>) {
    let (mut found, mut problems) = (Vec::new(), Vec::new());
    let Some(dir) = plugins_dir() else { return (found, problems) };
    let Ok(entries) = std::fs::read_dir(&dir) else { return (found, problems) };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for d in dirs {
        let file = d.join("plugin.toml");
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        match parse_manifest(&d, &text) {
            Ok(m) => found.push(m),
            Err(e) => problems.push(format!(
                "plugin `{}`: {e}",
                d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            )),
        }
    }
    (found, problems)
}

// ---- approvals ---------------------------------------------------------

/// What you allowed (or refused), kept outside the plugin's folder so a
/// plugin can't approve itself. A new permission or a different command
/// asks again.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
struct Approval {
    command: Vec<String>,
    permissions: Vec<Permission>,
    allowed: bool,
}

pub fn approvals_path() -> Option<PathBuf> {
    state_dir().map(|d| d.join("plugin-approvals.json"))
}

fn load_approvals() -> BTreeMap<String, Approval> {
    approvals_path().map(|p| load_approvals_from(&p)).unwrap_or_default()
}

fn load_approvals_from(path: &Path) -> BTreeMap<String, Approval> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Decision {
    Allowed,
    Refused,
    /// Never asked, or the manifest changed since.
    Ask,
}

pub fn decision(m: &Manifest) -> Decision {
    decide(m, &load_approvals())
}

/// [`decision`], with the approvals file at `path`.
pub fn decision_at(path: &Path, m: &Manifest) -> Decision {
    decide(m, &load_approvals_from(path))
}

fn decide(m: &Manifest, approvals: &BTreeMap<String, Approval>) -> Decision {
    if m.permissions.is_empty() {
        return Decision::Allowed;
    }
    if m.registry {
        // Approved when installed; Lumen never asks about these itself.
        let covered = approvals
            .get(&m.approval_key())
            .is_some_and(|a| a.allowed && m.permissions.iter().all(|p| a.permissions.contains(p)));
        return if covered { Decision::Allowed } else { Decision::Refused };
    }
    match approvals.get(&m.approval_key()) {
        Some(a) if a.command == m.command && m.permissions.iter().all(|p| a.permissions.contains(p)) => {
            if a.allowed {
                Decision::Allowed
            } else {
                Decision::Refused
            }
        }
        _ => Decision::Ask,
    }
}

/// What a plugin that isn't allowed to run says in the plugin list.
pub fn refused_status(m: &Manifest) -> String {
    if m.registry { format!("not approved: run `lumen plugins install {}`", m.name) } else { "not allowed".into() }
}

/// Approve a registry plugin's permissions (in the approvals file at
/// `path`), when it's installed or updated.
pub fn approve_registry_in(path: &Path, name: &str, permissions: &[Permission]) -> Result<(), String> {
    store_approval_at(
        path,
        format!("registry:{name}"),
        Approval { command: Vec::new(), permissions: permissions.to_vec(), allowed: true },
    )
}

/// The permissions a registry plugin has been allowed.
pub fn registry_approval_in(path: &Path, name: &str) -> Vec<Permission> {
    match load_approvals_from(path).remove(&format!("registry:{name}")) {
        Some(a) if a.allowed => a.permissions,
        _ => Vec::new(),
    }
}

/// Forget a registry plugin's approval, when it's removed: installing it
/// again asks again.
pub fn forget_registry_in(path: &Path, name: &str) -> Result<(), String> {
    let _lock = lock_approvals(path)?;
    let mut all = load_approvals_from(path);
    if all.remove(&format!("registry:{name}")).is_some() {
        save_approvals_to(path, &all)?;
    }
    Ok(())
}

fn save_approvals_to(path: &Path, all: &BTreeMap<String, Approval>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(all).map_err(|e| e.to_string())?;
    crate::registry::write_private(path, &text)
}

/// Keep your answer about `m`. An error means it wasn't kept, so Lumen will
/// ask again.
pub fn record_decision(m: &Manifest, allowed: bool) -> Result<(), String> {
    let path = approvals_path().ok_or("no home folder to keep the answer in")?;
    record_decision_at(&path, m, allowed)
}

/// [`record_decision`], in the approvals file at `path`.
pub fn record_decision_at(path: &Path, m: &Manifest, allowed: bool) -> Result<(), String> {
    store_approval_at(
        path,
        m.approval_key(),
        Approval { command: m.command.clone(), permissions: m.permissions.clone(), allowed },
    )
}

/// The approvals file's lock: Lumen and `lumen plugins` both change it.
/// Without it nothing is written: two writers would lose one's answers.
fn lock_approvals(path: &Path) -> Result<crate::registry::Lock, String> {
    crate::registry::Lock::take(&path.with_file_name("approvals.lock"))
}

fn store_approval_at(path: &Path, key: String, approval: Approval) -> Result<(), String> {
    let _lock = lock_approvals(path)?;
    let mut all = load_approvals_from(path);
    all.insert(key, approval);
    save_approvals_to(path, &all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed_at(dir: &Path, perms: Vec<Permission>) -> crate::registry::Installed {
        crate::registry::Installed {
            version: "1.0.0".into(),
            commit: "c".repeat(40),
            permissions: perms,
            dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn registry_plugins_are_discovered() {
        let dir = std::env::temp_dir().join(format!("lumen-reg-discover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            "version = \"1.0.0\"\ncommand = \"python3 main.py\"\npermissions = [\"panes\"]",
        )
        .unwrap();
        let mut installed = BTreeMap::new();
        installed.insert("mira/x".to_string(), installed_at(&dir, vec![Permission::Panes]));
        let (found, problems) = discover_registry(&["recent-dirs".into(), "mira/x".into()], &installed);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "mira/x");
        assert!(found[0].registry && !found[0].builtin);
        assert_eq!(found[0].approval_key(), "registry:mira/x");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_registry_plugins_are_reported() {
        let (found, problems) = discover_registry(&["mira/y".into()], &BTreeMap::new());
        assert!(found.is_empty());
        assert_eq!(problems, ["1 plugin in config.toml isn't installed: open Plugins to install it"]);
        let (_, problems) = discover_registry(&["mira/y".into(), "mira/z".into()], &BTreeMap::new());
        assert_eq!(problems, ["2 plugins in config.toml aren't installed: open Plugins to install them"]);
    }

    #[test]
    fn update_checks_are_daily_and_follow_installs() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let hour = Duration::from_secs(3600);
        assert_eq!(update_check(false, None, true, now), UpdateCheck::Off);
        assert_eq!(update_check(true, None, false, now), UpdateCheck::Fetch, "first check");
        assert_eq!(update_check(true, Some(now - hour / 2), false, now), UpdateCheck::Nothing);
        assert_eq!(update_check(true, Some(now - hour / 2), true, now), UpdateCheck::Recount, "an install: no network");
        assert_eq!(update_check(true, Some(now - hour - Duration::from_secs(1)), false, now), UpdateCheck::Fetch);
    }

    #[test]
    fn lumen_starting_counts_from_the_cache() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let mut t = UpdateTimer::new(start);
        assert_eq!(t.step(true, 0, start), UpdateCheck::Recount, "no network as Lumen opens");
        assert_eq!(t.step(true, 0, start + Duration::from_secs(60)), UpdateCheck::Nothing);
        assert_eq!(t.step(true, 1, start + Duration::from_secs(70)), UpdateCheck::Recount);
        assert_eq!(t.step(true, 1, start + Duration::from_secs(3700)), UpdateCheck::Fetch, "hourly");
        assert_eq!(t.step(false, 1, start + Duration::from_secs(3800)), UpdateCheck::Off);
        assert_eq!(t.step(true, 1, start + Duration::from_secs(3900)), UpdateCheck::Fetch, "turned on: at once");
    }

    #[test]
    fn missing_plugins_are_one_banner_line() {
        assert_eq!(missing_message(&[]), None);
        assert_eq!(
            missing_message(&["a/b".into()]).as_deref(),
            Some("1 plugin in config.toml isn't installed: open Plugins to install it")
        );
        assert_eq!(
            missing_message(&["a/b".into(), "c/d".into(), "e/f".into()]).as_deref(),
            Some("3 plugins in config.toml aren't installed: open Plugins to install them")
        );
    }

    #[test]
    fn registry_approval_is_permissions_only() {
        let dir = std::env::temp_dir();
        let manifest = |perms: &str, command: &str| {
            let mut m = parse_manifest(&dir, &format!("command = \"{command}\"\npermissions = [{perms}]")).unwrap();
            m.name = "mira/x".into();
            m.registry = true;
            m
        };
        let mut approvals = BTreeMap::new();
        approvals.insert(
            "registry:mira/x".to_string(),
            Approval { command: Vec::new(), permissions: vec![Permission::Panes], allowed: true },
        );
        assert_eq!(decide(&manifest("\"panes\"", "a"), &approvals), Decision::Allowed);
        assert_eq!(decide(&manifest("\"panes\"", "something else"), &approvals), Decision::Allowed);
        assert_eq!(decide(&manifest("", "a"), &approvals), Decision::Allowed);
        assert_eq!(decide(&manifest("\"panes\", \"input\"", "a"), &approvals), Decision::Refused);
        assert_eq!(decide(&manifest("\"panes\"", "a"), &BTreeMap::new()), Decision::Refused, "never Ask");
        assert!(refused_status(&manifest("\"panes\"", "a")).contains("lumen plugins install mira/x"));
    }

    #[test]
    fn manifest_reads_settings() {
        let m = parse_manifest(
            Path::new("p"),
            "command = \"x\"\n[settings.n]\ntype = \"integer\"\ndefault = 1\ndescription = \"d\"",
        )
        .unwrap();
        assert_eq!(m.settings.len(), 1);
        let e =
            parse_manifest(Path::new("p"), "command = \"x\"\n[settings.n]\ntype = \"integer\"\ndescription = \"d\"")
                .unwrap_err();
        assert!(e.contains("default"), "{e}");
    }

    #[test]
    fn a_moved_plugin_keeps_its_data() {
        let state = std::env::temp_dir().join(format!("lumen-moved-data-{}", std::process::id()));
        let old = state.join("plugin-data").join("pomodoro");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("stats.json"), "{}").unwrap();
        let dir = data_dir_in(&state, "sn/pomodoro").unwrap();
        assert!(dir.join("stats.json").is_file(), "{}", dir.display());
        assert!(!old.exists());
        // Once moved, an old folder made later is left alone.
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("stats.json"), "old").unwrap();
        let again = data_dir_in(&state, "sn/pomodoro").unwrap();
        assert_eq!(std::fs::read_to_string(again.join("stats.json")).unwrap(), "{}");
        // Others are untouched.
        assert!(data_dir_in(&state, "mira/x").unwrap().ends_with("plugin-data/mira/x"));
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn moved_names_point_at_the_registry() {
        assert_eq!(moved_to("project-layout"), Some("sn/project-layout"));
        assert_eq!(moved_to("recent-dirs"), None);
        for (old, new) in MOVED {
            assert!(builtin(old).is_none(), "{old} is still built in");
            assert!(crate::registry::valid_name(new), "{new}");
        }
    }

    #[test]
    fn builtin_plugins_have_valid_manifests() {
        for b in BUILTINS {
            let toml = b.files.iter().find(|(f, _)| *f == "plugin.toml").expect("plugin.toml").1;
            let mut m = parse_manifest(Path::new(b.name), toml).unwrap_or_else(|e| panic!("{}: {e}", b.name));
            assert!(!m.description.is_empty(), "{}", b.name);
            assert!(b.files.iter().any(|(f, _)| *f == m.command[1]), "{} runs a file it doesn't ship", b.name);
            assert!(b.files.iter().any(|(_, t)| t.contains("from lumen_plugin import")), "{}", b.name);
            let expected: &[(&str, i64, i64, i64)] = match b.name {
                "command-history" => &[("max-entries", 10_000, 100, 1_000_000)],
                "recent-dirs" => &[("max-shown", 200, 10, 1000)],
                _ => &[],
            };
            let got: Vec<(&str, i64, i64, i64)> = m
                .settings
                .iter()
                .map(|s| match s.kind {
                    crate::plugin_settings::Kind::Integer { min: Some(lo), max: Some(hi) } => {
                        (s.key.as_str(), s.default.as_i64().unwrap(), lo, hi)
                    }
                    ref k => panic!("{}: {} is {k:?}", b.name, s.key),
                })
                .collect();
            assert_eq!(got, expected, "{}'s settings", b.name);
            // Approvals for built-ins never apply to a folder of the same name.
            assert_eq!(m.approval_key(), b.name);
            m.builtin = true;
            assert_eq!(m.approval_key(), format!("builtin:{}", b.name));
        }
    }

    #[test]
    fn manifests() {
        let dir = Path::new("/x/git-branch");
        let m = parse_manifest(
            dir,
            "description = \"Switch branches\"\nversion = \"1.0\"\ncommand = \"python3 main.py\"\npermissions = [\"input\", \"panes\", \"input\"]\n",
        )
        .unwrap();
        assert_eq!(m.name, "git-branch");
        assert_eq!(m.command, vec!["python3", "main.py"]);
        assert_eq!(m.permissions, vec![Permission::Panes, Permission::Input]);
        let m = parse_manifest(dir, "command = [\"node\", \"my plugin.js\"]\n").unwrap();
        assert_eq!(m.command, vec!["node", "my plugin.js"]);
        assert!(m.permissions.is_empty());
        assert!(parse_manifest(dir, "version = \"1\"\n").unwrap_err().contains("`command` is missing"));
        assert!(
            parse_manifest(dir, "command = \"x\"\npermissions = [\"network\"]\n")
                .unwrap_err()
                .contains("unknown permission")
        );
        assert!(parse_manifest(dir, "command = \n").is_err());
    }

    #[test]
    fn plugins_need_permission_for_sensitive_methods() {
        let caller = Caller::Plugin { name: "p".into(), perms: vec![Permission::Panes] };
        assert!(caller.may(Permission::Panes));
        assert!(!caller.may(Permission::Input));
        assert!(Caller::Socket.may(Permission::Screen));
    }
}
