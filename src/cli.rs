//! `lumen msg`: control a running Lumen from the command line, over the
//! socket API (see `api.rs`). Run inside a Lumen pane it talks to that
//! Lumen and acts on that pane by default.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::api;

const HELP: &str = "\
Control a running Lumen.

Usage: lumen msg <command> [options]

Panes are numbered ids (see `list`). Inside a Lumen pane, commands act on
that pane unless --pane says otherwise; elsewhere, on the focused pane.

Commands:
  list [--json]                       Tabs and panes, with ids, titles and directories
  new-tab [--cwd DIR] [--run CMD]     Open a tab; prints the new pane's id
  split [right|down] [--pane ID] [--cwd DIR] [--run CMD] [--no-focus]
                                      Split a pane; prints the new pane's id
  focus [ID]                          Show and focus a pane
  close [ID]                          Close a pane
  send-text [--pane ID] TEXT          Type TEXT into a pane (\\n, \\r, \\t, \\e and \\\\ work)
  read-screen [--pane ID] [--lines N] Print the screen, or the last N lines with history
  set-title [--pane ID] [TITLE]       Set the pane's tab title; no TITLE goes back to normal
  notify TITLE [BODY]                 Show a desktop notification
  action NAME [--pane ID]             Run a key-binding action, e.g. split-right or find
  watch [EVENT...]                    Print events as JSON lines until interrupted
  call METHOD [JSON]                  Send any request and print the raw result
  help                                Show this

--run types a command into the new pane's shell, as if you'd typed it.
Events: pane-opened, pane-closed, focus-changed, title-changed, cwd-changed,
bell, command-started, command-finished (the last two need shell integration).
";

/// Run `lumen msg` with `args` (after `msg`); the process exit code.
pub fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("lumen msg: {e}");
            1
        }
    }
}

/// Parsed `--flag value` options and positional arguments.
struct Args {
    positional: Vec<String>,
    options: Map<String, Value>,
}

/// Split `args` into options and positionals. `value_flags` take a value;
/// `bool_flags` don't. Anything after `--` is positional.
fn parse(args: &[String], value_flags: &[&str], bool_flags: &[&str]) -> Result<Args, String> {
    let mut out = Args { positional: Vec::new(), options: Map::new() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            out.positional.extend(it.by_ref().cloned());
            break;
        }
        if let Some(flag) = a.strip_prefix("--") {
            let (name, inline) = match flag.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (flag, None),
            };
            if value_flags.contains(&name) {
                let v = inline.or_else(|| it.next().cloned()).ok_or_else(|| format!("--{name} needs a value"))?;
                out.options.insert(name.into(), v.into());
            } else if bool_flags.contains(&name) && inline.is_none() {
                out.options.insert(name.into(), true.into());
            } else {
                return Err(format!("unknown option --{name}; see `lumen msg help`"));
            }
        } else {
            out.positional.push(a.clone());
        }
    }
    Ok(out)
}

/// `--pane` from the options, else `$LUMEN_PANE` (the pane we're running in).
fn pane(args: &Args) -> Result<Value, String> {
    let given = args.options.get("pane").and_then(Value::as_str).map(str::to_string);
    let from_env = std::env::var("LUMEN_PANE").ok();
    match given.or(from_env) {
        Some(p) => p.trim().parse::<u64>().map(Value::from).map_err(|_| format!("`{p}` isn't a pane id")),
        None => Ok(Value::Null),
    }
}

/// `\n`, `\r`, `\t`, `\e` and `\\` in `s`, so `send-text 'make\n'` presses Enter.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('e') => out.push('\x1b'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn at_most(args: &Args, n: usize) -> Result<(), String> {
    if args.positional.len() > n {
        return Err(format!("unexpected `{}`; see `lumen msg help`", args.positional[n]));
    }
    Ok(())
}

fn run_inner(args: &[String]) -> Result<(), String> {
    let Some((cmd, rest)) = args.split_first() else {
        print!("{HELP}");
        return Ok(());
    };
    match cmd.as_str() {
        "help" | "-h" | "--help" => {
            print!("{HELP}");
            Ok(())
        }
        "list" => {
            let a = parse(rest, &[], &["json"])?;
            at_most(&a, 0)?;
            let result = Client::connect()?.call("list", json!({}))?;
            if a.options.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
            } else {
                print!("{}", list_text(&result));
            }
            Ok(())
        }
        "new-tab" => {
            let a = parse(rest, &["cwd", "run"], &[])?;
            at_most(&a, 0)?;
            let mut params = a.options.clone();
            absolute_cwd(&mut params)?;
            let r = Client::connect()?.call("new-tab", Value::Object(params))?;
            println!("{}", r["pane"]);
            Ok(())
        }
        "split" => {
            let a = parse(rest, &["pane", "cwd", "run"], &["no-focus"])?;
            at_most(&a, 1)?;
            let direction = a.positional.first().map(String::as_str).unwrap_or("right");
            let mut params = Map::new();
            params.insert("direction".into(), direction.into());
            params.insert("pane".into(), pane(&a)?);
            for key in ["cwd", "run"] {
                if let Some(v) = a.options.get(key) {
                    params.insert(key.into(), v.clone());
                }
            }
            if a.options.contains_key("no-focus") {
                params.insert("focus".into(), false.into());
            }
            absolute_cwd(&mut params)?;
            let r = Client::connect()?.call("split", Value::Object(params))?;
            println!("{}", r["pane"]);
            Ok(())
        }
        "focus" | "close" => {
            let a = parse(rest, &[], &[])?;
            at_most(&a, 1)?;
            let pane = match a.positional.first() {
                Some(p) => p.parse::<u64>().map(Value::from).map_err(|_| format!("`{p}` isn't a pane id"))?,
                None => pane(&a)?,
            };
            Client::connect()?.call(cmd, json!({ "pane": pane }))?;
            Ok(())
        }
        "send-text" => {
            let a = parse(rest, &["pane"], &[])?;
            at_most(&a, 1)?;
            let text = a.positional.first().ok_or("send-text needs the TEXT to type")?;
            Client::connect()?.call("send-text", json!({ "pane": pane(&a)?, "text": unescape(text) }))?;
            Ok(())
        }
        "read-screen" => {
            let a = parse(rest, &["pane", "lines"], &[])?;
            at_most(&a, 0)?;
            let lines = match a.options.get("lines").and_then(Value::as_str) {
                Some(n) => Value::from(n.parse::<u64>().map_err(|_| format!("`{n}` isn't a number of lines"))?),
                None => Value::Null,
            };
            let r = Client::connect()?.call("read-screen", json!({ "pane": pane(&a)?, "lines": lines }))?;
            println!("{}", r["text"].as_str().unwrap_or_default());
            Ok(())
        }
        "set-title" => {
            let a = parse(rest, &["pane"], &[])?;
            let title = (!a.positional.is_empty()).then(|| a.positional.join(" "));
            Client::connect()?.call("set-title", json!({ "pane": pane(&a)?, "title": title }))?;
            Ok(())
        }
        "notify" => {
            let a = parse(rest, &[], &[])?;
            at_most(&a, 2)?;
            let title = a.positional.first().ok_or("notify needs a TITLE")?;
            let body = a.positional.get(1).cloned().unwrap_or_default();
            Client::connect()?.call("notify", json!({ "title": title, "body": body }))?;
            Ok(())
        }
        "action" => {
            let a = parse(rest, &["pane"], &[])?;
            at_most(&a, 1)?;
            let name = a.positional.first().ok_or("action needs a NAME, such as split-right")?;
            let pane = match a.options.get("pane") {
                Some(_) => pane(&a)?,
                None => Value::Null,
            };
            Client::connect()?.call("run-action", json!({ "action": name, "pane": pane }))?;
            Ok(())
        }
        "watch" => {
            let a = parse(rest, &[], &[])?;
            let events = if a.positional.is_empty() { Value::Null } else { json!(a.positional) };
            let mut client = Client::connect()?;
            client.call("subscribe", json!({ "events": events }))?;
            let stdout = std::io::stdout();
            loop {
                let msg = client.read()?;
                if msg.get("method").and_then(Value::as_str) == Some("event") {
                    let mut out = stdout.lock();
                    if writeln!(out, "{}", msg["params"]).and_then(|_| out.flush()).is_err() {
                        return Ok(()); // stdout closed, e.g. piped into `head`
                    }
                }
            }
        }
        "call" => {
            let a = parse(rest, &[], &[])?;
            at_most(&a, 2)?;
            let method = a.positional.first().ok_or("call needs a METHOD")?;
            let params = match a.positional.get(1) {
                Some(p) => serde_json::from_str(p).map_err(|e| format!("PARAMS isn't valid JSON: {e}"))?,
                None => json!({}),
            };
            let r = Client::connect()?.call(method, params)?;
            println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
            Ok(())
        }
        other => Err(format!("unknown command `{other}`; see `lumen msg help`")),
    }
}

/// Make a relative `--cwd` absolute, since Lumen's own directory differs.
fn absolute_cwd(params: &mut Map<String, Value>) -> Result<(), String> {
    if let Some(dir) = params.get("cwd").and_then(Value::as_str) {
        let path = PathBuf::from(dir);
        if path.is_relative() {
            let here = std::env::current_dir().map_err(|e| format!("can't resolve {dir}: {e}"))?;
            params.insert("cwd".into(), here.join(path).display().to_string().into());
        }
    }
    Ok(())
}

/// `list`'s result as readable text: one line per tab, then its panes.
fn list_text(result: &Value) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut out = String::new();
    for tab in result["tabs"].as_array().into_iter().flatten() {
        let marker = if tab["active"] == true { "*" } else { " " };
        out.push_str(&format!("{marker} tab {}  {}\n", tab["index"].as_u64().unwrap_or(0) + 1, tab["title"].as_str().unwrap_or("")));
        for p in tab["panes"].as_array().into_iter().flatten() {
            let marker = if p["focused"] == true { "*" } else if p["active"] == true { "+" } else { " " };
            let cwd = p["cwd"].as_str().unwrap_or("?");
            let cwd = match cwd.strip_prefix(&home) {
                Some(rest) if !home.is_empty() => format!("~{rest}"),
                _ => cwd.to_string(),
            };
            let exit = match p["last_exit"].as_i64() {
                Some(0) => String::new(),
                Some(code) => format!("  (last exit {code})"),
                None => String::new(),
            };
            out.push_str(&format!(
                "  {marker} {:<12} {}x{}  {}  {}{exit}\n",
                p["id"],
                p["cols"],
                p["rows"],
                cwd,
                p["title"].as_str().unwrap_or("")
            ));
        }
    }
    out
}

struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

impl Client {
    /// Connect to `$LUMEN_SOCKET`, or else the newest running Lumen.
    fn connect() -> Result<Self, String> {
        let stream = match std::env::var_os("LUMEN_SOCKET").filter(|s| !s.is_empty()) {
            Some(path) => UnixStream::connect(&path).map_err(|e| {
                format!("can't reach Lumen at {} ({e}); was it closed?", PathBuf::from(&path).display())
            })?,
            None => newest_lumen()?,
        };
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        Ok(Self { reader: BufReader::new(stream), writer, next_id: 1 })
    }

    fn read(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err("Lumen closed the connection".into()),
            Ok(_) => serde_json::from_str(&line).map_err(|e| format!("bad reply from Lumen: {e}")),
            Err(e) => Err(format!("lost the connection to Lumen: {e}")),
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.writer, "{req}").map_err(|e| format!("can't send to Lumen: {e}"))?;
        loop {
            let msg = self.read()?;
            if msg.get("id") != Some(&json!(id)) {
                continue; // an event for an earlier subscription
            }
            if let Some(err) = msg.get("error") {
                return Err(err["message"].as_str().unwrap_or("unknown error").to_string());
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

/// The most recently started Lumen that's still running.
fn newest_lumen() -> Result<UnixStream, String> {
    let dir = api::socket_dir()?;
    let mut sockets: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("lumen-") && name.ends_with(".sock")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    sockets.sort_by_key(|s| std::cmp::Reverse(s.0));
    sockets
        .iter()
        .find_map(|(_, path)| UnixStream::connect(path).ok())
        .ok_or_else(|| "no running Lumen found (is `socket-api` turned off in config.toml?)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_flags_and_positionals() {
        let a = parse(&s(&["down", "--cwd", "/tmp", "--run=make test", "--no-focus"]), &["cwd", "run"], &["no-focus"]).unwrap();
        assert_eq!(a.positional, vec!["down"]);
        assert_eq!(a.options["cwd"], "/tmp");
        assert_eq!(a.options["run"], "make test");
        assert_eq!(a.options["no-focus"], true);
        assert!(parse(&s(&["--bogus"]), &[], &[]).is_err());
        assert!(parse(&s(&["--cwd"]), &["cwd"], &[]).is_err());
        let a = parse(&s(&["--", "--not-a-flag"]), &[], &[]).unwrap();
        assert_eq!(a.positional, vec!["--not-a-flag"]);
    }

    #[test]
    fn unescapes_text() {
        assert_eq!(unescape(r"make\n"), "make\n");
        assert_eq!(unescape(r"a\tb\\c\e[0m\q\"), "a\tb\\c\x1b[0m\\q\\");
    }
}
