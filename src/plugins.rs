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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
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
    pub dir: PathBuf,
}

/// Where plugins are installed.
pub fn plugins_dir() -> Option<PathBuf> {
    crate::settings::config_path().and_then(|p| Some(p.parent()?.join("plugins")))
}

fn state_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home?.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".local/state")))?
    };
    Some(base.join("lumen"))
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
        dir: dir.to_path_buf(),
    })
}

/// Every plugin folder with a `plugin.toml`, and problems reading them.
pub fn discover() -> (Vec<Manifest>, Vec<String>) {
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

fn approvals_path() -> Option<PathBuf> {
    state_dir().map(|d| d.join("plugin-approvals.json"))
}

fn load_approvals() -> BTreeMap<String, Approval> {
    approvals_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Decision {
    Allowed,
    Refused,
    /// Never asked, or the manifest changed since.
    Ask,
}

pub fn decision(m: &Manifest) -> Decision {
    if m.permissions.is_empty() {
        return Decision::Allowed;
    }
    match load_approvals().get(&m.name) {
        Some(a) if a.command == m.command && m.permissions.iter().all(|p| a.permissions.contains(p)) => {
            if a.allowed { Decision::Allowed } else { Decision::Refused }
        }
        _ => Decision::Ask,
    }
}

pub fn record_decision(m: &Manifest, allowed: bool) {
    let Some(path) = approvals_path() else { return };
    let mut all = load_approvals();
    all.insert(m.name.clone(), Approval { command: m.command.clone(), permissions: m.permissions.clone(), allowed });
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&all).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
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
    /// Which start this is, so a copy still shutting down after a reload
    /// can't touch its replacement.
    generation: u64,
}

static PLUGINS: Mutex<Vec<Running>> = Mutex::new(Vec::new());
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// The entry for this particular start of a plugin.
fn with_run<R>(name: &str, generation: u64, f: impl FnOnce(&mut Running) -> R) -> Option<R> {
    PLUGINS
        .lock()
        .unwrap()
        .iter_mut()
        .find(|r| r.manifest.name == name && r.generation == generation)
        .map(f)
}

/// A plugin as shown in the palette.
pub struct Info {
    pub manifest: Manifest,
    pub status: String,
    pub commands: Vec<PluginCommand>,
}

pub fn list() -> Vec<Info> {
    PLUGINS
        .lock()
        .unwrap()
        .iter()
        .map(|r| Info { manifest: r.manifest.clone(), status: r.status.clone(), commands: r.commands.clone() })
        .collect()
}

fn with_plugin<R>(name: &str, f: impl FnOnce(&mut Running) -> R) -> Option<R> {
    PLUGINS.lock().unwrap().iter_mut().find(|r| r.manifest.name == name).map(f)
}

/// Note a plugin that isn't running (refused, or waiting for approval), so
/// it still shows in the palette.
pub fn add_stopped(m: Manifest, status: &str) {
    let mut all = PLUGINS.lock().unwrap();
    all.retain(|r| r.manifest.name != m.name);
    all.push(Running {
        manifest: m,
        status: status.into(),
        out: None,
        commands: Vec::new(),
        stop: Arc::new(AtomicBool::new(true)),
        pid: None,
        generation: 0,
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
pub fn start(m: Manifest) {
    let Some(jobs) = api::jobs() else { return };
    let stop = Arc::new(AtomicBool::new(false));
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst);
    {
        let mut all = PLUGINS.lock().unwrap();
        if let Some(old) = all.iter().find(|r| r.manifest.name == m.name) {
            old.stop.store(true, Ordering::SeqCst);
        }
        all.retain(|r| r.manifest.name != m.name);
        all.push(Running {
            manifest: m.clone(),
            status: "starting".into(),
            out: None,
            commands: Vec::new(),
            stop: stop.clone(),
            pid: None,
            generation,
        });
    }
    let name = m.name.clone();
    let _ = std::thread::Builder::new()
        .name(format!("plugin-{name}"))
        .spawn(move || supervise(m, stop, jobs, generation));
}

const BACKOFF: [u64; 6] = [1, 2, 5, 10, 30, 60];

fn supervise(m: Manifest, stop: Arc<AtomicBool>, jobs: api::Jobs, generation: u64) {
    let name = m.name.clone();
    let caller = Caller::Plugin { name: name.clone(), perms: m.permissions.clone() };
    let mut quick_failures = 0usize;
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        let mut child = match Command::new(&m.command[0])
            .args(&m.command[1..])
            .current_dir(&m.dir)
            .env_remove("LUMEN_SOCKET")
            .env_remove("LUMEN_PANE")
            .env("LUMEN_PLUGIN", &name)
            .env("LUMEN_PLUGIN_DIR", &m.dir)
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
        let out = api::writer(child.stdin.take().expect("piped"));
        with_run(&name, generation, |r| {
            r.out = Some(out.clone());
            r.pid = Some(pid);
            r.status = "running".into();
        });
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
        api::serve_lines(stdout, &out, &jobs, &caller);
        drop(out);

        let status = child.wait().map(|s| s.to_string()).unwrap_or_else(|e| e.to_string());
        // Its commands and key bindings go with it.
        let had_commands = with_run(&name, generation, |r| {
            r.out = None;
            r.pid = None;
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

/// Stop every plugin: they're asked to exit (SIGTERM) and not restarted.
pub fn stop_all() {
    let all = PLUGINS.lock().unwrap();
    for r in all.iter() {
        r.stop.store(true, Ordering::SeqCst);
        if let Some(pid) = r.pid {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
    }
}

/// Forget every plugin (after [`stop_all`]), before loading them again.
pub fn clear() {
    PLUGINS.lock().unwrap().clear();
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
        .flat_map(|r| {
            r.commands.iter().filter_map(|c| Some((c.keys.clone()?, r.manifest.name.clone(), c.id.clone())))
        })
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
        assert!(parse_manifest(dir, "command = \"x\"\npermissions = [\"network\"]\n").unwrap_err().contains("unknown permission"));
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
