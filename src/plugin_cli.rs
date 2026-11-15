//! `lumen plugins`: install, update, remove and list plugins from the
//! registry (see `registry.rs`), and check a plugin before publishing it.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{Value, json};

use crate::plugins::{BUILTINS, Permission};
use crate::registry::{self, Approve, Index, Installed};

const HELP: &str = "\
Install, update and remove Lumen plugins.

Usage: lumen plugins <command> [options]

Commands:
  list [--json]                       Every plugin: built in, yours, and installed ones
  search [TEXT]                       Plugins in the registry, most stars first
  install OWNER/NAME [--yes]          Install a plugin and turn it on
  update [OWNER/NAME] [--yes]         Update one plugin, or all of them
  remove OWNER/NAME [--delete-data]   Remove a plugin; its data is kept unless asked
  on NAME | off NAME                  Turn a plugin on or off (built-in or installed)
  check FOLDER                        Check a plugin's plugin.toml before publishing it
  help                                Show this

Installing asks before a plugin may use what it needs (open panes, type into
them, read the screen); --yes approves that. An update that needs nothing new
doesn't ask. Running Lumens pick up changes by themselves.

Plugins come from the registry at https://github.com/lumenshell/plugins.
";

#[derive(Debug, PartialEq)]
enum Cmd {
    Help,
    List { json: bool },
    Search { text: String },
    Install { name: String, yes: bool },
    Update { name: Option<String>, yes: bool },
    Remove { name: String, delete_data: bool },
    On(String),
    Off(String),
    Check(String),
}

/// Run `lumen plugins` with `args` (after `plugins`); the exit code.
pub fn run(args: &[String]) -> i32 {
    let result = parse(args).and_then(|cmd| match cmd {
        Cmd::Help => {
            print!("{HELP}");
            Ok(())
        }
        cmd => execute(cmd),
    });
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("lumen plugins: {e}");
            1
        }
    }
}

fn parse(args: &[String]) -> Result<Cmd, String> {
    let mut flags = Vec::new();
    let mut words = Vec::new();
    for a in args {
        match a.strip_prefix("--") {
            Some(flag) => flags.push(flag.to_string()),
            None => words.push(a.clone()),
        }
    }
    let Some((command, rest)) = words.split_first() else {
        // No command: help, until the full-screen manager arrives.
        return Ok(Cmd::Help);
    };
    let allowed: &[&str] = match command.as_str() {
        "list" => &["json"],
        "install" | "update" => &["yes"],
        "remove" => &["delete-data"],
        _ => &[],
    };
    if let Some(bad) = flags.iter().find(|f| !allowed.contains(&f.as_str())) {
        return Err(format!("unknown option --{bad}; see `lumen plugins help`"));
    }
    let flag = |f: &str| flags.iter().any(|x| x == f);
    let registry_name = || match rest {
        [name] if registry::valid_name(name) => Ok(name.clone()),
        _ => Err(format!("`{command}` needs a plugin name like owner/name (see `lumen plugins search`)")),
    };
    let any_name = || match rest {
        [name] => Ok(name.clone()),
        _ => Err(format!("`{command}` needs one plugin name")),
    };
    Ok(match command.as_str() {
        "help" => Cmd::Help,
        "list" => Cmd::List { json: flag("json") },
        "search" => Cmd::Search { text: rest.join(" ") },
        "install" => Cmd::Install { name: registry_name()?, yes: flag("yes") },
        "update" => Cmd::Update { name: if rest.is_empty() { None } else { Some(registry_name()?) }, yes: flag("yes") },
        "remove" => Cmd::Remove { name: registry_name()?, delete_data: flag("delete-data") },
        "on" => Cmd::On(any_name()?),
        "off" => Cmd::Off(any_name()?),
        "check" => Cmd::Check(any_name()?),
        other => return Err(format!("unknown command `{other}`; see `lumen plugins help`")),
    })
}

/// Ask on the terminal whether `name` may have `new` permissions.
fn ask(name: &str, new: &[Permission]) -> bool {
    let what: Vec<&str> = new.iter().map(|p| p.describe()).collect();
    print!("Allow {name} to {}? [y/N] ", what.join("; "));
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    matches!(line.trim(), "y" | "Y" | "yes" | "Yes")
}

/// The `plugins` list in config.toml.
fn enabled() -> Vec<String> {
    crate::settings::config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| crate::settings::Settings::parse(&t).plugins)
        .unwrap_or_default()
}

fn execute(cmd: Cmd) -> Result<(), String> {
    let terminal = unsafe { libc::isatty(0) } == 1;
    let ask_fn = |n: &str, p: &[Permission]| ask(n, p);
    let approve = |yes: bool| {
        if yes {
            Approve::Yes
        } else if terminal {
            Approve::Ask(&ask_fn)
        } else {
            Approve::No
        }
    };
    match cmd {
        Cmd::Help => print!("{HELP}"),
        Cmd::List { json } => {
            let rows = rows(&enabled(), &registry::load_installed(), &crate::plugins::local_plugins(), registry::cached_index().as_ref());
            if json {
                println!("{}", rows_json(&rows));
            } else {
                for r in &rows {
                    let mark = if !r.installed { "!" } else if r.on { "●" } else { "○" };
                    let mut note = match r.kind {
                        "built-in" => "built in".to_string(),
                        "local" => format!("local {}", r.version).trim_end().to_string(),
                        _ => r.version.clone(),
                    };
                    if !r.installed {
                        note = format!("not installed: lumen plugins install {}", r.name);
                    } else if let Some(u) = &r.update {
                        note = format!("{note} → {u} (lumen plugins update {})", r.name);
                    }
                    println!("{mark} {:<28} {note}", r.name);
                }
            }
        }
        Cmd::Search { text } => {
            let index = registry::index(false)?;
            let installed = registry::load_installed();
            let found = registry::search(&index, &text);
            if found.is_empty() {
                println!("No plugins match \"{text}\".");
            }
            for e in found {
                let here = if installed.contains_key(&e.name) { "  (installed)" } else { "" };
                println!("{:<32} ★ {:<5} {}{here}", e.name, e.stars, e.description);
            }
        }
        Cmd::Install { name, yes } => {
            let index = registry::index(false)?;
            println!("{}", registry::install(&index, &name, &approve(yes))?);
        }
        Cmd::Update { name, yes } => {
            let index = registry::index(true)?;
            let names: Vec<String> = match name {
                Some(n) => vec![n],
                None => registry::load_installed().into_keys().collect(),
            };
            let mut changed = false;
            for n in names {
                if let Some(msg) = registry::update(&index, &n, &approve(yes))? {
                    println!("{msg}");
                    changed = true;
                }
            }
            if !changed {
                println!("Everything is up to date.");
            }
        }
        Cmd::Remove { name, delete_data } => println!("{}", registry::remove(&name, delete_data)?),
        Cmd::On(name) | Cmd::Off(name) if crate::plugins::local_plugins().iter().any(|(n, _)| *n == name) => {
            return Err(format!(
                "{name} is one of your own plugins, so it's always on; move its folder out of the plugins folder to turn it off"
            ));
        }
        Cmd::On(name) => {
            if crate::plugins::builtin(&name).is_none() && !registry::load_installed().contains_key(&name) {
                return Err(format!("there's no built-in or installed plugin called \"{name}\""));
            }
            registry::set_enabled(&name, true)?;
            println!("{name} is on");
        }
        Cmd::Off(name) => {
            registry::set_enabled(&name, false)?;
            println!("{name} is off");
        }
        Cmd::Check(dir) => println!("{}", check(Path::new(&dir))?),
    }
    Ok(())
}

/// `lumen plugins check FOLDER`: whether its plugin.toml is valid, and what
/// it says, as the registry's checks read it.
fn check(dir: &Path) -> Result<String, String> {
    let m = crate::plugins::check_folder(dir)?;
    let perms: Vec<&str> = m.permissions.iter().map(|p| p.name()).collect();
    Ok(format!("ok {} {} permissions={}", m.name, m.version, perms.join(",")))
}

/// One line of `lumen plugins list`.
struct Row {
    name: String,
    kind: &'static str,
    on: bool,
    version: String,
    installed: bool,
    /// A newer compatible version, if the last index fetched knows one.
    update: Option<String>,
}

fn rows(
    enabled: &[String],
    installed: &BTreeMap<String, Installed>,
    local: &[(String, String)],
    index: Option<&Index>,
) -> Vec<Row> {
    let on = |n: &str| enabled.iter().any(|e| e == n);
    let mut rows: Vec<Row> = BUILTINS
        .iter()
        .map(|b| Row { name: b.name.into(), kind: "built-in", on: on(b.name), version: String::new(), installed: true, update: None })
        .collect();
    rows.extend(local.iter().map(|(n, v)| Row {
        name: n.clone(),
        kind: "local",
        on: true,
        version: v.clone(),
        installed: true,
        update: None,
    }));
    for (name, i) in installed {
        let update = index
            .and_then(|idx| idx.plugins.iter().find(|e| &e.name == name))
            .and_then(|e| registry::best(e, registry::LUMEN_VERSION))
            .filter(|r| registry::newer(&r.version, &i.version))
            .map(|r| r.version.clone());
        rows.push(Row { name: name.clone(), kind: "registry", on: on(name), version: i.version.clone(), installed: true, update });
    }
    for name in enabled.iter().filter(|n| registry::valid_name(n) && !installed.contains_key(*n)) {
        rows.push(Row { name: name.clone(), kind: "registry", on: true, version: String::new(), installed: false, update: None });
    }
    rows
}

fn rows_json(rows: &[Row]) -> Value {
    let list: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({"name": r.name, "kind": r.kind, "on": r.on, "version": r.version, "installed": r.installed, "update": r.update})
        })
        .collect();
    json!({ "plugins": list })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Entry, Index, Installed, Release};

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn arguments_parse() {
        assert_eq!(parse(&s(&[])), Ok(Cmd::Help));
        assert_eq!(parse(&s(&["install", "mira/x", "--yes"])), Ok(Cmd::Install { name: "mira/x".into(), yes: true }));
        assert_eq!(parse(&s(&["install", "mira/x"])), Ok(Cmd::Install { name: "mira/x".into(), yes: false }));
        assert_eq!(parse(&s(&["update"])), Ok(Cmd::Update { name: None, yes: false }));
        assert_eq!(parse(&s(&["update", "--yes", "mira/x"])), Ok(Cmd::Update { name: Some("mira/x".into()), yes: true }));
        assert_eq!(parse(&s(&["remove", "mira/x", "--delete-data"])), Ok(Cmd::Remove { name: "mira/x".into(), delete_data: true }));
        assert_eq!(parse(&s(&["list", "--json"])), Ok(Cmd::List { json: true }));
        assert_eq!(parse(&s(&["search", "git", "tools"])), Ok(Cmd::Search { text: "git tools".into() }));
        assert_eq!(parse(&s(&["on", "recent-dirs"])), Ok(Cmd::On("recent-dirs".into())));
        assert_eq!(parse(&s(&["check", "."])), Ok(Cmd::Check(".".into())));
        assert!(parse(&s(&["instal", "mira/x"])).unwrap_err().contains("instal"));
        assert!(parse(&s(&["install"])).unwrap_err().contains("owner/name"));
        assert!(parse(&s(&["install", "Mira/X"])).unwrap_err().contains("owner/name"));
        assert!(parse(&s(&["install", "mira/x", "--force"])).unwrap_err().contains("--force"));
    }

    #[test]
    fn check_folder_reports() {
        let dir = std::env::temp_dir().join(format!("lumen-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), "version = \"1.2.0\"\ncommand = \"python3 main.py\"\npermissions = [\"input\", \"panes\"]\n").unwrap();
        assert_eq!(check(&dir), Ok(format!("ok {} 1.2.0 permissions=panes,input", dir.file_name().unwrap().to_string_lossy())));
        std::fs::write(dir.join("plugin.toml"), "command = \"x\"\n[settings.a]\ntype = \"integer\"\n").unwrap();
        assert!(check(&dir).unwrap_err().contains("settings.a"));
        std::fs::remove_file(dir.join("plugin.toml")).unwrap();
        assert!(check(&dir).unwrap_err().contains("plugin.toml"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn list_shows_every_kind() {
        let mut installed = std::collections::BTreeMap::new();
        installed.insert(
            "mira/x".to_string(),
            Installed { version: "1.0.0".into(), commit: "c".repeat(40), permissions: vec![], dir: "/i".into() },
        );
        let release = |v: &str| Release {
            version: v.into(),
            commit: "c".repeat(40),
            permissions: vec![],
            lumen: "1.0.0".into(),
            archive: String::new(),
        };
        let index = Index {
            version: 1,
            plugins: vec![Entry {
                name: "mira/x".into(),
                repo: String::new(),
                path: String::new(),
                description: String::new(),
                license: String::new(),
                categories: vec![],
                stars: 0,
                updated: String::new(),
                versions: vec![release("1.0.0"), release("1.1.0")],
            }],
        };
        let enabled = s(&["recent-dirs", "mira/x", "mira/gone"]);
        let local = vec![("where".to_string(), "0.1.0".to_string())];
        let rows = rows(&enabled, &installed, &local, Some(&index));
        let json = rows_json(&rows);
        let find = |n: &str| json["plugins"].as_array().unwrap().iter().find(|r| r["name"] == n).cloned().unwrap();
        assert_eq!(find("recent-dirs")["kind"], "built-in");
        assert_eq!(find("recent-dirs")["on"], true);
        assert_eq!(find("pomodoro")["on"], false);
        assert_eq!(find("where")["kind"], "local");
        assert_eq!(find("mira/x")["version"], "1.0.0");
        assert_eq!(find("mira/x")["update"], "1.1.0");
        assert_eq!(find("mira/gone")["installed"], false);
    }
}
