//! The socket API: scripts and plugins control Lumen over a Unix socket.
//!
//! Each Lumen process listens on `lumen-<pid>.sock` in a private per-user
//! directory, and tells its shells where with `LUMEN_SOCKET` (plus
//! `LUMEN_PANE`, the pane the shell runs in). The protocol is JSON-RPC 2.0,
//! one message per line. Requests are handled on the UI thread by
//! [`Workspace::handle_api`](crate::workspace::Workspace::handle_api);
//! `subscribe` turns a connection into an event stream, fed by [`emit`] from
//! anywhere (including the PTY threads).

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use futures::StreamExt;
use futures::channel::{mpsc as fmpsc, oneshot};
use gpui::{App, WindowHandle};
use serde_json::{Value, json};

use crate::plugins::Permission;
use crate::settings::Settings;
use crate::workspace::Workspace;

/// Bumped when a change could break existing clients.
pub const PROTOCOL: u32 = 1;

/// Longest request line accepted, so a stuck client can't eat memory.
const MAX_LINE: usize = 1 << 20;

/// Every event name, for `subscribe` and the docs.
pub const EVENTS: &[&str] = &[
    "pane-opened",
    "pane-closed",
    "focus-changed",
    "title-changed",
    "cwd-changed",
    "bell",
    "command-started",
    "command-finished",
];

/// A JSON-RPC error: code and message.
#[derive(Debug)]
pub struct ApiError(pub i64, pub String);

impl ApiError {
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self(-32602, msg.into())
    }
    pub fn failed(msg: impl Into<String>) -> Self {
        Self(-32000, msg.into())
    }
}

pub type ApiResult = Result<Value, ApiError>;

/// The socket this process listens on, once started.
static SOCKET: OnceLock<PathBuf> = OnceLock::new();

pub fn socket_path() -> Option<&'static Path> {
    SOCKET.get().map(PathBuf::as_path)
}

/// The private directory sockets live in: `$XDG_RUNTIME_DIR/lumen`, else
/// `$TMPDIR/lumen-<uid>` (or `/tmp`). Created 0700; an existing one must be a
/// real directory owned by us and closed to others, or it isn't used, so
/// another user can't plant or watch our socket.
pub fn socket_dir() -> Result<PathBuf, String> {
    let uid = unsafe { libc::getuid() };
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d).join("lumen"),
        None => {
            let tmp = std::env::var_os("TMPDIR").filter(|d| !d.is_empty()).unwrap_or_else(|| "/tmp".into());
            PathBuf::from(tmp).join(format!("lumen-{uid}"))
        }
    };
    if let Err(e) = std::fs::DirBuilder::new().mode(0o700).create(&dir)
        && e.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(format!("can't create {}: {e}", dir.display()));
    }
    let meta = std::fs::symlink_metadata(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(format!("{} isn't a private directory owned by you", dir.display()));
    }
    Ok(dir)
}

// ---- events ------------------------------------------------------------

struct Subscriber {
    /// Event names wanted; `None` means all.
    events: Option<HashSet<String>>,
    out: mpsc::Sender<String>,
    /// May see command text (plugins need the `screen` permission).
    screen: bool,
}

static SUBSCRIBERS: Mutex<Vec<Subscriber>> = Mutex::new(Vec::new());

/// Send an event to every subscriber that wants it. Cheap when nobody's
/// listening; safe to call from any thread.
pub fn emit(event: &str, mut params: Value) {
    let mut subs = SUBSCRIBERS.lock().unwrap();
    if subs.is_empty() {
        return;
    }
    if let Some(obj) = params.as_object_mut() {
        obj.insert("event".into(), event.into());
    }
    let line = json!({"jsonrpc": "2.0", "method": "event", "params": params}).to_string();
    // Without the `screen` permission, what was typed is left out.
    let redacted = params.get("command").is_some().then(|| {
        let mut p = params.clone();
        p["command"] = Value::Null;
        json!({"jsonrpc": "2.0", "method": "event", "params": p}).to_string()
    });
    // A failed send means the connection is gone.
    subs.retain(|s| {
        if s.events.as_ref().is_some_and(|e| !e.contains(event)) {
            return true;
        }
        let line = match (&redacted, s.screen) {
            (Some(r), false) => r.clone(),
            _ => line.clone(),
        };
        s.out.send(line).is_ok()
    });
}

// ---- server ------------------------------------------------------------

/// Who sent a request: a socket client (full access) or a plugin, limited to
/// the permissions it was granted.
#[derive(Clone, Debug)]
pub enum Caller {
    Socket,
    Plugin { name: String, perms: Vec<Permission> },
}

impl Caller {
    pub fn may(&self, p: Permission) -> bool {
        match self {
            Caller::Socket => true,
            Caller::Plugin { perms, .. } => perms.contains(&p),
        }
    }
}

/// The permission a method needs, if any.
fn required(method: &str) -> Option<Permission> {
    match method {
        "new-tab" | "split" | "focus" | "close" | "set-title" => Some(Permission::Panes),
        "send-text" | "run-action" => Some(Permission::Input),
        "read-screen" => Some(Permission::Screen),
        _ => None,
    }
}

/// A request for the UI thread.
pub struct Job {
    pub caller: Caller,
    pub method: String,
    pub params: Value,
    pub reply: oneshot::Sender<ApiResult>,
}

pub type Jobs = fmpsc::UnboundedSender<Job>;

static JOBS: OnceLock<Jobs> = OnceLock::new();

/// Where requests go once [`start`] has run.
pub fn jobs() -> Option<Jobs> {
    JOBS.get().cloned()
}

/// Run requests from the socket and plugins against `window`'s workspace,
/// on the UI thread.
pub fn start(window: WindowHandle<Workspace>, cx: &mut App) {
    let (tx, mut jobs) = fmpsc::unbounded::<Job>();
    let _ = JOBS.set(tx);
    cx.spawn(async move |cx| {
        while let Some(job) = jobs.next().await {
            let Job { caller, method, params, reply } = job;
            let reply = std::cell::Cell::new(Some(reply));
            let result = cx
                .update(|cx| {
                    if matches!(caller, Caller::Socket) && !Settings::get(cx).socket_api {
                        return Some(Err(ApiError::failed("the socket API is turned off in config.toml")));
                    }
                    window
                        .update(cx, |ws, window, cx| {
                            if method == "show-picker" {
                                // Answered when you pick something.
                                ws.show_picker(&params, reply.take().unwrap(), window, cx);
                                None
                            } else {
                                Some(ws.handle_api(&caller, &method, &params, window, cx))
                            }
                        })
                        .unwrap_or_else(|_| Some(Err(ApiError::failed("the window is closed"))))
                })
                .unwrap_or_else(|_| Some(Err(ApiError::failed("Lumen is quitting"))));
            if let (Some(result), Some(reply)) = (result, reply.take()) {
                let _ = reply.send(result);
            }
        }
    })
    .detach();
}

/// Open the socket, if enabled in the settings. Done before any pane starts,
/// so every shell gets `LUMEN_SOCKET`; requests wait until [`serve`].
pub fn listen(cx: &App) -> Option<UnixListener> {
    if !Settings::get(cx).socket_api {
        return None;
    }
    let path = match socket_dir() {
        Ok(dir) => dir.join(format!("lumen-{}.sock", std::process::id())),
        Err(e) => {
            eprintln!("lumen: socket API off: {e}");
            return None;
        }
    };
    // A socket left by a crashed process with our (recycled) pid.
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("lumen: socket API off: can't listen on {}: {e}", path.display());
            return None;
        }
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    let _ = SOCKET.set(path);
    Some(listener)
}

/// Accept socket connections. Needs [`start`] first.
pub fn serve(listener: UnixListener, cx: &mut App) {
    let Some(path) = socket_path().map(Path::to_path_buf) else { return };
    let Some(jobs) = jobs() else { return };
    std::thread::Builder::new()
        .name("lumen-api".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let jobs = jobs.clone();
                let _ = std::thread::Builder::new()
                    .name("lumen-api-conn".into())
                    .spawn(move || serve_connection(stream, jobs));
            }
        })
        .ok();

    cx.on_app_quit(move |_| {
        let _ = std::fs::remove_file(&path);
        async {}
    })
    .detach();
}

/// One socket connection.
fn serve_connection(stream: UnixStream, jobs: Jobs) {
    let Ok(write_half) = stream.try_clone() else { return };
    let out = writer(write_half);
    serve_lines(BufReader::new(stream), &out, &jobs, &Caller::Socket);
}

/// A thread writing lines to `w`; replies and events share it, so lines
/// never interleave. Ends when `w` can't be written.
pub fn writer(mut w: impl Write + Send + 'static) -> mpsc::Sender<String> {
    let (out, lines) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in lines {
            if w.write_all(line.as_bytes()).and_then(|_| w.write_all(b"\n")).and_then(|_| w.flush()).is_err() {
                break;
            }
        }
    });
    out
}

/// Read requests line by line from `reader` until it closes, answering on `out`.
pub fn serve_lines(mut reader: impl BufRead, out: &mpsc::Sender<String>, jobs: &Jobs, caller: &Caller) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match (&mut reader).take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.len() > MAX_LINE {
            let _ = out.send(error_response(Value::Null, ApiError(-32600, "request too long".into())));
            break;
        }
        let text = String::from_utf8_lossy(&buf);
        if text.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_line(&text, jobs, out, caller) {
            let _ = out.send(reply);
        }
    }
}

fn error_response(id: Value, e: ApiError) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": e.0, "message": e.1}}).to_string()
}

fn ok_response(id: Value, result: ApiResult) -> String {
    match result {
        Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}).to_string(),
        Err(e) => error_response(id, e),
    }
}

fn run_job(jobs: &Jobs, caller: &Caller, method: String, params: Value) -> ApiResult {
    let (reply, rx) = oneshot::channel();
    let job = Job { caller: caller.clone(), method, params, reply };
    if jobs.unbounded_send(job).is_err() {
        return Err(ApiError::failed("Lumen is quitting"));
    }
    futures::executor::block_on(rx).unwrap_or_else(|_| Err(ApiError::failed("Lumen is quitting")))
}

/// Handle one request line; the reply, or `None` for a notification (or a
/// reply that comes later).
fn handle_line(text: &str, jobs: &Jobs, out: &mpsc::Sender<String>, caller: &Caller) -> Option<String> {
    let msg: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return Some(error_response(Value::Null, ApiError(-32700, format!("not valid JSON: {e}")))),
    };
    let id = msg.get("id").cloned();
    let reply_id = id.clone().unwrap_or(Value::Null);
    let method = match (msg.get("jsonrpc").and_then(Value::as_str), msg.get("method").and_then(Value::as_str)) {
        (Some("2.0"), Some(m)) => m.to_string(),
        _ => {
            return Some(error_response(
                reply_id,
                ApiError(-32600, "expected {\"jsonrpc\": \"2.0\", \"method\": ..., \"id\": ...}".into()),
            ));
        }
    };
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

    if let (Caller::Plugin { name, .. }, Some(p)) = (caller, required(&method))
        && !caller.may(p)
    {
        let e = ApiError(
            -32001,
            format!("plugin \"{name}\" needs the \"{}\" permission for {method}; add it to its plugin.toml", p.name()),
        );
        return id.map(|id| error_response(id, e));
    }

    let result = if method == "subscribe" {
        subscribe(&params, out, caller.may(Permission::Screen))
    } else if method == "show-picker" {
        // Waits for you to choose, so don't hold up this connection's other
        // requests: answer from another thread.
        let (jobs, out, caller) = (jobs.clone(), out.clone(), caller.clone());
        std::thread::spawn(move || {
            let result = run_job(&jobs, &caller, method, params);
            if let Some(id) = id {
                let _ = out.send(ok_response(id, result));
            }
        });
        return None;
    } else if method == "notify" {
        // Runs a helper program, so keep it off the UI thread.
        match params.get("title").and_then(Value::as_str) {
            Some(title) => {
                let body = params.get("body").and_then(Value::as_str).unwrap_or("");
                notify(title, body).map(|_| json!({})).map_err(ApiError::failed)
            }
            None => Err(ApiError::invalid_params("`title` is required")),
        }
    } else {
        run_job(jobs, caller, method, params)
    };
    Some(ok_response(id?, result))
}

fn subscribe(params: &Value, out: &mpsc::Sender<String>, screen: bool) -> ApiResult {
    let events = match params.get("events") {
        None | Some(Value::Null) => None,
        Some(Value::Array(names)) => {
            let mut set = HashSet::new();
            for n in names {
                let n = n.as_str().ok_or_else(|| ApiError::invalid_params("`events` should be a list of names"))?;
                if !EVENTS.contains(&n) {
                    return Err(ApiError::invalid_params(format!(
                        "unknown event \"{n}\"; events are {}",
                        EVENTS.join(", ")
                    )));
                }
                set.insert(n.to_string());
            }
            Some(set)
        }
        Some(_) => return Err(ApiError::invalid_params("`events` should be a list of names")),
    };
    let names: Vec<&str> = match &events {
        Some(set) => EVENTS.iter().copied().filter(|e| set.contains(*e)).collect(),
        None => EVENTS.to_vec(),
    };
    SUBSCRIBERS.lock().unwrap().push(Subscriber { events, out: out.clone(), screen });
    Ok(json!({ "events": names }))
}

/// Show a desktop notification (`notify-send` on Linux, Notification Center
/// on macOS). Text goes as arguments, never through a shell.
pub fn notify(title: &str, body: &str) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("osascript");
        c.args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            title,
            body,
        ]);
        c
    } else {
        let mut c = std::process::Command::new("notify-send");
        c.args(["--app-name=Lumen", "--", title, body]);
        c
    };
    let status = cmd.status().map_err(|e| {
        if cfg!(target_os = "macos") {
            format!("couldn't run osascript: {e}")
        } else {
            format!("couldn't run notify-send ({e}); install libnotify-bin")
        }
    })?;
    if status.success() { Ok(()) } else { Err(format!("the notifier exited with {status}")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> (Option<Value>, mpsc::Receiver<String>) {
        let (jobs, _rx) = fmpsc::unbounded();
        let (out, events) = mpsc::channel();
        (handle_line(text, &jobs, &out, &Caller::Socket).map(|r| serde_json::from_str(&r).unwrap()), events)
    }

    #[test]
    fn bad_requests_get_json_rpc_errors() {
        assert_eq!(line("{oops").0.unwrap()["error"]["code"], -32700);
        assert_eq!(line(r#"{"method":"list","id":1}"#).0.unwrap()["error"]["code"], -32600);
        let bad = line(r#"{"jsonrpc":"2.0","method":"subscribe","params":{"events":["nope"]},"id":7}"#).0.unwrap();
        assert_eq!((bad["id"].clone(), bad["error"]["code"].clone()), (json!(7), json!(-32602)));
    }

    #[test]
    fn subscribers_get_only_the_events_they_asked_for() {
        let (reply, events) = line(r#"{"jsonrpc":"2.0","method":"subscribe","params":{"events":["bell"]},"id":1}"#);
        assert_eq!(reply.unwrap()["result"]["events"], json!(["bell"]));
        emit("pane-opened", json!({"pane": 1}));
        emit("bell", json!({"pane": 2}));
        let got: Value = serde_json::from_str(&events.try_recv().unwrap()).unwrap();
        assert_eq!(got["params"], json!({"event": "bell", "pane": 2}));
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn plugins_are_held_to_their_permissions() {
        let (jobs, _rx) = fmpsc::unbounded();
        let (out, events) = mpsc::channel();
        let plugin = Caller::Plugin { name: "p".into(), perms: vec![Permission::Panes] };
        let ask = |text: &str| -> Value {
            serde_json::from_str(&handle_line(text, &jobs, &out, &plugin).unwrap()).unwrap()
        };
        let denied = ask(r#"{"jsonrpc":"2.0","id":1,"method":"send-text","params":{"text":"rm -rf ~"}}"#);
        assert_eq!(denied["error"]["code"], -32001);
        assert!(denied["error"]["message"].as_str().unwrap().contains("\"input\" permission"));
        assert_eq!(ask(r#"{"jsonrpc":"2.0","id":2,"method":"read-screen"}"#)["error"]["code"], -32001);

        // Without `screen`, command text in events is left out.
        let sub = ask(r#"{"jsonrpc":"2.0","id":3,"method":"subscribe","params":{"events":["command-finished"]}}"#);
        assert!(sub.get("result").is_some());
        emit("command-finished", json!({"pane": 9, "command": "export TOKEN=secret", "exit": 0}));
        let got: Value = serde_json::from_str(&events.try_recv().unwrap()).unwrap();
        assert_eq!(got["params"]["command"], Value::Null);
        assert_eq!(got["params"]["exit"], 0);
    }

    #[test]
    fn socket_dir_is_private() {
        let dir = socket_dir().unwrap();
        let meta = std::fs::symlink_metadata(&dir).unwrap();
        assert_eq!(meta.mode() & 0o077, 0);
    }
}
