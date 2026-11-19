//! Running plugins: starting each one as a process, restarting it when it
//! exits, and stopping it. Also what a running plugin has: its settings and
//! its palette commands.

use std::io::Read;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout};

use super::*;

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
    /// Its connection and process, while [`supervise`] is running it.
    live: Option<Live>,
    /// Which start this is, so a copy still shutting down after a reload
    /// can't touch its replacement. 0 for one that was never started.
    generation: u64,
    /// Its settings now, defaults filled in: given to every (re)start.
    settings: serde_json::Map<String, Value>,
}

impl Running {
    /// An entry with no process yet.
    fn new(manifest: Manifest, status: &str, generation: u64, settings: serde_json::Map<String, Value>) -> Running {
        Running {
            manifest,
            status: status.into(),
            out: None,
            commands: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
            live: None,
            generation,
            settings,
        }
    }
}

/// A running plugin's connection and process, for stopping it.
#[derive(Clone)]
struct Live {
    /// Its connection's id, as [`api::disconnect`] takes it.
    conn: u64,
    process: Process,
}

/// How long a stopped plugin has to exit before it's killed.
const STOP_GRACE: Duration = Duration::from_millis(if cfg!(test) { 300 } else { 2000 });

/// How long plugins have to exit when Lumen quits before they're killed.
/// Shorter than [`STOP_GRACE`], because quitting waits for it.
const QUIT_GRACE: Duration = Duration::from_millis(if cfg!(test) { 300 } else { 500 });

/// The longest line of a plugin's stderr kept whole in its log.
const LOG_LINE: usize = 16 * 1024;

static PLUGINS: Mutex<Vec<Running>> = Mutex::new(Vec::new());
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// A plugin's process and whatever it starts: it leads a process group of
/// its own, so a signal reaches all of them.
///
/// Until the process has been waited for, its pid and its group's id can't
/// have gone to anything else. Signals are only sent before then, with the
/// lock held, so they can't reach a stranger.
#[derive(Clone)]
struct Process {
    pid: u32,
    /// The child, until it has been waited for.
    child: Arc<Mutex<Option<Child>>>,
}

/// The ends of a plugin's pipes that Lumen holds.
struct Pipes {
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
}

impl Process {
    /// Start `cmd` in a process group of its own, with pipes for its
    /// stdin, stdout and stderr.
    fn spawn(cmd: &mut Command) -> std::io::Result<(Process, Pipes)> {
        let mut child =
            cmd.process_group(0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let pipes = Pipes {
            stdin: child.stdin.take().expect("piped"),
            stdout: child.stdout.take().expect("piped"),
            stderr: child.stderr.take().expect("piped"),
        };
        Ok((Process { pid: child.id(), child: Arc::new(Mutex::new(Some(child))) }, pipes))
    }

    /// Send `sig` to it and whatever it started, unless it has been waited for.
    fn signal(&self, sig: i32) {
        let child = self.child.lock().unwrap();
        if child.is_some() {
            signal_group(self.pid, sig);
        }
    }

    /// Ask it and whatever it started to exit (SIGTERM), and kill what's
    /// still there after `grace`.
    fn stop(&self, grace: Duration) {
        self.signal(libc::SIGTERM);
        let this = self.clone();
        let _ = std::thread::Builder::new().name("plugin-stop".into()).spawn(move || {
            std::thread::sleep(grace);
            this.signal(libc::SIGKILL);
        });
    }

    /// Whether it has exited. What it started may still be running.
    fn has_exited(&self) -> bool {
        let child = self.child.lock().unwrap();
        child.is_none() || exited(self.pid, false)
    }

    /// Wait for it to exit and say how it went. If `and_the_rest` then says
    /// so, whatever it started and left behind is killed.
    fn wait(&self, and_the_rest: impl FnOnce() -> bool) -> String {
        exited(self.pid, true);
        let mut child = self.child.lock().unwrap();
        if and_the_rest() {
            signal_group(self.pid, libc::SIGKILL);
        }
        match child.take() {
            Some(mut child) => child.wait().map(|s| s.to_string()).unwrap_or_else(|e| e.to_string()),
            None => "already waited for".into(),
        }
    }
}

/// Send `sig` to the process group `pid` leads, or to the process alone if
/// it leads none.
fn signal_group(pid: u32, sig: i32) {
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

/// Whether our child `pid` has exited, waiting until it has if `block`. It
/// isn't reaped: until it is, the pid and its process group stay its own.
fn exited(pid: u32, block: bool) -> bool {
    let flags = libc::WEXITED | libc::WNOWAIT | if block { 0 } else { libc::WNOHANG };
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        loop {
            if libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, flags) == 0 {
                // Left as zeros when there's nothing to report yet.
                return info.si_signo != 0;
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                // Not a child of ours (any more): nothing to wait for.
                return true;
            }
        }
    }
}

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

/// Put `new` in place of any entry with its name, stopping what that one
/// was running: a new version, or new permissions, and the old process goes.
fn replace(new: Running) {
    let mut all = PLUGINS.lock().unwrap();
    all.iter().filter(|r| r.manifest.name == new.manifest.name).for_each(end);
    all.retain(|r| r.manifest.name != new.manifest.name);
    all.push(new);
}

/// Note a plugin that isn't running (refused, or waiting for approval), so
/// it still shows in the palette.
pub fn add_stopped(m: Manifest, status: &str, settings: serde_json::Map<String, Value>) {
    replace(Running { stop: Arc::new(AtomicBool::new(true)), ..Running::new(m, status, 0, settings) });
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

/// Give `each` every line of `reader` until it closes. A line longer than
/// [`LOG_LINE`] comes in pieces, so one that never ends can't use up memory,
/// and bytes that aren't UTF-8 are replaced instead of ending the reading.
fn log_lines(reader: impl Read, mut each: impl FnMut(&str)) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match (&mut reader).take(LOG_LINE as u64).read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => each(String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r'])),
        }
    }
}

/// Start `m` and keep it running: restart it when it exits, waiting longer
/// after each quick crash, until it's stopped.
pub fn start(m: Manifest, settings: serde_json::Map<String, Value>) {
    let Some(jobs) = api::jobs() else { return };
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst);
    let entry = Running::new(m.clone(), "starting", generation, settings);
    let stop = entry.stop.clone();
    replace(entry);
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
        cmd.args(&m.command[1..])
            .current_dir(&m.dir)
            .env_remove("LUMEN_SOCKET")
            .env_remove("LUMEN_PANE")
            .env("LUMEN_PLUGIN", &name)
            .env("LUMEN_PLUGIN_DIR", &m.dir)
            .envs(data_dir(&name).map(|d| ("LUMEN_PLUGIN_DATA", d)))
            // Read now, not at the first start: they may have changed since.
            .env("LUMEN_PLUGIN_SETTINGS", Value::Object(started_with.clone()).to_string());
        let (process, pipes) = match Process::spawn(&mut cmd) {
            Ok(started) => started,
            Err(e) => {
                let msg = format!("couldn't start `{}`: {e}", m.command.join(" "));
                log(&name, &format!("{} [lumen] {msg}", now()));
                with_run(&name, generation, |r| r.status = format!("failed: {msg}"));
                // Retrying won't help until the manifest or PATH changes.
                return;
            }
        };
        // Stopping the plugin closes its stdin and ends its replies and events.
        let out = api::writer_until(pipes.stdin, Some(stop.clone()));
        let live = Live { conn: api::new_conn(), process: process.clone() };
        attach(&name, generation, out.clone(), live.clone(), &started_with);
        log(&name, &format!("{} [lumen] started `{}` (pid {})", now(), m.command.join(" "), process.pid));
        if stop.load(Ordering::SeqCst) {
            // Told to stop while starting: don't leave it behind.
            process.signal(libc::SIGKILL);
        }

        let (stderr, err_name) = (pipes.stderr, name.clone());
        std::thread::spawn(move || log_lines(stderr, |line| log(&err_name, &format!("{} {line}", now()))));
        // Nothing it asks for after it's told to stop is carried out.
        api::serve_conn(BufReader::new(pipes.stdout), live.conn, &out, &jobs, &caller, Some(&stop));
        drop(out);

        // Once it's been told to stop, whatever it started goes with it.
        let status = process.wait(|| stop.load(Ordering::SeqCst));
        // Its commands and key bindings go with it.
        let had_commands = with_run(&name, generation, |r| {
            r.out = None;
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

/// Stop a plugin's process and don't restart it. At once, it loses its
/// connection: nothing more it asks for is carried out, it's sent no more
/// events, and its panes and styles go. It and everything it started are
/// asked to exit (SIGTERM), and killed if still there after [`STOP_GRACE`].
fn end(r: &Running) {
    r.stop.store(true, Ordering::SeqCst);
    let Some(live) = &r.live else { return };
    if let Some(jobs) = api::jobs() {
        let caller = Caller::Plugin { name: r.manifest.name.clone(), perms: r.manifest.permissions.clone() };
        api::disconnect(&jobs, &caller, live.conn);
    }
    live.process.stop(STOP_GRACE);
}

/// Stop every plugin as Lumen quits, and return once they're gone. They
/// get [`QUIT_GRACE`] to exit; then they and whatever they started are
/// killed, so nothing outlives Lumen by ignoring being asked.
pub fn shut_down() {
    let processes: Vec<Process> = {
        let all = PLUGINS.lock().unwrap();
        all.iter().for_each(end);
        all.iter().filter_map(|r| r.live.as_ref().map(|live| live.process.clone())).collect()
    };
    finish(&processes, QUIT_GRACE);
}

/// Wait up to `grace` for `processes` to exit, then kill any still there,
/// and whatever those that exited left behind.
fn finish(processes: &[Process], grace: Duration) {
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline && !processes.iter().all(Process::has_exited) {
        std::thread::sleep(Duration::from_millis(10));
    }
    for process in processes {
        process.signal(libc::SIGKILL);
    }
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
    live: Live,
    started_with: &serde_json::Map<String, Value>,
) {
    with_run(name, generation, |r| {
        if &r.settings != started_with {
            let _ = out.send(settings_message(&r.settings));
        }
        r.out = Some(out);
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
        PLUGINS.lock().unwrap().push(Running { out: Some(tx), ..Running::new(m, "running", 0, settings) });
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
        PLUGINS.lock().unwrap().push(Running::new(m, "starting", 7, map(json!({"n": 1}))));
        assert!(update_settings("t-starting", map(json!({"n": 2}))));
        // Already waited for, so nothing is ever signalled.
        let live = Live { conn: 0, process: Process { pid: 1, child: Arc::new(Mutex::new(None)) } };
        attach("t-starting", 7, tx.clone(), live.clone(), &map(json!({"n": 1})));
        let msg: Value = serde_json::from_str(&rx.try_recv().expect("the newer values are sent")).unwrap();
        assert_eq!(msg["params"]["settings"], json!({"n": 2}));
        attach("t-starting", 7, tx, live, &map(json!({"n": 2})));
        assert!(rx.try_recv().is_err(), "nothing when it started with them");
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

        let entry = Running::new(m.clone(), "starting", 4242, serde_json::Map::new());
        let stop = entry.stop.clone();
        PLUGINS.lock().unwrap().push(entry);
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
        let (process, _pipes) = Process::spawn(std::process::Command::new("sleep").arg("30")).unwrap();
        let m = parse_manifest(Path::new("t-replace"), "command = \"x\"").unwrap();
        PLUGINS.lock().unwrap().push(Running {
            live: Some(Live { conn: api::new_conn(), process: process.clone() }),
            ..Running::new(m.clone(), "running", 9, serde_json::Map::new())
        });
        add_stopped(m, "not allowed", serde_json::Map::new());
        let deadline = Instant::now() + Duration::from_secs(2);
        while !process.has_exited() {
            assert!(Instant::now() < deadline, "the old process is still running");
            std::thread::sleep(Duration::from_millis(20));
        }
        process.wait(|| false);
    }

    #[test]
    fn quitting_waits_for_plugins_then_kills_them() {
        use std::io::BufRead;
        // One that exits when asked: quitting doesn't wait the whole time.
        let (polite, _pipes) = Process::spawn(std::process::Command::new("sleep").arg("30")).unwrap();
        polite.signal(libc::SIGTERM);
        let began = Instant::now();
        finish(std::slice::from_ref(&polite), Duration::from_secs(5));
        assert!(began.elapsed() < Duration::from_secs(4), "waited though it had exited");
        polite.wait(|| false);

        // One that ignores being asked, as does the `sleep` it starts.
        let script = "trap '' TERM; sleep 60 & echo $!; wait";
        let (stubborn, pipes) = Process::spawn(std::process::Command::new("sh").args(["-c", script])).unwrap();
        let mut line = String::new();
        BufReader::new(pipes.stdout).read_line(&mut line).unwrap();
        let child: i32 = line.trim().parse().unwrap();
        stubborn.signal(libc::SIGTERM);
        let began = Instant::now();
        finish(std::slice::from_ref(&stubborn), QUIT_GRACE);
        assert!(began.elapsed() >= QUIT_GRACE, "killed before its time was up");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !stubborn.has_exited() || alive(child) {
            assert!(Instant::now() < deadline, "it or its child outlived quitting");
            std::thread::sleep(Duration::from_millis(10));
        }
        stubborn.wait(|| false);
        // Waited for, so nothing more is signalled: its pid may be someone else's now.
        assert!(stubborn.child.lock().unwrap().is_none());
    }

    #[test]
    fn long_and_odd_stderr_lines_are_logged_in_pieces() {
        let mut input = b"first\r\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', LOG_LINE * 2 + 100));
        input.extend(b"\n\xff\xfe not utf-8\nlast, with no end");
        let mut lines = Vec::new();
        log_lines(std::io::Cursor::new(input), |line| lines.push(line.to_string()));
        assert_eq!(lines.len(), 6, "{:?}", lines.iter().map(String::len).collect::<Vec<_>>());
        assert_eq!(lines[0], "first");
        assert!(lines.iter().all(|l| l.len() <= LOG_LINE));
        assert_eq!(lines[1..4].iter().map(String::len).sum::<usize>(), LOG_LINE * 2 + 100, "nothing is lost");
        assert_eq!(lines[4], "\u{fffd}\u{fffd} not utf-8");
        assert_eq!(lines[5], "last, with no end");
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
