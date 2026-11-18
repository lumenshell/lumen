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
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use toml_edit::DocumentMut;

use crate::api::{self, ApiError, ApiResult, Caller};
use crate::keymap;

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
const PYTHON_PACKAGE: &str = include_str!("../python/lumen_plugin/__init__.py");

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
            files: &[$(($file, include_str!(concat!("../plugins/", $name, "/", $file)))),*],
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

// ---- running -----------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct PluginCommand {
    pub id: String,
    pub title: String,
    /// Normalized key, if the plugin asked for one.
    pub keys: Option<String>,
}

struct Running {
    manifest: Manifest,
    status: String,
    /// Messages to the plugin, while it's running.
    out: Option<mpsc::Sender<String>>,
    commands: Vec<PluginCommand>,
    stop: Arc<AtomicBool>,
    pid: Option<u32>,
    /// Its connection and process, while [`supervise`] is running it.
    live: Option<Live>,
    /// Which start this is, so a copy still shutting down after a reload
    /// can't touch its replacement.
    generation: u64,
    /// Its settings now, defaults filled in: given to every (re)start.
    settings: serde_json::Map<String, Value>,
}

/// A running plugin's connection and process, for stopping it.
#[derive(Clone)]
struct Live {
    /// Its connection's id, as [`api::disconnect`] takes it.
    conn: u64,
    /// Whether the process has been waited for. Until then its pid, and
    /// its process group's id, can't have gone to anything else; signals
    /// are sent with this held, so they can't either.
    reaped: Arc<Mutex<bool>>,
}

/// How long a stopped plugin has to exit before it's killed.
const STOP_GRACE: Duration = Duration::from_millis(if cfg!(test) { 300 } else { 2000 });

static PLUGINS: Mutex<Vec<Running>> = Mutex::new(Vec::new());
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// The entry for this particular start of a plugin.
fn with_run<R>(name: &str, generation: u64, f: impl FnOnce(&mut Running) -> R) -> Option<R> {
    PLUGINS.lock().unwrap().iter_mut().find(|r| r.manifest.name == name && r.generation == generation).map(f)
}

/// A plugin as shown in the palette.
pub struct Info {
    pub manifest: Manifest,
    pub status: String,
    pub commands: Vec<PluginCommand>,
    /// Started (running, or restarting after a crash), rather than noted as
    /// refused or waiting for approval.
    pub started: bool,
}

pub fn list() -> Vec<Info> {
    PLUGINS
        .lock()
        .unwrap()
        .iter()
        .map(|r| Info {
            manifest: r.manifest.clone(),
            status: r.status.clone(),
            commands: r.commands.clone(),
            started: r.generation != 0,
        })
        .collect()
}

fn with_plugin<R>(name: &str, f: impl FnOnce(&mut Running) -> R) -> Option<R> {
    PLUGINS.lock().unwrap().iter_mut().find(|r| r.manifest.name == name).map(f)
}

/// Note a plugin that isn't running (refused, or waiting for approval), so
/// it still shows in the palette.
pub fn add_stopped(m: Manifest, status: &str, settings: serde_json::Map<String, Value>) {
    let mut all = PLUGINS.lock().unwrap();
    all.iter().filter(|r| r.manifest.name == m.name).for_each(end);
    all.retain(|r| r.manifest.name != m.name);
    all.push(Running {
        manifest: m,
        status: status.into(),
        out: None,
        commands: Vec::new(),
        stop: Arc::new(AtomicBool::new(true)),
        pid: None,
        live: None,
        generation: 0,
        settings,
    });
}

/// Append a line to the plugin's log, starting afresh past 1 MB.
fn log(name: &str, line: &str) {
    let Some(path) = log_path(name) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

/// Local time for log lines, from the C library (no date crate needed).
fn now() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let mut buf = [0u8; 32];
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), c"%Y-%m-%d %H:%M:%S".as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

/// Start `m` and keep it running: restart it when it exits, waiting longer
/// after each quick crash, until [`stop_all`].
pub fn start(m: Manifest, settings: serde_json::Map<String, Value>) {
    let Some(jobs) = api::jobs() else { return };
    let stop = Arc::new(AtomicBool::new(false));
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst);
    {
        let mut all = PLUGINS.lock().unwrap();
        // A new version, or new permissions: the old process goes.
        all.iter().filter(|r| r.manifest.name == m.name).for_each(end);
        all.retain(|r| r.manifest.name != m.name);
        all.push(Running {
            manifest: m.clone(),
            status: "starting".into(),
            out: None,
            commands: Vec::new(),
            stop: stop.clone(),
            pid: None,
            live: None,
            generation,
            settings,
        });
    }
    let name = m.name.clone();
    let _ =
        std::thread::Builder::new().name(format!("plugin-{name}")).spawn(move || supervise(m, stop, jobs, generation));
}

const BACKOFF: [u64; 6] = [1, 2, 5, 10, 30, 60];

fn supervise(m: Manifest, stop: Arc<AtomicBool>, jobs: api::Jobs, generation: u64) {
    let name = m.name.clone();
    let caller = Caller::Plugin { name: name.clone(), perms: m.permissions.clone() };
    let mut quick_failures = 0usize;
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        let started_with = current_settings(&name).unwrap_or_default();
        let mut cmd = Command::new(&m.command[0]);
        if let Some(path) = plugin_python_path() {
            cmd.env("PYTHONPATH", path);
        }
        let mut child = match cmd
            .args(&m.command[1..])
            .current_dir(&m.dir)
            .env_remove("LUMEN_SOCKET")
            .env_remove("LUMEN_PANE")
            .env("LUMEN_PLUGIN", &name)
            .env("LUMEN_PLUGIN_DIR", &m.dir)
            .envs(data_dir(&name).map(|d| ("LUMEN_PLUGIN_DATA", d)))
            // Read now, not at the first start: they may have changed since.
            .env("LUMEN_PLUGIN_SETTINGS", Value::Object(started_with.clone()).to_string())
            // A process group of its own, so stopping it reaches whatever it started.
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("couldn't start `{}`: {e}", m.command.join(" "));
                log(&name, &format!("{} [lumen] {msg}", now()));
                with_run(&name, generation, |r| r.status = format!("failed: {msg}"));
                // Retrying won't help until the manifest or PATH changes.
                return;
            }
        };
        let pid = child.id();
        // Stopping the plugin closes its stdin and ends its replies and events.
        let out = api::writer_until(child.stdin.take().expect("piped"), Some(stop.clone()));
        let live = Live { conn: api::new_conn(), reaped: Arc::new(Mutex::new(false)) };
        attach(&name, generation, out.clone(), pid, live.clone(), &started_with);
        log(&name, &format!("{} [lumen] started `{}` (pid {pid})", now(), m.command.join(" ")));
        if stop.load(Ordering::SeqCst) {
            // Told to stop while starting: don't leave it behind.
            let _ = child.kill();
        }

        let stderr = child.stderr.take().expect("piped");
        let err_name = name.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                log(&err_name, &format!("{} {line}", now()));
            }
        });
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        // Nothing it asks for after it's told to stop is carried out.
        api::serve_conn(stdout, live.conn, &out, &jobs, &caller, Some(&stop));
        drop(out);

        wait_unreaped(pid);
        let status = {
            let mut reaped = live.reaped.lock().unwrap();
            if stop.load(Ordering::SeqCst) {
                // Whatever it started and left behind goes with it.
                signal(pid, libc::SIGKILL);
            }
            let status = child.wait().map(|s| s.to_string()).unwrap_or_else(|e| e.to_string());
            *reaped = true;
            status
        };
        // Its commands and key bindings go with it.
        let had_commands = with_run(&name, generation, |r| {
            r.out = None;
            r.pid = None;
            r.live = None;
            !std::mem::take(&mut r.commands).is_empty()
        })
        .unwrap_or(false);
        if had_commands {
            let (reply, _) = futures::channel::oneshot::channel();
            let _ = jobs.unbounded_send(api::Job {
                caller: caller.clone(),
                method: "_plugin-commands-changed".into(),
                params: json!({}),
                reply,
                origin: None,
            });
        }
        if stop.load(Ordering::SeqCst) {
            log(&name, &format!("{} [lumen] stopped", now()));
            break;
        }
        quick_failures = if started.elapsed() > Duration::from_secs(60) { 0 } else { quick_failures + 1 };
        let wait = BACKOFF[quick_failures.clamp(1, BACKOFF.len()) - 1];
        log(&name, &format!("{} [lumen] exited ({status}); restarting in {wait}s", now()));
        with_run(&name, generation, |r| r.status = format!("exited ({status}), restarting in {wait}s"));
        let until = Instant::now() + Duration::from_secs(wait);
        while Instant::now() < until && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    with_run(&name, generation, |r| r.status = "stopped".into());
}

/// Send `sig` to a plugin's process group (it leads one of its own), or to
/// the process alone if it has none.
fn signal(pid: u32, sig: i32) {
    let Ok(pid) = i32::try_from(pid) else { return };
    // 0 and 1 would mean our own group, and every process there is.
    if pid <= 1 {
        return;
    }
    unsafe {
        if libc::kill(-pid, sig) != 0 {
            libc::kill(pid, sig);
        }
    }
}

/// Block until our child `pid` has exited, without reaping it: until it's
/// reaped, the pid and its process group stay its own.
fn wait_unreaped(pid: u32) {
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        while libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) != 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {}
    }
}

/// Stop a plugin's process and don't restart it. At once, it loses its
/// connection: nothing more it asks for is carried out, it's sent no more
/// events, and its panes and styles go. It and everything it started are
/// asked to exit (SIGTERM), and killed if still there after [`STOP_GRACE`].
fn end(r: &Running) {
    r.stop.store(true, Ordering::SeqCst);
    let Some(pid) = r.pid else { return };
    let Some(live) = r.live.clone() else {
        // Not one [`supervise`] started.
        signal(pid, libc::SIGTERM);
        return;
    };
    if let Some(jobs) = api::jobs() {
        let caller = Caller::Plugin { name: r.manifest.name.clone(), perms: r.manifest.permissions.clone() };
        api::disconnect(&jobs, &caller, live.conn);
    }
    if !*live.reaped.lock().unwrap() {
        signal(pid, libc::SIGTERM);
    }
    let _ = std::thread::Builder::new().name("plugin-stop".into()).spawn(move || {
        std::thread::sleep(STOP_GRACE);
        let reaped = live.reaped.lock().unwrap();
        if !*reaped {
            signal(pid, libc::SIGKILL);
        }
    });
}

/// Stop every plugin, as [`stop`] does, without forgetting them.
pub fn stop_all() {
    PLUGINS.lock().unwrap().iter().for_each(end);
}

/// Stop one plugin and forget it: it loses its connection, is asked to
/// exit, is killed if it doesn't, and isn't restarted.
pub fn stop(name: &str) {
    let mut all = PLUGINS.lock().unwrap();
    all.iter().filter(|r| r.manifest.name == name).for_each(end);
    all.retain(|r| r.manifest.name != name);
}

/// Which plugins to (re)start, stop and leave alone, by name.
#[derive(Debug, PartialEq, Default)]
pub struct Reload {
    pub start: Vec<String>,
    pub stop: Vec<String>,
    pub keep: Vec<String>,
}

/// Compare what's loaded with what should be. A plugin whose manifest
/// (folder, command, permissions, version, settings) or decision changed is
/// started again; one no longer wanted is stopped; the rest keep running,
/// so turning one plugin on doesn't restart the others.
pub fn plan_reload(running: &[(Manifest, Decision)], wanted: &[(Manifest, Decision)]) -> Reload {
    let mut r = Reload::default();
    for (m, d) in wanted {
        let same = running.iter().any(|(rm, rd)| rm == m && rd == d);
        if same { &mut r.keep } else { &mut r.start }.push(m.name.clone());
    }
    for (m, _) in running {
        if !wanted.iter().any(|(w, _)| w.name == m.name) {
            r.stop.push(m.name.clone());
        }
    }
    r
}

/// Forget every plugin (after [`stop_all`]), before loading them again.
pub fn clear() {
    PLUGINS.lock().unwrap().clear();
}

// ---- settings ------------------------------------------------------------

/// The plugin's process is up and can be written to. Settings saved since
/// it was given `started_with` reached nobody, so they're sent now.
fn attach(
    name: &str,
    generation: u64,
    out: mpsc::Sender<String>,
    pid: u32,
    live: Live,
    started_with: &serde_json::Map<String, Value>,
) {
    with_run(name, generation, |r| {
        if &r.settings != started_with {
            let _ = out.send(settings_message(&r.settings));
        }
        r.out = Some(out);
        r.pid = Some(pid);
        r.live = Some(live);
        r.status = "running".into();
    });
}

/// A plugin's settings now, if Lumen knows the plugin.
fn current_settings(name: &str) -> Option<serde_json::Map<String, Value>> {
    with_plugin(name, |r| r.settings.clone())
}

/// The `settings-changed` notification for a plugin.
fn settings_message(settings: &serde_json::Map<String, Value>) -> String {
    json!({"jsonrpc": "2.0", "method": "event", "params": {"event": "settings-changed", "settings": settings}})
        .to_string()
}

/// Give a plugin new settings. When they differ from what it has, they're
/// kept for its next start and, if it's running, sent to it now: it isn't
/// restarted. Returns whether they differed, so the same change seen by
/// several windows reaches the plugin once.
pub fn update_settings(name: &str, settings: serde_json::Map<String, Value>) -> bool {
    with_plugin(name, |r| {
        if r.settings == settings {
            return false;
        }
        if let Some(out) = &r.out {
            let _ = out.send(settings_message(&settings));
        }
        r.settings = settings;
        true
    })
    .unwrap_or(false)
}

// ---- commands ------------------------------------------------------------

/// `register-command` from a plugin: add (or replace) a palette command,
/// optionally with a key.
pub fn register_command(caller: &Caller, params: &Value) -> ApiResult {
    let Caller::Plugin { name, .. } = caller else {
        return Err(ApiError::failed("only plugins can register commands"));
    };
    let field = |k: &str| params.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let id = field("id").ok_or_else(|| ApiError::invalid_params("`id` is required"))?.to_string();
    let title = field("title").ok_or_else(|| ApiError::invalid_params("`title` is required"))?.to_string();
    let keys = match field("keys") {
        Some(k) => Some(keymap::normalize_keys(k).map_err(ApiError::invalid_params)?),
        None => None,
    };
    let cmd = PluginCommand { id: id.clone(), title, keys };
    with_plugin(name, |r| {
        r.commands.retain(|c| c.id != id);
        r.commands.push(cmd);
    });
    Ok(json!({}))
}

/// Key bindings plugins asked for: (keys, plugin, command id).
pub fn key_bindings() -> Vec<(String, String, String)> {
    PLUGINS
        .lock()
        .unwrap()
        .iter()
        .flat_map(|r| r.commands.iter().filter_map(|c| Some((c.keys.clone()?, r.manifest.name.clone(), c.id.clone()))))
        .collect()
}

/// Tell a plugin one of its commands was chosen.
pub fn run_command(plugin: &str, id: &str, pane: Option<u64>) -> Result<(), String> {
    let out = with_plugin(plugin, |r| r.out.clone()).flatten();
    let out = out.ok_or_else(|| format!("plugin \"{plugin}\" isn't running"))?;
    let msg = json!({"jsonrpc": "2.0", "method": "command", "params": {"id": id, "pane": pane}});
    out.send(msg.to_string()).map_err(|_| format!("plugin \"{plugin}\" isn't running"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(name: &str, settings: serde_json::Map<String, Value>) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel();
        let m = parse_manifest(Path::new(name), "command = \"x\"").unwrap();
        PLUGINS.lock().unwrap().push(Running {
            manifest: m,
            status: "running".into(),
            out: Some(tx),
            commands: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
            pid: None,
            live: None,
            generation: 0,
            settings,
        });
        rx
    }

    fn map(v: Value) -> serde_json::Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn changed_settings_are_sent() {
        let rx = running("t-changed", map(json!({"n": 1})));
        assert!(update_settings("t-changed", map(json!({"n": 2}))));
        let msg: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(
            msg,
            json!({"jsonrpc": "2.0", "method": "event", "params": {"event": "settings-changed", "settings": {"n": 2}}})
        );
    }

    #[test]
    fn same_settings_send_nothing() {
        let rx = running("t-same", map(json!({"n": 1})));
        assert!(!update_settings("t-same", map(json!({"n": 1}))));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn restart_uses_current_settings() {
        let _rx = running("t-restart", map(json!({"n": 1})));
        update_settings("t-restart", map(json!({"n": 3})));
        assert_eq!(current_settings("t-restart"), Some(map(json!({"n": 3}))));
    }

    #[test]
    fn settings_saved_while_starting_are_sent() {
        // Saved after the process got its environment, before its stdin was ready.
        let (tx, rx) = mpsc::channel();
        let m = parse_manifest(Path::new("t-starting"), "command = \"x\"").unwrap();
        PLUGINS.lock().unwrap().push(Running {
            manifest: m,
            status: "starting".into(),
            out: None,
            commands: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
            pid: None,
            live: None,
            generation: 7,
            settings: map(json!({"n": 1})),
        });
        assert!(update_settings("t-starting", map(json!({"n": 2}))));
        let live = Live { conn: 0, reaped: Arc::new(Mutex::new(true)) };
        attach("t-starting", 7, tx.clone(), 1, live.clone(), &map(json!({"n": 1})));
        let msg: Value = serde_json::from_str(&rx.try_recv().expect("the newer values are sent")).unwrap();
        assert_eq!(msg["params"]["settings"], json!({"n": 2}));
        attach("t-starting", 7, tx, 1, live, &map(json!({"n": 2})));
        assert!(rx.try_recv().is_err(), "nothing when it started with them");
    }

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

    fn plugin(name: &str, dir: &str) -> Manifest {
        let mut m = parse_manifest(Path::new(dir), "command = \"x\"").unwrap();
        m.name = name.into();
        m
    }

    #[test]
    fn unchanged_plugins_keep_running() {
        let list = vec![(plugin("a", "/p/a"), Decision::Allowed), (plugin("b", "/p/b"), Decision::Refused)];
        let r = plan_reload(&list, &list);
        assert_eq!(r, Reload { start: vec![], stop: vec![], keep: vec!["a".into(), "b".into()] });
    }

    #[test]
    fn changed_plugins_restart() {
        let running = vec![
            (plugin("mira/x", "/i/mira/x/1.0.0"), Decision::Allowed),
            (plugin("gone", "/p/gone"), Decision::Allowed),
            (plugin("approved", "/p/approved"), Decision::Refused),
            (plugin("same", "/p/same"), Decision::Allowed),
        ];
        let wanted = vec![
            (plugin("mira/x", "/i/mira/x/1.1.0"), Decision::Allowed),
            (plugin("approved", "/p/approved"), Decision::Allowed),
            (plugin("same", "/p/same"), Decision::Allowed),
            (plugin("new", "/p/new"), Decision::Allowed),
        ];
        let r = plan_reload(&running, &wanted);
        assert_eq!(r.start, ["mira/x", "approved", "new"]);
        assert_eq!(r.stop, ["gone"]);
        assert_eq!(r.keep, ["same"]);
    }

    /// Whether process `pid` is still there (a zombie doesn't count).
    fn alive(pid: i32) -> bool {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        match stat.rsplit_once(") ") {
            Some((_, rest)) => !rest.starts_with('Z'),
            None => unsafe { libc::kill(pid, 0) == 0 },
        }
    }

    #[test]
    fn a_plugin_that_ignores_being_stopped_is_cut_off_and_killed() {
        use futures::StreamExt;
        let dir = std::env::temp_dir().join(format!("lumen-stubborn-{}", std::process::id())).join("t-stubborn");
        std::fs::create_dir_all(&dir).unwrap();
        let pids = dir.join("pids");
        // Ignores SIGTERM, as does the `sleep` it starts. It asks for one
        // thing while running and another once it's been told to stop
        // (its stdin closes), then waits for ever.
        let script = format!(
            "trap '' TERM; sleep 60 & echo \"$$ $!\" > '{}'; \
             echo '{{\"jsonrpc\":\"2.0\",\"method\":\"before\",\"id\":1}}'; \
             while read -r line; do :; done; \
             echo '{{\"jsonrpc\":\"2.0\",\"method\":\"after\",\"id\":2}}'; \
             wait",
            pids.display()
        );
        let toml = format!("command = [\"sh\", \"-c\", {}]", serde_json::to_string(&script).unwrap());
        let m = parse_manifest(&dir, &toml).unwrap();

        // The UI's side: note what's asked for, and answer it.
        let (jobs, mut asked) = futures::channel::mpsc::unbounded::<api::Job>();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let noted = seen.clone();
        std::thread::spawn(move || {
            while let Some(job) = futures::executor::block_on(asked.next()) {
                noted.lock().unwrap().push(job.method.clone());
                let _ = job.reply.send(Ok(json!({})));
            }
        });
        let has = |method: &str| seen.lock().unwrap().iter().any(|m| m == method);
        let until = |what: &str, done: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !done() {
                assert!(Instant::now() < deadline, "{what}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };

        let stop = Arc::new(AtomicBool::new(false));
        PLUGINS.lock().unwrap().push(Running {
            manifest: m.clone(),
            status: "starting".into(),
            out: None,
            commands: Vec::new(),
            stop: stop.clone(),
            pid: None,
            live: None,
            generation: 4242,
            settings: serde_json::Map::new(),
        });
        let supervisor = std::thread::spawn(move || supervise(m, stop, jobs, 4242));
        until("its first request arrives", &|| has("before"));
        let both: Vec<i32> =
            std::fs::read_to_string(&pids).unwrap().split_whitespace().map(|p| p.parse().unwrap()).collect();
        let (plugin, child) = (both[0], both[1]);
        assert!(alive(plugin) && alive(child));

        let stopped = Instant::now();
        super::stop("t-stubborn");
        // Its connection goes at once, well before it's killed.
        until("it's disconnected", &|| has("_disconnected"));
        assert!(stopped.elapsed() < STOP_GRACE, "disconnected only once it was killed");
        assert!(alive(plugin), "it ignores SIGTERM, so it's still there");
        // Then it and what it started are killed.
        until("it and its child are killed", &|| !alive(plugin) && !alive(child));
        assert!(stopped.elapsed() >= STOP_GRACE, "killed before its time was up");
        supervisor.join().unwrap();
        assert!(!has("after"), "a request sent after it was stopped was carried out: {:?}", seen.lock().unwrap());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn replacing_a_plugin_stops_its_process() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let m = parse_manifest(Path::new("t-replace"), "command = \"x\"").unwrap();
        PLUGINS.lock().unwrap().push(Running {
            manifest: m.clone(),
            status: "running".into(),
            out: None,
            commands: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
            pid: Some(child.id()),
            live: None,
            generation: 9,
            settings: serde_json::Map::new(),
        });
        add_stopped(m, "not allowed", serde_json::Map::new());
        let deadline = Instant::now() + Duration::from_secs(2);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "the old process is still running");
            std::thread::sleep(Duration::from_millis(20));
        }
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

    #[test]
    fn registering_commands_checks_fields() {
        let caller = Caller::Plugin { name: "nobody-running".into(), perms: vec![] };
        assert!(register_command(&caller, &json!({"title": "x"})).is_err());
        assert!(register_command(&caller, &json!({"id": "a", "title": "A", "keys": "ctrl-shfit-x"})).is_err());
        assert!(register_command(&caller, &json!({"id": "a", "title": "A", "keys": "ctrl-alt-g"})).is_ok());
        assert!(register_command(&Caller::Socket, &json!({"id": "a", "title": "A"})).is_err());
    }
}
