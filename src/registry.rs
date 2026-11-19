//! Plugins from the registry (`lumenshell/plugins`): their names, where
//! they're installed, the record of what's installed, the index, and
//! installing, updating and removing them. Nothing here needs GPUI, so the
//! `lumen plugins` command uses it directly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::plugins::Permission;

/// Whether `name` is a registry name, `owner/name`: each part lower-case
/// letters, digits and `-`, 1 to 39 characters, not starting or ending with
/// `-` (GitHub's rules for account names).
pub fn valid_name(name: &str) -> bool {
    let part = |p: &str| {
        (1..=39).contains(&p.len())
            && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !p.starts_with('-')
            && !p.ends_with('-')
    };
    matches!(name.split_once('/'), Some((owner, rest)) if part(owner) && part(rest))
}

/// Whether `v` is a version as the registry accepts them: three dot-separated
/// numbers, like `1.2.3`. Versions become folder names, so nothing else gets in.
fn valid_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// `$VAR`, or `~/fallback` on Linux; `~/mac` on macOS.
fn base(var: &str, fallback: &str, mac: &str) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return Some(home()?.join(mac));
    }
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| Some(home()?.join(fallback)))
}

/// Where registry plugins are installed: `<owner>/<name>/<version>/` inside.
pub fn install_root() -> Option<PathBuf> {
    Some(base("XDG_DATA_HOME", ".local/share", "Library/Application Support")?.join("lumen/installed-plugins"))
}

/// Lumen's cache folder (the plugin index lives here).
pub fn cache_dir() -> Option<PathBuf> {
    Some(base("XDG_CACHE_HOME", ".cache", "Library/Caches")?.join("lumen"))
}

/// The record of what's installed.
pub fn installed_path() -> Option<PathBuf> {
    Some(base("XDG_STATE_HOME", ".local/state", "Library/Application Support")?.join("lumen/installed.json"))
}

/// One installed registry plugin.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Installed {
    pub version: String,
    pub commit: String,
    pub permissions: Vec<Permission>,
    /// The folder holding its `plugin.toml`.
    pub dir: PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct InstalledFile {
    plugins: BTreeMap<String, Installed>,
}

/// Every installed registry plugin, by name. A missing or unreadable record
/// means nothing is installed.
pub fn load_installed() -> BTreeMap<String, Installed> {
    installed_path().map(|p| load_installed_from(&p)).unwrap_or_default()
}

pub(crate) fn load_installed_from(path: &Path) -> BTreeMap<String, Installed> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<InstalledFile>(&t).ok())
        .map(|f| f.plugins)
        .unwrap_or_default()
}

fn save_installed_to(path: &Path, all: &BTreeMap<String, Installed>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&InstalledFile { plugins: all.clone() }).map_err(|e| e.to_string())?;
    write_private(path, &text)
}

/// Write `text` to `path` readable only by you, all at once: a reader sees
/// the old file or the new one, never half of one.
pub fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp{}-{n}", std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

// ---- the index ------------------------------------------------------------

/// Where the registry's index is published.
pub const INDEX_URL: &str = "https://lumenshell.github.io/plugins/index.json";

/// How long a downloaded index is used before fetching it again.
const INDEX_FRESH: std::time::Duration = std::time::Duration::from_secs(3600);

/// The registry's list of plugins.
#[derive(Deserialize, Clone, Debug)]
pub struct Index {
    /// Checked by `parse_index` before the rest is read.
    #[allow(dead_code)]
    pub version: u32,
    pub plugins: Vec<Entry>,
}

// `repo`, `license` and `updated` are for the plugin manager's details view.
#[allow(dead_code)]
#[derive(Deserialize, Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub repo: String,
    /// The folder in the repository holding `plugin.toml`; "" is the root.
    #[serde(default)]
    pub path: String,
    pub description: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub stars: u64,
    /// When its newest version was published (YYYY-MM-DD).
    #[serde(default)]
    pub updated: String,
    /// Oldest first.
    pub versions: Vec<Release>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Release {
    pub version: String,
    pub commit: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    /// The oldest Lumen it works with.
    pub lumen: String,
    /// A `.tar.gz` of the repository at `commit`, with one top-level folder.
    pub archive: String,
}

/// Whether version `a` is newer than `b`: dotted numbers compared in turn,
/// missing parts counting as 0, anything after a `-` ignored.
pub fn newer(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        let v = v.split('-').next().unwrap_or_default();
        let mut p: Vec<u64> = v.split('.').map(|n| n.trim().parse().unwrap_or(0)).collect();
        while p.last() == Some(&0) {
            p.pop();
        }
        p
    }
    parts(a) > parts(b)
}

/// The newest version that works with Lumen `lumen`.
pub fn best<'a>(entry: &'a Entry, lumen: &str) -> Option<&'a Release> {
    entry.versions.iter().filter(|r| !newer(&r.lumen, lumen)).fold(None, |found: Option<&Release>, r| match found {
        Some(f) if !newer(&r.version, &f.version) => Some(f),
        _ => Some(r),
    })
}

/// Plugins whose name, description or a category contains `text` (any case;
/// "" is all), most stars first.
pub fn search<'a>(index: &'a Index, text: &str) -> Vec<&'a Entry> {
    let text = text.trim().to_lowercase();
    let mut found: Vec<&Entry> = index
        .plugins
        .iter()
        .filter(|e| {
            text.is_empty()
                || e.name.to_lowercase().contains(&text)
                || e.description.to_lowercase().contains(&text)
                || e.categories.iter().any(|c| c.to_lowercase().contains(&text))
        })
        .collect();
    found.sort_by(|a, b| b.stars.cmp(&a.stars).then_with(|| a.name.cmp(&b.name)));
    found
}

/// How many of `installed` have a newer version this Lumen can run.
pub fn count_updates(installed: &BTreeMap<String, Installed>, index: &Index) -> usize {
    installed
        .iter()
        .filter(|(name, i)| {
            index
                .plugins
                .iter()
                .find(|e| &e.name == *name)
                .and_then(|e| best(e, LUMEN_VERSION))
                .is_some_and(|r| newer(&r.version, &i.version))
        })
        .count()
}

/// Download `url` to `dest` with curl. An absolute path is copied instead
/// (for testing an index or archive before it's published).
pub fn fetch(url: &str, dest: &Path) -> Result<(), String> {
    fetch_within(url, dest, FETCH_LIMIT)
}

/// The most [`fetch`] downloads: room for the index and any archive [`LIMITS`] allows.
const FETCH_LIMIT: u64 = 64 * MB;

/// [`fetch`], refusing more than `limit` bytes: by its announced size, while
/// it arrives (curl 8.4 and later), and once it's here.
fn fetch_within(url: &str, dest: &Path, limit: u64) -> Result<(), String> {
    let too_big = || format!("{url} is over {}; not downloaded", size(limit));
    if url.starts_with('/') {
        if std::fs::metadata(url).map_err(|e| format!("{url}: {e}"))?.len() > limit {
            return Err(too_big());
        }
        return std::fs::copy(url, dest).map(|_| ()).map_err(|e| format!("{url}: {e}"));
    }
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--proto", "=https,file", "--max-time", "120", "--max-filesize"])
        .arg(limit.to_string())
        .arg("-o")
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "Lumen needs `curl` to download plugins; install it and try again".into(),
            _ => format!("couldn't run curl: {e}"),
        })?;
    // 63 is curl's "file size exceeded".
    let arrived = std::fs::metadata(dest).map_or(0, |m| m.len());
    if out.status.code() == Some(63) || arrived > limit {
        let _ = std::fs::remove_file(dest);
        return Err(too_big());
    }
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("couldn't download {url}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// The registry's index: the copy downloaded within the last hour unless
/// `refresh`, else a fresh one. `LUMEN_PLUGIN_INDEX` (a URL or a file path)
/// replaces the published one.
pub fn index(refresh: bool) -> Result<Index, String> {
    let cache = cache_dir().ok_or("no home folder")?.join("plugin-index.json");
    index_from(&index_url(), &cache, refresh)
}

/// Where the index comes from: `LUMEN_PLUGIN_INDEX`, else the published one.
pub(crate) fn index_url() -> String {
    std::env::var("LUMEN_PLUGIN_INDEX").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| INDEX_URL.into())
}

/// The index kept from the last download, however old, without the network
/// (for `lumen plugins list`).
pub fn cached_index() -> Option<Index> {
    let text = std::fs::read_to_string(cache_dir()?.join("plugin-index.json")).ok()?;
    parse_index(&text).ok()
}

pub(crate) fn parse_index(text: &str) -> Result<Index, String> {
    let version = serde_json::from_str::<serde_json::Value>(text)
        .map_err(|e| format!("the plugin index isn't valid JSON: {e}"))?
        .get("version")
        .and_then(serde_json::Value::as_u64);
    if version != Some(1) {
        return Err("this Lumen is too old for the plugin index; update Lumen".into());
    }
    serde_json::from_str(text).map_err(|e| format!("the plugin index isn't valid: {e}"))
}

pub(crate) fn index_from(url: &str, cache: &Path, refresh: bool) -> Result<Index, String> {
    index_within(url, cache, if refresh { std::time::Duration::ZERO } else { INDEX_FRESH })
}

/// The index: the cached copy if it's younger than `max_age`, else fetched.
pub(crate) fn index_within(url: &str, cache: &Path, max_age: std::time::Duration) -> Result<Index, String> {
    let fresh =
        std::fs::metadata(cache).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age < max_age));
    if fresh && let Some(index) = std::fs::read_to_string(cache).ok().and_then(|t| parse_index(&t).ok()) {
        return Ok(index);
    }
    let dir = cache.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = cache.with_extension(format!("download{}", std::process::id()));
    let got = fetch(url, &tmp).and_then(|()| {
        let text = std::fs::read_to_string(&tmp).map_err(|e| e.to_string())?;
        parse_index(&text)
    });
    match got {
        Ok(index) => {
            // Only a good index replaces the one kept.
            std::fs::rename(&tmp, cache).map_err(|e| format!("{}: {e}", cache.display()))?;
            Ok(index)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

// ---- installing -----------------------------------------------------------

/// Every place installing touches. [`Paths::real`] in use; a temporary
/// folder in tests.
pub struct Paths {
    /// `<owner>/<name>/<version>/` go in here.
    pub root: PathBuf,
    pub installed: PathBuf,
    pub approvals: PathBuf,
    pub config: PathBuf,
    /// Plugins' own data: `<owner>/<name>/` in here.
    pub data: PathBuf,
}

impl Paths {
    pub fn real() -> Result<Paths, String> {
        let none = || "no home folder".to_string();
        Ok(Paths {
            root: install_root().ok_or_else(none)?,
            installed: installed_path().ok_or_else(none)?,
            approvals: crate::plugins::approvals_path().ok_or_else(none)?,
            config: crate::settings::config_path().ok_or_else(none)?,
            data: base("XDG_STATE_HOME", ".local/state", "Library/Application Support")
                .ok_or_else(none)?
                .join("lumen/plugin-data"),
        })
    }
}

/// How permissions a plugin hasn't been allowed yet get approved.
pub enum Approve<'a> {
    /// Approved already (`--yes`).
    Yes,
    /// Ask: given the plugin's name and only the new permissions.
    Ask(&'a dyn Fn(&str, &[Permission]) -> bool),
    /// Nobody to ask: refuse.
    No,
}

/// An advisory lock on `path` (created if missing), held until dropped, so
/// two `lumen plugins` at once, or `lumen plugins` and Lumen itself, take
/// turns at the files they both change.
pub struct Lock(std::fs::File);

impl Lock {
    pub fn take(path: &Path) -> Result<Lock, String> {
        use std::os::fd::AsRawFd;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(format!("{}: couldn't lock it", path.display()));
        }
        Ok(Lock(file))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

impl Paths {
    /// Held while installing, updating, removing or turning a plugin on or
    /// off. (Approvals have their own lock, in `plugins.rs`.)
    fn lock(&self) -> Result<Lock, String> {
        Lock::take(&self.installed.with_file_name("plugins.lock"))
    }
}

/// This Lumen's version, for choosing compatible plugin versions.
pub const LUMEN_VERSION: &str = env!("CARGO_PKG_VERSION");

fn find<'a>(index: &'a Index, name: &str) -> Result<&'a Entry, String> {
    index
        .plugins
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| format!("there's no plugin called \"{name}\" in the registry"))
}

fn newest(entry: &Entry) -> Result<&Release, String> {
    best(entry, LUMEN_VERSION).ok_or_else(|| {
        let need = entry
            .versions
            .iter()
            .map(|r| r.lumen.as_str())
            .fold("", |a, b| if a.is_empty() || newer(a, b) { b } else { a });
        format!("{} needs Lumen {need} or newer; this is {LUMEN_VERSION}", entry.name)
    })
}

pub fn install(index: &Index, name: &str, approve: &Approve) -> Result<String, String> {
    install_in(&Paths::real()?, index, name, approve)
}

pub fn update(index: &Index, name: &str, approve: &Approve) -> Result<Option<String>, String> {
    update_in(&Paths::real()?, index, name, approve)
}

pub fn remove(name: &str, delete_data: bool) -> Result<String, String> {
    remove_in(&Paths::real()?, name, delete_data)
}

pub fn set_enabled(name: &str, on: bool) -> Result<(), String> {
    set_enabled_in(&Paths::real()?, name, on)
}

/// Install the newest compatible version of `name`, and turn it on.
pub(crate) fn install_in(p: &Paths, index: &Index, name: &str, approve: &Approve) -> Result<String, String> {
    let _lock = p.lock()?;
    let entry = find(index, name)?;
    let release = newest(entry)?;
    if let Some(i) = load_installed_from(&p.installed).get(name).filter(|i| i.version == release.version) {
        // Installed already; its approval may have been lost since.
        approve_new(p, name, &i.permissions, approve)?;
        set_enabled_locked(p, name, true)?;
        return Ok(format!("{name} {} is already installed", release.version));
    }
    put_in_place(p, entry, release, approve)?;
    Ok(format!("installed {name} {}", release.version))
}

/// Move `name` to its newest compatible version; `None` when it has it.
pub(crate) fn update_in(p: &Paths, index: &Index, name: &str, approve: &Approve) -> Result<Option<String>, String> {
    let _lock = p.lock()?;
    let current = load_installed_from(&p.installed).remove(name).ok_or_else(|| format!("{name} isn't installed"))?;
    let entry = find(index, name)?;
    let release = newest(entry)?;
    if !newer(&release.version, &current.version) {
        return Ok(None);
    }
    put_in_place(p, entry, release, approve)?;
    Ok(Some(format!("updated {name} {} → {}", current.version, release.version)))
}

/// Remove `name`'s files, its record, its approval and its lines in
/// config.toml; its data too if `delete_data`.
pub(crate) fn remove_in(p: &Paths, name: &str, delete_data: bool) -> Result<String, String> {
    let _lock = p.lock()?;
    let mut all = load_installed_from(&p.installed);
    if all.remove(name).is_none() {
        return Err(format!("{name} isn't installed"));
    }
    if !valid_name(name) {
        return Err(format!("\"{name}\" isn't a registry name"));
    }
    save_installed_to(&p.installed, &all)?;
    let folder = p.root.join(name);
    if folder.exists() {
        std::fs::remove_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    crate::plugins::forget_registry_in(&p.approvals, name)?;
    crate::settings::edit_config_at(&p.config, |doc| {
        remove_from_list(doc, name);
        adopt_old_table(doc, name);
        if let Some(t) = doc.get_mut("plugin").and_then(|i| i.as_table_like_mut()) {
            t.remove(name);
        }
    })?;
    if delete_data {
        // Its data from when it was built in, if it never ran since.
        if let Some((old, _)) = crate::plugins::MOVED.iter().find(|(_, new)| *new == name) {
            let before = p.data.join(old);
            if before.exists() {
                std::fs::remove_dir_all(&before).map_err(|e| format!("{}: {e}", before.display()))?;
            }
        }
        let data = p.data.join(name);
        if data.exists() {
            std::fs::remove_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;
        }
        return Ok(format!("removed {name} and its data"));
    }
    Ok(format!("removed {name} (its data is kept in {})", p.data.join(name).display()))
}

fn remove_from_list(doc: &mut toml_edit::DocumentMut, name: &str) {
    if let Some(list) = doc.get_mut("plugins").and_then(|i| i.as_array_mut()) {
        // A plugin that moved to the registry may still be listed by its
        // built-in name.
        let gone =
            |v: &toml_edit::Value| v.as_str().is_some_and(|v| v == name || crate::plugins::moved_to(v) == Some(name));
        // The first item's spacing goes to whichever comes first after.
        let first = list.get(0).map(|v| v.decor().prefix().cloned());
        list.retain(|v| !gone(v));
        if let (Some(Some(prefix)), Some(v)) = (first, list.get_mut(0)) {
            v.decor_mut().set_prefix(prefix);
        }
    }
}

/// A plugin that moved to the registry takes over its `[plugin.OLD]` table:
/// renamed to `[plugin."NEW"]`, under any keys that table has already. So a
/// later edit or reset of one setting can't bring back or drop the others.
pub(crate) fn adopt_old_table(doc: &mut toml_edit::DocumentMut, name: &str) {
    let Some((old, _)) = crate::plugins::MOVED.iter().find(|(_, new)| *new == name) else { return };
    let Some(plugins) = doc.get_mut("plugin").and_then(|i| i.as_table_mut()) else { return };
    let Some(Ok(old_table)) = plugins.remove(old).map(toml_edit::Item::into_table) else { return };
    if !plugins.contains_key(name) {
        plugins.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    if let Some(new_table) = plugins.get_mut(name).and_then(|t| t.as_table_mut()) {
        for (key, value) in old_table.iter() {
            if !new_table.contains_key(key) {
                new_table.insert(key, value.clone());
            }
        }
    }
}

/// Turn a plugin on or off: add it to, or take it out of, `plugins`.
pub(crate) fn set_enabled_in(p: &Paths, name: &str, on: bool) -> Result<(), String> {
    let _lock = p.lock()?;
    set_enabled_locked(p, name, on)
}

/// [`set_enabled_in`], with the lock already held.
fn set_enabled_locked(p: &Paths, name: &str, on: bool) -> Result<(), String> {
    crate::settings::edit_config_at(&p.config, |doc| {
        if !doc.get("plugins").is_some_and(|i| i.is_array()) {
            doc["plugins"] = toml_edit::value(toml_edit::Array::new());
        }
        remove_from_list(doc, name);
        if on && let Some(list) = doc["plugins"].as_array_mut() {
            list.push(name);
        }
    })
}

/// A temporary folder under the install root, removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Download, check, approve and install one release, then switch over to
/// it. Until the switch nothing installed changes, and a switch that fails
/// part way is undone, so a failure leaves the old version (if any) in use.
fn put_in_place(p: &Paths, entry: &Entry, release: &Release, approve: &Approve) -> Result<(), String> {
    let name = entry.name.as_str();
    if !valid_name(name) {
        return Err(format!("\"{name}\" isn't a registry name"));
    }
    if !valid_version(&release.version) {
        return Err(format!("{name}: \"{}\" isn't a version", release.version));
    }
    std::fs::create_dir_all(&p.root).map_err(|e| format!("{}: {e}", p.root.display()))?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let scratch = Scratch(p.root.join(format!(".download-{}-{stamp}", std::process::id())));
    std::fs::create_dir_all(&scratch.0).map_err(|e| format!("{}: {e}", scratch.0.display()))?;
    let archive = scratch.0.join("archive.tar.gz");
    fetch(&release.archive, &archive)?;
    let top = unpack(&archive, &scratch.0.join("files"))?;
    let sub = Path::new(&entry.path);
    if sub.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return Err(format!("{name}: its `path` \"{}\" isn't a folder inside the repository", entry.path));
    }
    let folder = top.join(sub);
    let text = std::fs::read_to_string(folder.join("plugin.toml"))
        .map_err(|_| format!("{name} {}: no plugin.toml in the download", release.version))?;
    let manifest = crate::plugins::parse_manifest(&folder, &text).map_err(|e| format!("{name}: plugin.toml: {e}"))?;
    if manifest.version != release.version {
        return Err(format!(
            "{name}: its plugin.toml says version {}, the registry says {}; not installed",
            manifest.version, release.version
        ));
    }
    let unlisted: Vec<&str> =
        manifest.permissions.iter().filter(|x| !release.permissions.contains(x)).map(|x| x.name()).collect();
    if !unlisted.is_empty() || release.permissions.iter().any(|x| !manifest.permissions.contains(x)) {
        return Err(format!(
            "{name} {}: its plugin.toml asks for {}, which doesn't match the registry; not installed",
            release.version,
            if unlisted.is_empty() { "different permissions".to_string() } else { unlisted.join(", ") }
        ));
    }
    check_approval(p, name, &manifest.permissions, approve)?;
    // The switch: the files, their record, the approval, then turning it
    // on. Each step that fails undoes the ones before it.
    let dest = p.root.join(name).join(&release.version);
    // The same version installed again: its files are kept aside until
    // the new ones are in use.
    let aside = scratch.0.join("previous");
    let had_files = dest.exists();
    if had_files {
        std::fs::rename(&dest, &aside).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    let put_files_back = || {
        let _ = std::fs::remove_dir_all(&dest);
        if had_files {
            let _ = std::fs::rename(&aside, &dest);
        }
    };
    let moved = std::fs::create_dir_all(dest.parent().ok_or("no folder")?)
        .and_then(|_| std::fs::rename(&folder, &dest))
        .map_err(|e| format!("{}: {e}", dest.display()));
    if let Err(e) = moved {
        put_files_back();
        return Err(e);
    }

    let before = load_installed_from(&p.installed);
    let mut all = before.clone();
    let old = all.insert(
        name.to_string(),
        Installed {
            version: release.version.clone(),
            commit: release.commit.clone(),
            permissions: manifest.permissions.clone(),
            dir: dest.clone(),
        },
    );
    if let Err(e) = save_installed_to(&p.installed, &all) {
        put_files_back();
        return Err(e);
    }
    let put_record_back = || {
        let _ = save_installed_to(&p.installed, &before);
        put_files_back();
    };

    let approved_before = crate::plugins::registry_approval_in(&p.approvals, name);
    if !manifest.permissions.is_empty()
        && let Err(e) = crate::plugins::approve_registry_in(&p.approvals, name, &manifest.permissions)
    {
        put_record_back();
        return Err(format!("{name}: couldn't keep your approval ({e}); not installed"));
    }
    if let Err(e) = set_enabled_locked(p, name, true) {
        let _ = if approved_before.is_empty() {
            crate::plugins::forget_registry_in(&p.approvals, name)
        } else {
            crate::plugins::approve_registry_in(&p.approvals, name, &approved_before)
        };
        put_record_back();
        return Err(format!("{name}: couldn't turn it on ({e}); not installed"));
    }
    if let Some(old) = old.filter(|o| o.dir != dest) {
        let _ = std::fs::remove_dir_all(&old.dir);
    }
    Ok(())
}

/// Whether `permissions` are allowed for `name`: those not approved already
/// are approved now (`approve` says how), or it's an error.
fn check_approval(p: &Paths, name: &str, permissions: &[Permission], approve: &Approve) -> Result<(), String> {
    let new = new_permissions_in(p, name, permissions);
    if new.is_empty() {
        return Ok(());
    }
    match approve {
        Approve::Yes => Ok(()),
        Approve::Ask(ask) if ask(name, &new) => Ok(()),
        Approve::Ask(_) => Err(format!("{name} wasn't allowed; nothing was changed")),
        Approve::No => {
            let names: Vec<&str> = new.iter().map(|x| x.name()).collect();
            Err(format!("{name} needs your approval to use: {}; add --yes to approve", names.join(", ")))
        }
    }
}

/// Which of `permissions` aren't approved yet for `name`: what installing
/// or updating would ask about.
pub(crate) fn new_permissions_in(p: &Paths, name: &str, permissions: &[Permission]) -> Vec<Permission> {
    let allowed = crate::plugins::registry_approval_in(&p.approvals, name);
    permissions.iter().copied().filter(|x| !allowed.contains(x)).collect()
}

/// Approve what an installed plugin uses, asking for anything not approved.
fn approve_new(p: &Paths, name: &str, permissions: &[Permission], approve: &Approve) -> Result<(), String> {
    check_approval(p, name, permissions, approve)?;
    if !permissions.is_empty() {
        crate::plugins::approve_registry_in(&p.approvals, name, permissions)?;
    }
    Ok(())
}

/// How much a plugin's download may hold. Plugins are source files, so
/// these are far above any real one, and only stop an archive made to fill
/// the disk or keep `tar` busy.
pub(crate) struct Limits {
    /// The `.tar.gz` itself, in bytes.
    pub archive: u64,
    /// Files and folders in it.
    pub entries: usize,
    /// One file, unpacked, in bytes.
    pub file: u64,
    /// Everything, unpacked, in bytes.
    pub unpacked: u64,
}

const MB: u64 = 1024 * 1024;
pub(crate) const LIMITS: Limits = Limits { archive: 50 * MB, entries: 10_000, file: 50 * MB, unpacked: 200 * MB };

/// `bytes` for a message: whole megabytes, or kilobytes below one.
fn size(bytes: u64) -> String {
    if bytes >= MB { format!("{} MB", bytes / MB) } else { format!("{} KB", bytes.div_ceil(1024)) }
}

/// The size on a line of `tar -tv`, if it can be told. GNU tar prints
/// `mode owner/group size date time name`; BSD tar (macOS) prints
/// `mode links owner group size month day time name`.
fn listed_size(line: &str) -> Option<u64> {
    let fields: Vec<&str> = line.split_whitespace().take(5).collect();
    let at = if fields.get(1)?.contains('/') { 2 } else { 4 };
    fields.get(at)?.parse().ok()
}

/// Run `tar <flags> <archive>`, giving `each` every line it prints. An
/// error from `each` stops tar there, so a listing made to go on for ever
/// doesn't. Returns whether tar succeeded.
fn tar_lines(flags: &str, archive: &Path, mut each: impl FnMut(&str) -> Result<(), String>) -> Result<bool, String> {
    use std::io::{BufRead, Read};
    /// Longer than any real path; a longer line isn't a listing.
    const LINE: usize = 64 * 1024;
    let mut child = std::process::Command::new("tar")
        .arg(flags)
        .arg(archive)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't run tar: {e}"))?;
    let mut out = std::io::BufReader::new(child.stdout.take().expect("piped"));
    let mut buf = Vec::new();
    let result = loop {
        buf.clear();
        match (&mut out).take(LINE as u64 + 1).read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break Ok(()),
            Ok(_) if buf.len() > LINE => {
                break Err("the download has a name too long to be real; not installed".to_string());
            }
            Ok(_) => {}
        }
        if let Err(e) = each(String::from_utf8_lossy(&buf).trim_end_matches('\n')) {
            break Err(e);
        }
    };
    if result.is_err() {
        let _ = child.kill();
    }
    drop(out);
    let status = child.wait().map_err(|e| format!("couldn't run tar: {e}"))?;
    result.map(|()| status.success())
}

/// Whether `archive`'s files come to more than `limit` bytes, found by having
/// tar unpack them to a pipe and counting. Unlike the sizes in a listing,
/// which sit after names the archive chooses, this can't be talked round;
/// and nothing is written. Tar is stopped as soon as the answer is yes.
fn unpacks_past(archive: &Path, limit: u64) -> Result<bool, String> {
    use std::io::Read;
    let mut child = std::process::Command::new("tar")
        .arg("-xzOf")
        .arg(archive)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't run tar: {e}"))?;
    let out = child.stdout.take().expect("piped");
    let seen = std::io::copy(&mut out.take(limit.saturating_add(1)), &mut std::io::sink());
    let past = seen.as_ref().is_ok_and(|n| *n > limit);
    if past || seen.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| format!("couldn't run tar: {e}"))?;
    match seen {
        Ok(_) if past => Ok(true),
        Ok(_) if status.success() => Ok(false),
        _ => Err("the download isn't a .tar.gz archive; not installed".to_string()),
    }
}

/// Stop `cmd` writing any file longer than `cap` bytes: whatever a listing
/// said, the kernel ends it there (SIGXFSZ).
fn cap_file_size(cmd: &mut std::process::Command, cap: u64) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(move || {
            let limit = libc::rlimit { rlim_cur: cap as libc::rlim_t, rlim_max: cap as libc::rlim_t };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Unpack a `.tar.gz` into `into` and return its one top-level folder.
/// Refuses archives with absolute paths, `..`, links or special files, so
/// nothing can be written outside `into`, and ones holding more than
/// [`LIMITS`], so nothing can fill the disk.
fn unpack(archive: &Path, into: &Path) -> Result<PathBuf, String> {
    unpack_within(archive, into, &LIMITS)
}

fn unpack_within(archive: &Path, into: &Path, limits: &Limits) -> Result<PathBuf, String> {
    let bad = |why: &str| format!("the download {why}; not installed");
    let too_many = || bad(&format!("has more than {} files", limits.entries));
    let too_big = || bad(&format!("unpacks to more than {}", size(limits.unpacked)));
    let file_too_big = || bad(&format!("has a file over {}", size(limits.file)));
    let packed = std::fs::metadata(archive).map_err(|e| format!("{}: {e}", archive.display()))?.len();
    if packed > limits.archive {
        return Err(bad(&format!("is over {}", size(limits.archive))));
    }

    // Read the listings before anything is unpacked, stopping at the first
    // thing wrong. Links are refused here: not every tar refuses to write
    // through one by itself. GNU tar marks them `l` and `h`; BSD tar shows
    // hard links as "link to".
    let (mut entries, mut total) = (0usize, 0u64);
    let listed = tar_lines("-tvzf", archive, |line| {
        if !(line.starts_with('-') || line.starts_with('d')) || line.contains(" link to ") || line.contains(" -> ") {
            return Err(bad("has a link or special file"));
        }
        entries += 1;
        if entries > limits.entries {
            return Err(too_many());
        }
        // An early answer only: the owner's name comes before the size and
        // can put it out of place, so the total is counted again below.
        if let Some(bytes) = listed_size(line).filter(|_| line.starts_with('-')) {
            if bytes > limits.file {
                return Err(file_too_big());
            }
            total = total.saturating_add(bytes);
            if total > limits.unpacked {
                return Err(too_big());
            }
        }
        Ok(())
    })?;
    let mut names = 0usize;
    let named = tar_lines("-tzf", archive, |entry| {
        names += 1;
        if names > limits.entries {
            return Err(too_many());
        }
        let path = Path::new(entry);
        if path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(bad("has files outside its folder"));
        }
        Ok(())
    })?;
    if !listed || !named {
        return Err(bad("isn't a .tar.gz archive"));
    }
    if unpacks_past(archive, limits.unpacked)? {
        return Err(too_big());
    }

    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let mut tar = std::process::Command::new("tar");
    tar.arg("-xzf").arg(archive).arg("-C").arg(into).args(["--no-same-owner", "--no-same-permissions"]);
    cap_file_size(&mut tar, limits.file);
    let out = tar.output().map_err(|e| format!("couldn't run tar: {e}"))?;
    if !out.status.success() {
        use std::os::unix::process::ExitStatusExt;
        if out.status.signal() == Some(libc::SIGXFSZ) {
            return Err(file_too_big());
        }
        return Err(bad(&format!("couldn't be unpacked ({})", String::from_utf8_lossy(&out.stderr).trim())));
    }
    // Checked again once unpacked, in case the listing missed something.
    fn check(dir: &Path, limits: &Limits, entries: &mut usize, total: &mut u64) -> Result<(), String> {
        let over = |what: String| format!("the download {what}; not installed");
        for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
            let kind = e.file_type().map_err(|e| e.to_string())?;
            *entries += 1;
            if *entries > limits.entries {
                return Err(over(format!("has more than {} files", limits.entries)));
            }
            if kind.is_dir() {
                check(&e.path(), limits, entries, total)?;
            } else if !kind.is_file() {
                return Err(over(format!("has a link or special file ({})", e.file_name().to_string_lossy())));
            } else {
                let bytes = e.metadata().map_err(|e| e.to_string())?.len();
                if bytes > limits.file {
                    return Err(over(format!("has a file over {}", size(limits.file))));
                }
                *total = total.saturating_add(bytes);
                if *total > limits.unpacked {
                    return Err(over(format!("unpacks to more than {}", size(limits.unpacked))));
                }
            }
        }
        Ok(())
    }
    check(into, limits, &mut 0, &mut 0)?;
    let tops: Vec<PathBuf> = std::fs::read_dir(into).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    match tops.as_slice() {
        [one] if one.is_dir() => Ok(one.clone()),
        _ => Err(bad("should hold exactly one folder")),
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub(crate) fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-registry-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(crate) fn entry(name: &str, stars: u64, versions: &[(&str, &str)]) -> Entry {
        Entry {
            name: name.into(),
            repo: format!("https://github.com/{name}"),
            path: String::new(),
            description: format!("{name} does things"),
            license: "MIT".into(),
            categories: vec!["git".into()],
            stars,
            updated: "2026-11-01".into(),
            versions: versions
                .iter()
                .map(|(v, lumen)| Release {
                    version: v.to_string(),
                    commit: "c".repeat(40),
                    permissions: vec![],
                    lumen: lumen.to_string(),
                    archive: String::new(),
                })
                .collect(),
        }
    }

    pub(crate) fn paths(name: &str) -> (PathBuf, Paths) {
        let dir = temp(name);
        let p = Paths {
            root: dir.join("installed-plugins"),
            installed: dir.join("state/installed.json"),
            approvals: dir.join("state/plugin-approvals.json"),
            config: dir.join("config/config.toml"),
            data: dir.join("state/plugin-data"),
        };
        (dir, p)
    }

    /// A tarball of a plugin, as GitHub would serve it: one top folder.
    pub(crate) fn archive(dir: &Path, version: &str, perms: &str) -> String {
        let top = dir.join(format!("src-{version}/x-{version}"));
        std::fs::create_dir_all(&top).unwrap();
        std::fs::write(
            top.join("plugin.toml"),
            format!("version = \"{version}\"\ncommand = \"python3 main.py\"\npermissions = [{perms}]\n"),
        )
        .unwrap();
        std::fs::write(top.join("main.py"), "print('hi')\n").unwrap();
        let out = dir.join(format!("x-{version}.tar.gz"));
        let ok = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&out)
            .arg("-C")
            .arg(top.parent().unwrap())
            .arg(format!("x-{version}"))
            .status()
            .unwrap()
            .success();
        assert!(ok);
        out.to_str().unwrap().to_string()
    }

    pub(crate) fn release(version: &str, perms: Vec<Permission>, archive: String) -> Release {
        Release { version: version.into(), commit: "c".repeat(40), permissions: perms, lumen: "1.0.0".into(), archive }
    }

    pub(crate) fn index_with(releases: Vec<Release>) -> Index {
        let mut e = entry("mira/x", 1, &[]);
        e.versions = releases;
        Index { version: 1, plugins: vec![e] }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use std::time::Duration;

    #[test]
    fn updates_are_counted() {
        let installed =
            |v: &str| Installed { version: v.into(), commit: "c".repeat(40), permissions: vec![], dir: PathBuf::new() };
        let all =
            BTreeMap::from([("mira/x".to_string(), installed("1.0.0")), ("mira/y".to_string(), installed("1.0.0"))]);
        let index = Index {
            version: 1,
            plugins: vec![
                entry("mira/x", 1, &[("1.0.0", "1.0.0"), ("1.1.0", "1.0.0")]),
                entry("mira/y", 1, &[("1.0.0", "1.0.0"), ("2.0.0", "99.0.0")]),
                entry("mira/z", 1, &[("5.0.0", "1.0.0")]),
            ],
        };
        assert_eq!(count_updates(&all, &index), 1);
    }

    #[test]
    fn the_index_is_kept_for_a_day() {
        let dir = temp("index-day");
        let cache = dir.join("cache.json");
        std::fs::write(&cache, r#"{"version": 1, "plugins": []}"#).unwrap();
        let two_hours_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 3600);
        std::fs::File::options().write(true).open(&cache).unwrap().set_modified(two_hours_ago).unwrap();
        let nowhere = dir.join("missing.json").display().to_string();
        assert!(index_within(&nowhere, &cache, Duration::from_secs(24 * 3600)).is_ok(), "read from the cache");
        assert!(index_within(&nowhere, &cache, Duration::from_secs(3600)).is_err(), "too old: fetched, and that fails");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn versions_compare() {
        assert!(newer("1.10.0", "1.9.9"));
        assert!(!newer("1.9.9", "1.10.0"));
        assert!(!newer("1.2", "1.2.0") && !newer("1.2.0", "1.2"));
        assert!(!newer("1.3.0-beta", "1.3.0") && !newer("1.3.0", "1.3.0-beta"));
        assert!(newer("2", "1.99.99"));
    }

    #[test]
    fn best_skips_versions_needing_newer_lumen() {
        let e = entry("mira/x", 0, &[("1.0.0", "1.2.0"), ("1.1.0", "1.3.0"), ("2.0.0", "1.9.0")]);
        assert_eq!(best(&e, "1.3.0").unwrap().version, "1.1.0");
        assert_eq!(best(&e, "1.2.5").unwrap().version, "1.0.0");
        assert!(best(&e, "1.1.0").is_none());
        assert_eq!(best(&e, "2.0.0").unwrap().version, "2.0.0");
    }

    #[test]
    fn search_matches_and_ranks_by_stars() {
        let mut docker = entry("mira/docker-panes", 10, &[("1.0.0", "1.0.0")]);
        docker.categories = vec!["containers".into()];
        let index = Index {
            version: 1,
            plugins: vec![
                entry("a/git-tools", 5, &[("1.0.0", "1.0.0")]),
                docker,
                entry("b/git-more", 50, &[("1.0.0", "1.0.0")]),
            ],
        };
        let names = |text: &str| search(&index, text).iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("GIT"), ["b/git-more", "a/git-tools"], "any case, most stars first");
        assert_eq!(names("containers"), ["mira/docker-panes"], "categories match too");
        assert_eq!(names(""), ["b/git-more", "mira/docker-panes", "a/git-tools"]);
        assert!(names("nothing like it").is_empty());
    }

    #[test]
    fn index_reads_a_local_file_and_rejects_new_formats() {
        let dir = temp("index");
        let source = dir.join("index.json");
        std::fs::write(
            &source,
            r#"{"version": 1, "plugins": [{"name": "mira/x", "repo": "https://github.com/mira/x", "description": "d",
                "license": "MIT", "categories": [], "stars": 3, "updated": "2026-11-01",
                "versions": [{"version": "1.0.0", "commit": "cccccccccccccccccccccccccccccccccccccccc",
                              "permissions": ["panes"], "lumen": "1.2.0", "archive": "file:///x.tar.gz"}]}]}"#,
        )
        .unwrap();
        let cache = dir.join("cache.json");
        let index = index_from(source.to_str().unwrap(), &cache, false).unwrap();
        assert_eq!(index.plugins[0].name, "mira/x");
        assert_eq!(index.plugins[0].versions[0].permissions, [Permission::Panes]);
        assert!(cache.exists(), "kept for next time");
        // Fresh cache: the source isn't read again.
        std::fs::write(&source, "garbage").unwrap();
        assert!(index_from(source.to_str().unwrap(), &cache, false).is_ok());
        assert!(index_from(source.to_str().unwrap(), &cache, true).is_err(), "refresh reads the source");
        std::fs::write(&source, r#"{"version": 2, "plugins": []}"#).unwrap();
        let e = index_from(source.to_str().unwrap(), &cache, true).unwrap_err();
        assert!(e.contains("update Lumen"), "{e}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A home for one test: every path under `dir`.
    fn config(p: &Paths) -> String {
        std::fs::read_to_string(&p.config).unwrap_or_default()
    }

    #[test]
    fn versions_from_the_index_must_be_plain() {
        let (dir, p) = paths("badversion");
        let victim = p.root.join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("keep"), "x").unwrap();
        for v in ["../../victim", "1.0.0/../../../victim", "", "1.0", "1.0.0-x"] {
            let idx = index_with(vec![release(v, vec![], archive(&dir, "1.0.0", ""))]);
            let err = install_in(&p, &idx, "mira/x", &Approve::Yes).unwrap_err();
            assert!(err.contains("isn't a version"), "{v}: {err}");
        }
        assert!(victim.join("keep").is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn install_puts_it_in_place() {
        let (dir, p) = paths("install");
        let idx = index_with(vec![release("1.0.0", vec![Permission::Panes], archive(&dir, "1.0.0", "\"panes\""))]);
        let msg = install_in(&p, &idx, "mira/x", &Approve::Yes).unwrap();
        assert!(msg.contains("mira/x 1.0.0"), "{msg}");
        assert!(p.root.join("mira/x/1.0.0/plugin.toml").is_file());
        assert!(p.root.join("mira/x/1.0.0/main.py").is_file());
        let all = load_installed_from(&p.installed);
        assert_eq!(all["mira/x"].version, "1.0.0");
        assert_eq!(all["mira/x"].dir, p.root.join("mira/x/1.0.0"));
        assert!(config(&p).contains("plugins = [\"mira/x\"]"), "{}", config(&p));
        assert_eq!(crate::plugins::registry_approval_in(&p.approvals, "mira/x"), vec![Permission::Panes]);
        let leftovers: Vec<_> = std::fs::read_dir(&p.root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers, [std::ffi::OsString::from("mira")], "no temporary folders left");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installing_again_restores_a_lost_approval() {
        let (dir, p) = paths("reapprove");
        let idx = index_with(vec![release("1.0.0", vec![Permission::Panes], archive(&dir, "1.0.0", "\"panes\""))]);
        install_in(&p, &idx, "mira/x", &Approve::Yes).unwrap();
        std::fs::remove_file(&p.approvals).unwrap();
        assert!(install_in(&p, &idx, "mira/x", &Approve::No).unwrap_err().contains("--yes"), "asks again");
        install_in(&p, &idx, "mira/x", &Approve::Yes).unwrap();
        assert_eq!(crate::plugins::registry_approval_in(&p.approvals, "mira/x"), vec![Permission::Panes]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installs_at_once_all_land() {
        let (dir, p) = paths("together");
        let mut idx = Index { version: 1, plugins: vec![] };
        for i in 0..8 {
            let mut e = entry(&format!("mira/p{i}"), 0, &[]);
            let sub = dir.join(format!("p{i}"));
            std::fs::create_dir_all(&sub).unwrap();
            e.versions = vec![release("1.0.0", vec![Permission::Panes], archive(&sub, "1.0.0", "\"panes\""))];
            idx.plugins.push(e);
        }
        std::thread::scope(|s| {
            for i in 0..8 {
                let (p, idx) = (&p, &idx);
                s.spawn(move || install_in(p, idx, &format!("mira/p{i}"), &Approve::Yes).unwrap());
            }
        });
        assert_eq!(load_installed_from(&p.installed).len(), 8, "no install lost");
        let approvals =
            (0..8).filter(|i| !crate::plugins::registry_approval_in(&p.approvals, &format!("mira/p{i}")).is_empty());
        assert_eq!(approvals.count(), 8, "no approval lost");
        let config = config(&p);
        assert_eq!((0..8).filter(|i| config.contains(&format!("\"mira/p{i}\""))).count(), 8, "{config}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn permissions_must_match_the_index() {
        let (dir, p) = paths("perms-match");
        let idx = index_with(vec![release("1.0.0", vec![], archive(&dir, "1.0.0", "\"input\""))]);
        let e = install_in(&p, &idx, "mira/x", &Approve::Yes).unwrap_err();
        assert!(e.contains("input"), "{e}");
        assert!(!p.root.join("mira").exists());
        assert!(load_installed_from(&p.installed).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Everything an install changes, to compare before and after.
    fn state(p: &Paths) -> (String, Vec<Permission>) {
        fn files(dir: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                if e.path().is_dir() {
                    files(&e.path(), out);
                } else {
                    out.push(format!(
                        "{}={}",
                        e.path().display(),
                        std::fs::read_to_string(e.path()).unwrap_or_default()
                    ));
                }
            }
        }
        let mut found = Vec::new();
        files(&p.root, &mut found);
        found.sort();
        let installed: Vec<_> =
            load_installed_from(&p.installed).into_iter().map(|(n, i)| (n, i.version, i.dir)).collect();
        (
            format!("{found:?} {installed:?} {:?}", config(p)),
            crate::plugins::registry_approval_in(&p.approvals, "mira/x"),
        )
    }

    #[test]
    fn an_install_that_cant_keep_its_approval_changes_nothing() {
        let (dir, p) = paths("no-approval");
        let idx = index_with(vec![release("1.0.0", vec![Permission::Panes], archive(&dir, "1.0.0", "\"panes\""))]);
        // The approvals file can't be written: a folder is in its way.
        std::fs::create_dir_all(&p.approvals).unwrap();
        let before = state(&p);
        let e = install_in(&p, &idx, "mira/x", &Approve::Yes).unwrap_err();
        assert!(e.contains("couldn't keep your approval") && e.contains("not installed"), "{e}");
        assert_eq!(state(&p), before);
        assert!(!p.root.join("mira/x").exists() || std::fs::read_dir(p.root.join("mira/x")).unwrap().next().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_update_that_cant_be_turned_on_leaves_the_old_version() {
        let (dir, p) = paths("no-enable");
        let v1 = release("1.0.0", vec![Permission::Panes], archive(&dir, "1.0.0", "\"panes\""));
        let v2 = release(
            "1.1.0",
            vec![Permission::Panes, Permission::Input],
            archive(&dir, "1.1.0", "\"panes\", \"input\""),
        );
        install_in(&p, &index_with(vec![v1.clone()]), "mira/x", &Approve::Yes).unwrap();
        let before = state(&p);
        // config.toml can't be rewritten: its folder is read-only.
        use std::os::unix::fs::PermissionsExt;
        let folder = p.config.parent().unwrap().to_path_buf();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = update_in(&p, &index_with(vec![v1, v2]), "mira/x", &Approve::Yes);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        let e = result.unwrap_err();
        assert!(e.contains("couldn't turn it on") && e.contains("not installed"), "{e}");
        // The old files, record and approval (without `input`) are as they were.
        assert_eq!(state(&p), before);
        assert_eq!(before.1, vec![Permission::Panes]);
        assert!(!p.root.join("mira/x/1.1.0").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refused_approval_installs_nothing() {
        let (dir, p) = paths("refused");
        let idx = index_with(vec![release("1.0.0", vec![Permission::Input], archive(&dir, "1.0.0", "\"input\""))]);
        let no = |_: &str, _: &[Permission]| false;
        assert!(install_in(&p, &idx, "mira/x", &Approve::Ask(&no)).is_err());
        assert!(install_in(&p, &idx, "mira/x", &Approve::No).unwrap_err().contains("--yes"));
        assert!(!p.root.join("mira").exists());
        assert!(load_installed_from(&p.installed).is_empty());
        assert_eq!(config(&p), "");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn update_without_new_permissions_does_not_ask() {
        let (dir, p) = paths("update-quiet");
        let v1 = release("1.0.0", vec![Permission::Panes], archive(&dir, "1.0.0", "\"panes\""));
        install_in(&p, &index_with(vec![v1.clone()]), "mira/x", &Approve::Yes).unwrap();
        let v2 = release("1.1.0", vec![Permission::Panes], archive(&dir, "1.1.0", "\"panes\""));
        let panic = |_: &str, _: &[Permission]| -> bool { panic!("asked") };
        let msg = update_in(&p, &index_with(vec![v1.clone(), v2.clone()]), "mira/x", &Approve::Ask(&panic)).unwrap();
        assert!(msg.unwrap().contains("1.1.0"));
        assert!(p.root.join("mira/x/1.1.0").is_dir());
        assert!(!p.root.join("mira/x/1.0.0").exists(), "the old version is gone");
        assert_eq!(update_in(&p, &index_with(vec![v1, v2]), "mira/x", &Approve::No).unwrap(), None, "already newest");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn update_with_new_permissions_asks() {
        let (dir, p) = paths("update-asks");
        let v1 = release("1.0.0", vec![Permission::Panes], archive(&dir, "1.0.0", "\"panes\""));
        install_in(&p, &index_with(vec![v1.clone()]), "mira/x", &Approve::Yes).unwrap();
        let v2 = release(
            "2.0.0",
            vec![Permission::Panes, Permission::Screen],
            archive(&dir, "2.0.0", "\"panes\", \"screen\""),
        );
        let asked = std::cell::RefCell::new(Vec::new());
        let ask = |_: &str, new: &[Permission]| {
            asked.borrow_mut().extend_from_slice(new);
            false
        };
        assert!(update_in(&p, &index_with(vec![v1.clone(), v2.clone()]), "mira/x", &Approve::Ask(&ask)).is_err());
        assert_eq!(*asked.borrow(), [Permission::Screen], "only the new permission");
        assert_eq!(load_installed_from(&p.installed)["mira/x"].version, "1.0.0");
        let yes = |_: &str, _: &[Permission]| true;
        update_in(&p, &index_with(vec![v1, v2]), "mira/x", &Approve::Ask(&yes)).unwrap();
        assert_eq!(
            crate::plugins::registry_approval_in(&p.approvals, "mira/x"),
            vec![Permission::Panes, Permission::Screen]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_update_keeps_the_old_version() {
        let (dir, p) = paths("failed-update");
        let v1 = release("1.0.0", vec![], archive(&dir, "1.0.0", ""));
        install_in(&p, &index_with(vec![v1.clone()]), "mira/x", &Approve::Yes).unwrap();
        let v2 = release("2.0.0", vec![], dir.join("missing.tar.gz").to_str().unwrap().to_string());
        assert!(update_in(&p, &index_with(vec![v1, v2]), "mira/x", &Approve::Yes).is_err());
        assert_eq!(load_installed_from(&p.installed)["mira/x"].version, "1.0.0");
        assert!(p.root.join("mira/x/1.0.0/plugin.toml").is_file());
        let in_x: Vec<_> = std::fs::read_dir(p.root.join("mira/x")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(in_x, [std::ffi::OsString::from("1.0.0")]);
        let in_root: Vec<_> = std::fs::read_dir(&p.root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(in_root, [std::ffi::OsString::from("mira")], "no temporary folders left");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn listing_sizes_are_read_from_gnu_and_bsd_tar() {
        assert_eq!(listed_size("-rw-r--r-- root/root      1234 2026-11-18 10:00 x/main.py"), Some(1234));
        assert_eq!(listed_size("-rw-r--r--  0 root   wheel    5678 Nov 18 10:00 x/main.py"), Some(5678));
        assert_eq!(listed_size("drwxr-xr-x root/root         0 2026-11-18 10:00 x/"), Some(0));
        assert_eq!(listed_size("nonsense"), None);
        assert_eq!((size(50 * MB), size(1500)), ("50 MB".to_string(), "2 KB".to_string()));
    }

    #[test]
    fn archives_over_the_limits_are_refused() {
        let (dir, _p) = paths("limits");
        // 40 files of 1000 bytes, which pack down to almost nothing.
        let top = dir.join("src/x");
        std::fs::create_dir_all(&top).unwrap();
        for n in 0..40 {
            std::fs::write(top.join(format!("f{n}")), "0".repeat(1000)).unwrap();
        }
        let archive = dir.join("x.tar.gz");
        let ok = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(dir.join("src"))
            .arg("x")
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let packed = std::fs::metadata(&archive).unwrap().len();
        assert!(packed < 2000, "it compresses well: {packed}");
        let roomy = || Limits { archive: 1 << 20, entries: 100, file: 1 << 20, unpacked: 1 << 20 };
        let mut n = 0;
        let mut attempt = |limits: Limits| {
            n += 1;
            let into = dir.join(format!("out-{n}"));
            (unpack_within(&archive, &into, &limits), into)
        };
        assert!(attempt(roomy()).0.is_ok());
        let (e, _) = attempt(Limits { archive: 100, ..roomy() });
        assert!(e.unwrap_err().contains("is over 1 KB"));
        // Refused from the listing: nothing is unpacked.
        let (e, into) = attempt(Limits { entries: 10, ..roomy() });
        assert!(e.unwrap_err().contains("has more than 10 files") && !into.exists());
        let (e, into) = attempt(Limits { file: 999, ..roomy() });
        assert!(e.unwrap_err().contains("has a file over 1 KB") && !into.exists());
        let (e, into) = attempt(Limits { unpacked: 20_000, ..roomy() });
        assert!(e.unwrap_err().contains("unpacks to more than 20 KB") && !into.exists());
        // Too big to download at all.
        let e = fetch_within(archive.to_str().unwrap(), &dir.join("copy"), 100).unwrap_err();
        assert!(e.contains("is over 1 KB; not downloaded") && !dir.join("copy").exists(), "{e}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_listing_that_hides_sizes_cannot_hide_how_much_unpacks() {
        // The owner's name is the archive's to choose and is printed before
        // the size, so one with spaces in it moves the size out of place.
        let (dir, _p) = paths("owner");
        let archive = dir.join("x.tar.gz");
        let script = "import io, sys, tarfile\nt = tarfile.open(sys.argv[1], 'w:gz')\n\
                      for n in range(40):\n    \
                      i = tarfile.TarInfo(f'x/f{n}'); i.size = 1000; i.uname = 'a/b 0 '; i.gname = 'g'\n    \
                      t.addfile(i, io.BytesIO(b'0' * 1000))\nt.close()";
        let ok = std::process::Command::new("python3").args(["-c", script]).arg(&archive).status().unwrap().success();
        assert!(ok);
        let limits = |unpacked| Limits { archive: 1 << 20, entries: 100, file: 1 << 20, unpacked };
        // Refused before tar writes anything.
        let into = dir.join("out");
        let e = unpack_within(&archive, &into, &limits(20_000)).unwrap_err();
        assert!(e.contains("unpacks to more than 20 KB") && !into.exists(), "{e}");
        assert!(unpack_within(&archive, &into, &limits(40_000)).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tar_cannot_write_a_file_past_the_limit() {
        // What stops a file whose size the listing didn't give away.
        let (dir, _p) = paths("fsize");
        std::fs::create_dir_all(dir.join("src/x")).unwrap();
        std::fs::write(dir.join("src/x/big"), "0".repeat(300_000)).unwrap();
        let archive = dir.join("x.tar.gz");
        assert!(
            std::process::Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(dir.join("src"))
                .arg("x")
                .status()
                .unwrap()
                .success()
        );
        let mut tar = std::process::Command::new("tar");
        std::fs::create_dir_all(dir.join("out")).unwrap();
        tar.arg("-xzf").arg(&archive).arg("-C").arg(dir.join("out"));
        cap_file_size(&mut tar, 100_000);
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(tar.output().unwrap().status.signal(), Some(libc::SIGXFSZ));
        assert!(std::fs::metadata(dir.join("out/x/big")).map_or(0, |m| m.len()) <= 100_000);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn archives_cannot_escape() {
        let (dir, p) = paths("escape");
        // Hand-made archives, as a hostile repository could serve them.
        let make = |name: &str, script: &str| {
            let out = dir.join(name);
            let ok = std::process::Command::new("python3")
                .args(["-c", script, out.to_str().unwrap(), dir.to_str().unwrap()])
                .status()
                .unwrap()
                .success();
            assert!(ok, "{name}");
            out.to_str().unwrap().to_string()
        };
        let head = "import io, sys, tarfile\nt = tarfile.open(sys.argv[1], 'w:gz')\n\
                    def add(name, data=b'x', kind=tarfile.REGTYPE, link=''):\n    \
                    i = tarfile.TarInfo(name); i.type = kind; i.linkname = link; i.size = len(data) if kind == tarfile.REGTYPE else 0\n    \
                    t.addfile(i, io.BytesIO(data) if kind == tarfile.REGTYPE else None)\n\
                    add('x/plugin.toml', b'version = \"1.0.0\"\\ncommand = \"sh x\"\\n')\n";
        let cases = [
            make("dots.tar.gz", &format!("{head}add('x/../../evil-dots')\nt.close()")),
            make("abs.tar.gz", &format!("{head}add(sys.argv[2] + '/evil-abs')\nt.close()")),
            make("link.tar.gz", &format!("{head}add('x/link', kind=tarfile.SYMTYPE, link='/etc')\nt.close()")),
            make("two.tar.gz", &format!("{head}add('y/other')\nt.close()")),
            make(
                "through-link.tar.gz",
                &format!(
                    "{head}add('x/out', kind=tarfile.SYMTYPE, link=sys.argv[2])\nadd('x/out/evil-through')\nt.close()"
                ),
            ),
            make("hard.tar.gz", &format!("{head}add('x/hard', kind=tarfile.LNKTYPE, link='/etc/passwd')\nt.close()")),
        ];
        for archive in cases {
            let idx = index_with(vec![release("1.0.0", vec![], archive.clone())]);
            let e = install_in(&p, &idx, "mira/x", &Approve::Yes).unwrap_err();
            assert!(e.contains("not installed"), "{archive}: {e}");
            if archive.contains("link") || archive.contains("hard") {
                // Refused from the listing, before tar writes anything: not
                // every tar refuses to write through a link by itself.
                assert!(e.contains("has a link"), "{archive}: {e}");
            }
        }
        assert!(!dir.join("evil-abs").exists() && !dir.parent().unwrap().join("evil-dots").exists());
        assert!(!dir.join("evil-through").exists(), "nothing written through a link");
        assert!(load_installed_from(&p.installed).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn remove_keeps_data_unless_asked() {
        let (dir, p) = paths("remove");
        let v1 = release("1.0.0", vec![], archive(&dir, "1.0.0", ""));
        let idx = index_with(vec![v1]);
        install_in(&p, &idx, "mira/x", &Approve::Yes).unwrap();
        std::fs::write(&p.config, "plugins = [\"recent-dirs\", \"mira/x\"]\n\n[plugin.\"mira/x\"]\nn = 1\n").unwrap();
        std::fs::create_dir_all(p.data.join("mira/x")).unwrap();
        remove_in(&p, "mira/x", false).unwrap();
        assert!(p.data.join("mira/x").exists(), "data kept");
        assert!(!p.root.join("mira/x").exists());
        assert!(load_installed_from(&p.installed).is_empty());
        let text = config(&p);
        assert!(text.contains("plugins = [\"recent-dirs\"]") && !text.contains("mira/x"), "{text}");
        install_in(&p, &idx, "mira/x", &Approve::Yes).unwrap();
        remove_in(&p, "mira/x", true).unwrap();
        assert!(!p.data.join("mira/x").exists(), "data deleted when asked");
        assert!(remove_in(&p, "mira/x", false).is_err(), "not installed");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn plugins_turn_on_and_off() {
        let (dir, p) = paths("on-off");
        set_enabled_in(&p, "mira/x", true).unwrap();
        set_enabled_in(&p, "mira/x", true).unwrap();
        assert_eq!(config(&p), "plugins = [\"mira/x\"]\n");
        set_enabled_in(&p, "mira/x", false).unwrap();
        assert_eq!(config(&p), "plugins = []\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn turning_off_a_moved_plugin_removes_its_old_name() {
        let (dir, p) = paths("moved-off");
        std::fs::create_dir_all(p.config.parent().unwrap()).unwrap();
        std::fs::write(&p.config, "plugins = [\"pomodoro\", \"recent-dirs\"]\n").unwrap();
        set_enabled_in(&p, "sn/pomodoro", false).unwrap();
        assert_eq!(config(&p), "plugins = [\"recent-dirs\"]\n");
        std::fs::write(&p.config, "plugins = [\"pomodoro\"]\n").unwrap();
        set_enabled_in(&p, "sn/pomodoro", true).unwrap();
        assert_eq!(config(&p), "plugins = [\"sn/pomodoro\"]\n", "listed once, by its new name");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn removing_a_moved_plugin_takes_its_old_settings_and_data() {
        let (dir, p) = paths("moved-remove");
        std::fs::create_dir_all(p.config.parent().unwrap()).unwrap();
        std::fs::write(&p.config, "plugins = [\"pomodoro\"]\n[plugin.pomodoro]\nlong-break-every = 3\n").unwrap();
        let mut all = BTreeMap::new();
        all.insert(
            "sn/pomodoro".to_string(),
            Installed {
                version: "1.0.0".into(),
                commit: "c".repeat(40),
                permissions: vec![],
                dir: p.root.join("sn/pomodoro"),
            },
        );
        save_installed_to(&p.installed, &all).unwrap();
        std::fs::create_dir_all(p.data.join("pomodoro")).unwrap();
        remove_in(&p, "sn/pomodoro", true).unwrap();
        let text = config(&p);
        assert!(!text.contains("pomodoro"), "{text}");
        assert!(!p.data.join("pomodoro").exists(), "its data from before the move goes too");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn names_are_checked() {
        for good in ["mira/docker-panes", "a/b1", "lumenshell/snake"] {
            assert!(valid_name(good), "{good}");
        }
        let long = format!("a/{}", "b".repeat(40));
        for bad in ["mira", "Mira/x", "mira/-x", "mira/x-", "mira/x/y", "../x", "mira/", "/x", "mi ra/x", long.as_str()]
        {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn installed_records_round_trip() {
        let dir = temp("round-trip");
        let path = dir.join("installed.json");
        let mut all = BTreeMap::new();
        all.insert(
            "mira/x".to_string(),
            Installed {
                version: "1.0.0".into(),
                commit: "a".repeat(40),
                permissions: vec![Permission::Panes],
                dir: dir.join("x"),
            },
        );
        all.insert(
            "mira/y".to_string(),
            Installed { version: "2.1.0".into(), commit: "b".repeat(40), permissions: vec![], dir: dir.join("y") },
        );
        save_installed_to(&path, &all).unwrap();
        assert_eq!(load_installed_from(&path), all);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installed_records_survive_garbage() {
        let dir = temp("garbage");
        let path = dir.join("installed.json");
        std::fs::write(&path, "{\"plugins\":").unwrap();
        assert!(load_installed_from(&path).is_empty());
        assert!(load_installed_from(&dir.join("missing.json")).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
