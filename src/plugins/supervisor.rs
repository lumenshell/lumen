//! Running plugins: starting each one as a process, restarting it when it
//! exits, and stopping it. Also what a running plugin has: its settings and
//! its palette commands.

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
    fn registering_commands_checks_fields() {
        let caller = Caller::Plugin { name: "nobody-running".into(), perms: vec![] };
        assert!(register_command(&caller, &json!({"title": "x"})).is_err());
        assert!(register_command(&caller, &json!({"id": "a", "title": "A", "keys": "ctrl-shfit-x"})).is_err());
        assert!(register_command(&caller, &json!({"id": "a", "title": "A", "keys": "ctrl-alt-g"})).is_ok());
        assert!(register_command(&Caller::Socket, &json!({"id": "a", "title": "A"})).is_err());
    }
}
