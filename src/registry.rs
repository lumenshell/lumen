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

fn load_installed_from(path: &Path) -> BTreeMap<String, Installed> {
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

/// Download `url` to `dest` with curl. An absolute path is copied instead
/// (for testing an index or archive before it's published).
pub fn fetch(url: &str, dest: &Path) -> Result<(), String> {
    if url.starts_with('/') {
        return std::fs::copy(url, dest).map(|_| ()).map_err(|e| format!("{url}: {e}"));
    }
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--proto", "=https,file", "--max-time", "120", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "Lumen needs `curl` to download plugins; install it and try again".into(),
            _ => format!("couldn't run curl: {e}"),
        })?;
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
    let url = std::env::var("LUMEN_PLUGIN_INDEX").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| INDEX_URL.into());
    let cache = cache_dir().ok_or("no home folder")?.join("plugin-index.json");
    index_from(&url, &cache, refresh)
}

/// The index kept from the last download, however old, without the network
/// (for `lumen plugins list`).
pub fn cached_index() -> Option<Index> {
    let text = std::fs::read_to_string(cache_dir()?.join("plugin-index.json")).ok()?;
    parse_index(&text).ok()
}

fn parse_index(text: &str) -> Result<Index, String> {
    let version = serde_json::from_str::<serde_json::Value>(text)
        .map_err(|e| format!("the plugin index isn't valid JSON: {e}"))?
        .get("version")
        .and_then(serde_json::Value::as_u64);
    if version != Some(1) {
        return Err("this Lumen is too old for the plugin index; update Lumen".into());
    }
    serde_json::from_str(text).map_err(|e| format!("the plugin index isn't valid: {e}"))
}

fn index_from(url: &str, cache: &Path, refresh: bool) -> Result<Index, String> {
    let fresh = std::fs::metadata(cache)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|age| age < INDEX_FRESH));
    if fresh && !refresh
        && let Some(index) = std::fs::read_to_string(cache).ok().and_then(|t| parse_index(&t).ok())
    {
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
            data: base("XDG_STATE_HOME", ".local/state", "Library/Application Support").ok_or_else(none)?.join("lumen/plugin-data"),
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
    index.plugins.iter().find(|e| e.name == name).ok_or_else(|| format!("there's no plugin called \"{name}\" in the registry"))
}

fn newest(entry: &Entry) -> Result<&Release, String> {
    best(entry, LUMEN_VERSION).ok_or_else(|| {
        let need = entry.versions.iter().map(|r| r.lumen.as_str()).fold("", |a, b| if a.is_empty() || newer(a, b) { b } else { a });
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
fn install_in(p: &Paths, index: &Index, name: &str, approve: &Approve) -> Result<String, String> {
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
fn update_in(p: &Paths, index: &Index, name: &str, approve: &Approve) -> Result<Option<String>, String> {
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
fn remove_in(p: &Paths, name: &str, delete_data: bool) -> Result<String, String> {
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
    crate::plugins::forget_registry_in(&p.approvals, name);
    crate::settings::edit_config_at(&p.config, |doc| {
        remove_from_list(doc, name);
        if let Some(t) = doc.get_mut("plugin").and_then(|i| i.as_table_like_mut()) {
            t.remove(name);
        }
    })?;
    if delete_data {
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
        list.retain(|v| v.as_str() != Some(name));
    }
}

/// Turn a plugin on or off: add it to, or take it out of, `plugins`.
fn set_enabled_in(p: &Paths, name: &str, on: bool) -> Result<(), String> {
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
/// it. Until the switch nothing installed changes, so a failure leaves the
/// old version (if any) in use.
fn put_in_place(p: &Paths, entry: &Entry, release: &Release, approve: &Approve) -> Result<(), String> {
    let name = entry.name.as_str();
    if !valid_name(name) {
        return Err(format!("\"{name}\" isn't a registry name"));
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
    // The switch: move the files into place, then record them.
    let dest = p.root.join(name).join(&release.version);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    std::fs::create_dir_all(dest.parent().ok_or("no folder")?).map_err(|e| e.to_string())?;
    std::fs::rename(&folder, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut all = load_installed_from(&p.installed);
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
        let _ = std::fs::remove_dir_all(&dest);
        return Err(e);
    }
    if !manifest.permissions.is_empty() {
        crate::plugins::approve_registry_in(&p.approvals, name, &manifest.permissions);
    }
    set_enabled_locked(p, name, true)?;
    if let Some(old) = old.filter(|o| o.dir != dest) {
        let _ = std::fs::remove_dir_all(&old.dir);
    }
    Ok(())
}

/// Whether `permissions` are allowed for `name`: those not approved already
/// are approved now (`approve` says how), or it's an error.
fn check_approval(p: &Paths, name: &str, permissions: &[Permission], approve: &Approve) -> Result<(), String> {
    let allowed = crate::plugins::registry_approval_in(&p.approvals, name);
    let new: Vec<Permission> = permissions.iter().copied().filter(|x| !allowed.contains(x)).collect();
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

/// Approve what an installed plugin uses, asking for anything not approved.
fn approve_new(p: &Paths, name: &str, permissions: &[Permission], approve: &Approve) -> Result<(), String> {
    check_approval(p, name, permissions, approve)?;
    if !permissions.is_empty() {
        crate::plugins::approve_registry_in(&p.approvals, name, permissions);
    }
    Ok(())
}

/// Unpack a `.tar.gz` into `into` and return its one top-level folder.
/// Refuses archives with absolute paths, `..`, links or special files, so
/// nothing can be written outside `into`.
fn unpack(archive: &Path, into: &Path) -> Result<PathBuf, String> {
    let bad = |why: &str| format!("the download {why}; not installed");
    let tar = |flags: &str| {
        std::process::Command::new("tar").arg(flags).arg(archive).output().map_err(|e| format!("couldn't run tar: {e}"))
    };
    let (list, long) = (tar("-tzf")?, tar("-tvzf")?);
    if !list.status.success() || !long.status.success() {
        return Err(bad("isn't a .tar.gz archive"));
    }
    // Links are refused before anything is unpacked: not every tar refuses
    // to write through one by itself. GNU tar marks them `l` and `h`;
    // BSD tar shows hard links as "link to".
    for line in String::from_utf8_lossy(&long.stdout).lines() {
        if !(line.starts_with('-') || line.starts_with('d')) || line.contains(" link to ") || line.contains(" -> ") {
            return Err(bad("has a link or special file"));
        }
    }
    for entry in String::from_utf8_lossy(&list.stdout).lines() {
        let path = Path::new(entry);
        if path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(bad("has files outside its folder"));
        }
    }
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let out = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .args(["--no-same-owner", "--no-same-permissions"])
        .output()
        .map_err(|e| format!("couldn't run tar: {e}"))?;
    if !out.status.success() {
        return Err(bad(&format!("couldn't be unpacked ({})", String::from_utf8_lossy(&out.stderr).trim())));
    }
    // Checked again once unpacked, in case the listing missed one.
    fn check(dir: &Path) -> Result<(), String> {
        for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
            let kind = e.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                check(&e.path())?;
            } else if !kind.is_file() {
                return Err(format!("the download has a link or special file ({}); not installed", e.file_name().to_string_lossy()));
            }
        }
        Ok(())
    }
    check(into)?;
    let tops: Vec<PathBuf> = std::fs::read_dir(into).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    match tops.as_slice() {
        [one] if one.is_dir() => Ok(one.clone()),
        _ => Err(bad("should hold exactly one folder")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-registry-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(name: &str, stars: u64, versions: &[(&str, &str)]) -> Entry {
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
            plugins: vec![entry("a/git-tools", 5, &[("1.0.0", "1.0.0")]), docker, entry("b/git-more", 50, &[("1.0.0", "1.0.0")])],
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
    fn paths(name: &str) -> (PathBuf, Paths) {
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
    fn archive(dir: &Path, version: &str, perms: &str) -> String {
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

    fn release(version: &str, perms: Vec<Permission>, archive: String) -> Release {
        Release { version: version.into(), commit: "c".repeat(40), permissions: perms, lumen: "1.0.0".into(), archive }
    }

    fn index_with(releases: Vec<Release>) -> Index {
        let mut e = entry("mira/x", 1, &[]);
        e.versions = releases;
        Index { version: 1, plugins: vec![e] }
    }

    fn config(p: &Paths) -> String {
        std::fs::read_to_string(&p.config).unwrap_or_default()
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
        let approvals = (0..8).filter(|i| !crate::plugins::registry_approval_in(&p.approvals, &format!("mira/p{i}")).is_empty());
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
        let v2 = release("2.0.0", vec![Permission::Panes, Permission::Screen], archive(&dir, "2.0.0", "\"panes\", \"screen\""));
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
        assert_eq!(crate::plugins::registry_approval_in(&p.approvals, "mira/x"), vec![Permission::Panes, Permission::Screen]);
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
                &format!("{head}add('x/out', kind=tarfile.SYMTYPE, link=sys.argv[2])\nadd('x/out/evil-through')\nt.close()"),
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
    fn names_are_checked() {
        for good in ["mira/docker-panes", "a/b1", "lumenshell/snake"] {
            assert!(valid_name(good), "{good}");
        }
        let long = format!("a/{}", "b".repeat(40));
        for bad in ["mira", "Mira/x", "mira/-x", "mira/x-", "mira/x/y", "../x", "mira/", "/x", "mi ra/x", long.as_str()] {
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
            Installed { version: "1.0.0".into(), commit: "a".repeat(40), permissions: vec![Permission::Panes], dir: dir.join("x") },
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
