//! The plugin manager: `lumen plugins` with no arguments, a full-screen
//! terminal UI for finding, installing, updating, removing, turning on and
//! off, and configuring plugins.

pub mod data;
pub mod model;
pub mod settings_edit;
pub mod view;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use serde_json::json;

use crate::manager::data::Snapshot;
use crate::manager::model::{Effect, Model};
use crate::registry::{self, Approve, Index, Paths};

/// Where the manager reads and writes: the real places, or a test's.
pub(crate) struct Env {
    pub paths: Paths,
    pub index_url: String,
    /// The index's cached copy.
    pub cache: PathBuf,
}

impl Env {
    fn real() -> Result<Env, String> {
        Ok(Env {
            paths: Paths::real()?,
            index_url: registry::index_url(),
            cache: registry::cache_dir().ok_or("no home folder")?.join("plugin-index.json"),
        })
    }
}

/// `lumen plugins` with no arguments, in a terminal; the exit code.
pub fn run() -> i32 {
    let env = match Env::real() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("lumen plugins: {e}");
            return 1;
        }
    };
    let guard = match Screen::enter() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("lumen plugins: can't use this terminal: {e}");
            return 1;
        }
    };
    if std::env::var_os("LUMEN_MANAGER_TEST_PANIC").is_some() {
        panic!("a test asked the manager to panic");
    }
    let backend = CrosstermBackend::new(std::io::stdout());
    let result = run_with(env, backend, |wait| {
        if event::poll(wait).ok()? { event::read().ok() } else { None }
    });
    drop(guard);
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("lumen plugins: {e}");
            1
        }
    }
}

/// The terminal in raw mode on the alternate screen, catching the mouse;
/// put back when dropped, and before a panic's message.
struct Screen;

impl Screen {
    fn enter() -> std::io::Result<Screen> {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            Screen::restore();
            previous(info);
        }));
        enable_raw_mode()?;
        let screen = Screen;
        execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        Ok(screen)
    }

    fn restore() {
        let _ = execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        let _ = std::io::stdout().flush();
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        Screen::restore();
    }
}

/// What the worker thread hands back after each job.
struct Done {
    /// Something you asked for (rather than the load as the manager opens).
    asked: bool,
    result: Result<String, String>,
    snapshot: Snapshot,
    /// The index, when this job loaded it (or failed to).
    index: Option<Result<Index, String>>,
}

enum Job {
    /// Load the index: the cached copy unless it's over an hour old.
    LoadIndex { refresh: bool },
    Apply(Box<Effect>),
}

/// The manager's loop, drawing to `backend` and reading events from
/// `next_event` (which waits up to the given time for one).
pub(crate) fn run_with<B: Backend>(
    env: Env,
    backend: B,
    mut next_event: impl FnMut(Duration) -> Option<Event>,
) -> Result<(), String>
where
    B::Error: std::fmt::Display,
{
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    let cached = std::fs::read_to_string(&env.cache).ok().and_then(|t| registry::parse_index(&t).ok());
    let mut model = Model::new(Snapshot::load(&env.paths, cached.clone(), statuses()));
    let (jobs, done) = worker(env, cached);
    let _ = jobs.send(Job::LoadIndex { refresh: false });
    loop {
        terminal.draw(|f| view::draw(f, &model)).map_err(|e| e.to_string())?;
        while let Ok(d) = done.try_recv() {
            match d.index {
                Some(Ok(_)) => model.index_error = None,
                Some(Err(e)) => model.index_error = Some(e),
                None => {}
            }
            if d.asked {
                model.done(d.result, d.snapshot);
            } else {
                model.loaded(d.result, d.snapshot);
            }
        }
        let effects = match next_event(Duration::from_millis(100)) {
            Some(Event::Key(k)) if k.kind != KeyEventKind::Release => model.key(k),
            Some(Event::Mouse(m)) => {
                match m.kind {
                    MouseEventKind::ScrollDown => model.scroll(3),
                    MouseEventKind::ScrollUp => model.scroll(-3),
                    MouseEventKind::Down(MouseButton::Left) => {
                        let area = terminal.size().map_err(|e| e.to_string())?;
                        let (row, tab) = view::hit(area.into(), &model, m.column, m.row);
                        model.click(row, tab);
                    }
                    _ => {}
                }
                Vec::new()
            }
            _ => Vec::new(),
        };
        for e in effects {
            match e {
                Effect::Quit => return Ok(()),
                Effect::RefreshIndex => {
                    let _ = jobs.send(Job::LoadIndex { refresh: true });
                }
                e => {
                    let _ = jobs.send(Job::Apply(Box::new(e)));
                }
            }
        }
    }
}

/// A thread doing jobs one at a time, so the screen never waits on the
/// network or the disk. It's left behind (and ends with the process) if
/// you quit mid-download.
fn worker(env: Env, cached: Option<Index>) -> (mpsc::Sender<Job>, mpsc::Receiver<Done>) {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut index = cached;
        for job in job_rx {
            let asked = !matches!(job, Job::LoadIndex { refresh: false });
            let (result, loaded) = match job {
                Job::LoadIndex { refresh } => {
                    let got = registry::index_from(&env.index_url, &env.cache, refresh);
                    let result = match (&got, refresh) {
                        (Ok(i), true) => Ok(format!("The plugin index is up to date ({} plugins)", i.plugins.len())),
                        (Ok(_), false) => Ok(String::new()),
                        (Err(e), _) => Err(format!("Couldn't load the plugin index: {e}")),
                    };
                    if let Ok(i) = &got {
                        index = Some(i.clone());
                    }
                    (result, Some(got))
                }
                Job::Apply(effect) => (apply(&env.paths, index.as_ref(), &effect), None),
            };
            let snapshot = Snapshot::load(&env.paths, index.clone(), statuses());
            if done_tx.send(Done { asked, result, snapshot, index: loaded }).is_err() {
                break;
            }
        }
    });
    (job_tx, done_rx)
}

/// Carry out `effect`; what happened, for the status line. Approval was
/// asked already, by the model.
pub(crate) fn apply(p: &Paths, index: Option<&Index>, effect: &Effect) -> Result<String, String> {
    let index = || index.ok_or_else(|| "The plugin index isn't loaded yet; press r to load it".to_string());
    match effect {
        Effect::Install { name, allowed } => {
            let index = index()?;
            only_allowed(p, index, name, allowed)?;
            registry::install_in(p, index, name, &Approve::Yes)
        }
        Effect::Update { name, allowed } => {
            let index = index()?;
            only_allowed(p, index, name, allowed)?;
            Ok(registry::update_in(p, index, name, &Approve::Yes)?.unwrap_or_else(|| format!("{name} is up to date")))
        }
        Effect::UpdateAll(each) => {
            let index = index()?;
            let (mut done, mut failed) = (0, Vec::new());
            for (name, allowed) in each {
                let updated = only_allowed(p, index, name, allowed)
                    .and_then(|()| registry::update_in(p, index, name, &Approve::Yes));
                match updated {
                    Ok(Some(_)) => done += 1,
                    Ok(None) => {}
                    Err(e) => failed.push(format!("{name} failed: {e}")),
                }
            }
            let summary = match done {
                0 if failed.is_empty() => "Everything is up to date".to_string(),
                1 => "Updated 1 plugin".to_string(),
                n => format!("Updated {n} plugins"),
            };
            if failed.is_empty() { Ok(summary) } else { Err(format!("{summary}; {}", failed.join("; "))) }
        }
        Effect::Remove { name, delete_data } => registry::remove_in(p, name, *delete_data),
        Effect::SetEnabled { name, on, approve } => {
            if let Some(m) = approve {
                // Approved before it's listed, so a running Lumen starts it
                // without asking again.
                crate::plugins::record_decision_at(&p.approvals, m, true);
            }
            registry::set_enabled_in(p, name, *on)?;
            Ok(format!("Turned {name} {}", if *on { "on" } else { "off" }))
        }
        Effect::SaveSetting { plugin, key, value } => {
            let toml = match value {
                Some(v) => Some(settings_edit::to_toml(v).ok_or_else(|| format!("`{key}` can't be saved"))?),
                None => None,
            };
            crate::settings::edit_config_at(&p.config, |doc| {
                if !doc.contains_key("plugin") {
                    let mut t = toml_edit::Table::new();
                    t.set_implicit(true);
                    doc.insert("plugin", toml_edit::Item::Table(t));
                }
                let Some(plugins) = doc["plugin"].as_table_mut() else { return };
                if !plugins.contains_key(plugin) {
                    plugins.insert(plugin, toml_edit::Item::Table(toml_edit::Table::new()));
                }
                if let Some(table) = plugins.get_mut(plugin).and_then(|t| t.as_table_mut()) {
                    match &toml {
                        Some(v) => {
                            // Keep a comment after the old value.
                            let mut v = v.clone();
                            if let Some(old) = table.get(key).and_then(|i| i.as_value()) {
                                *v.decor_mut() = old.decor().clone();
                            }
                            table.insert(key, toml_edit::Item::Value(v));
                        }
                        None => {
                            table.remove(key);
                        }
                    }
                    if table.is_empty() {
                        plugins.remove(plugin);
                    }
                }
                if plugins.is_empty() {
                    doc.remove("plugin");
                }
            })?;
            Ok(match value {
                Some(_) => format!("Saved {plugin}'s {key}"),
                None => format!("Reset {plugin}'s {key} to its default"),
            })
        }
        Effect::RefreshIndex | Effect::Quit => Ok(String::new()),
    }
}

/// Refuse when `name`'s newest release needs permissions beyond what was
/// shown and `allowed` (the index changed after you were asked).
fn only_allowed(p: &Paths, index: &Index, name: &str, allowed: &[crate::plugins::Permission]) -> Result<(), String> {
    let needs = index
        .plugins
        .iter()
        .find(|e| e.name == name)
        .and_then(|e| registry::best(e, registry::LUMEN_VERSION))
        .map(|r| registry::new_permissions_in(p, name, &r.permissions))
        .unwrap_or_default();
    if needs.iter().all(|x| allowed.contains(x)) {
        Ok(())
    } else {
        Err(format!("{name} now asks for more than you allowed; nothing was changed. Press the key again to see what"))
    }
}

/// What the Lumen this runs in says about each plugin: its status, and
/// whether that's trouble. Empty outside Lumen, or if Lumen can't say.
fn statuses() -> BTreeMap<String, (String, bool)> {
    if std::env::var_os("LUMEN_SOCKET").is_none_or(|s| s.is_empty()) {
        return BTreeMap::new();
    }
    let Ok(mut client) = crate::cli::Client::connect() else { return BTreeMap::new() };
    let Ok(reply) = client.call("plugin-status", json!({})) else { return BTreeMap::new() };
    reply["plugins"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|p| {
                    let name = p["name"].as_str()?.to_string();
                    Some((name, (p["status"].as_str().unwrap_or_default().to_string(), p["trouble"].as_bool().unwrap_or(false))))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use crate::plugins::Permission;
    use crate::registry::testing::{archive, entry, paths, release};
    use crate::registry::{Index, Paths};

    fn index_of(plugins: Vec<(&str, Vec<crate::registry::Release>)>) -> Index {
        Index {
            version: 1,
            plugins: plugins
                .into_iter()
                .map(|(n, v)| {
                    let mut e = entry(n, 1, &[]);
                    e.versions = v;
                    e
                })
                .collect(),
        }
    }

    fn install(name: &str, allowed: Vec<Permission>) -> Effect {
        Effect::Install { name: name.into(), allowed }
    }

    #[test]
    fn installs_refuse_permissions_not_shown() {
        let (dir, p) = paths("manager-not-shown");
        let idx = index_of(vec![("mira/x", vec![release("1.0.0", vec![Permission::Panes], archive(&dir, "1.0.0", "\"panes\""))])]);
        let e = apply(&p, Some(&idx), &install("mira/x", vec![])).unwrap_err();
        assert!(e.contains("asks for more"), "{e}");
        assert!(crate::registry::load_installed_from(&p.installed).is_empty(), "nothing installed");
        apply(&p, Some(&idx), &install("mira/x", vec![Permission::Panes])).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn update_all_updates_only_what_was_shown() {
        let (dir, p) = paths("manager-update-shown");
        let ydir = dir.join("y");
        let (x1, x2) = (release("1.0.0", vec![], archive(&dir, "1.0.0", "")), release("1.1.0", vec![], archive(&dir, "1.1.0", "")));
        let (y1, y2) = (release("1.0.0", vec![], archive(&ydir, "1.0.0", "")), release("1.1.0", vec![], archive(&ydir, "1.1.0", "")));
        let first = index_of(vec![("mira/x", vec![x1.clone()]), ("mira/y", vec![y1.clone()])]);
        apply(&p, Some(&first), &install("mira/x", vec![])).unwrap();
        apply(&p, Some(&first), &install("mira/y", vec![])).unwrap();
        let next = index_of(vec![("mira/x", vec![x1, x2]), ("mira/y", vec![y1, y2])]);
        apply(&p, Some(&next), &Effect::UpdateAll(vec![("mira/x".into(), vec![])])).unwrap();
        let all = crate::registry::load_installed_from(&p.installed);
        assert_eq!((all["mira/x"].version.as_str(), all["mira/y"].version.as_str()), ("1.1.0", "1.0.0"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn config(p: &Paths) -> String {
        std::fs::read_to_string(&p.config).unwrap_or_default()
    }

    #[test]
    fn effects_change_the_files() {
        let (dir, p) = paths("manager-effects");
        let v1 = release("1.0.0", vec![], archive(&dir, "1.0.0", ""));
        let v2 = release("1.1.0", vec![], archive(&dir, "1.1.0", ""));
        let one = index_of(vec![("mira/x", vec![v1.clone()])]);
        let msg = apply(&p, Some(&one), &install("mira/x", vec![])).unwrap();
        assert!(msg.contains("mira/x 1.0.0"), "{msg}");
        assert!(config(&p).contains("\"mira/x\""));
        apply(&p, Some(&one), &Effect::SetEnabled { name: "mira/x".into(), on: false, approve: None }).unwrap();
        assert!(!config(&p).contains("\"mira/x\""));
        let two = index_of(vec![("mira/x", vec![v1, v2])]);
        let msg = apply(&p, Some(&two), &Effect::Update { name: "mira/x".into(), allowed: vec![] }).unwrap();
        assert!(msg.contains("1.1.0"), "{msg}");
        assert_eq!(crate::registry::load_installed_from(&p.installed)["mira/x"].version, "1.1.0");
        let data = p.data.join("mira/x");
        std::fs::create_dir_all(&data).unwrap();
        apply(&p, Some(&two), &Effect::Remove { name: "mira/x".into(), delete_data: false }).unwrap();
        assert!(crate::registry::load_installed_from(&p.installed).is_empty());
        assert!(data.is_dir(), "data kept");
        let e = apply(&p, None, &install("mira/x", vec![])).unwrap_err();
        assert!(e.contains("index"), "{e}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn turning_a_builtin_on_records_approval_first() {
        let (dir, p) = paths("manager-builtin");
        let m = crate::plugins::builtin("build-status").unwrap().parsed().unwrap();
        assert!(!m.permissions.is_empty(), "the test needs a built-in with permissions");
        assert_ne!(crate::plugins::decision_at(&p.approvals, &m), crate::plugins::Decision::Allowed);
        apply(&p, None, &Effect::SetEnabled { name: "build-status".into(), on: true, approve: Some(m.clone()) }).unwrap();
        assert_eq!(crate::plugins::decision_at(&p.approvals, &m), crate::plugins::Decision::Allowed);
        assert!(config(&p).contains("\"build-status\""));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn update_all_reports_each_failure() {
        let (dir, p) = paths("manager-update-all");
        let ydir = dir.join("y");
        let x1 = release("1.0.0", vec![], archive(&dir, "1.0.0", ""));
        let x2 = release("1.1.0", vec![], archive(&dir, "1.1.0", ""));
        let y1 = release("1.0.0", vec![], archive(&ydir, "1.0.0", ""));
        let y2 = release("1.1.0", vec![], dir.join("missing.tar.gz").display().to_string());
        let first = index_of(vec![("mira/x", vec![x1.clone()]), ("mira/y", vec![y1.clone()])]);
        apply(&p, Some(&first), &install("mira/x", vec![])).unwrap();
        apply(&p, Some(&first), &install("mira/y", vec![])).unwrap();
        let next = index_of(vec![("mira/x", vec![x1, x2]), ("mira/y", vec![y1, y2])]);
        let e = apply(&p, Some(&next), &Effect::UpdateAll(vec![("mira/x".into(), vec![]), ("mira/y".into(), vec![])])).unwrap_err();
        assert!(e.contains("mira/y"), "{e}");
        assert!(e.contains("Updated 1 plugin"), "{e}");
        assert_eq!(crate::registry::load_installed_from(&p.installed)["mira/x"].version, "1.1.0");
        assert_eq!(crate::registry::load_installed_from(&p.installed)["mira/y"].version, "1.0.0");
        std::fs::remove_dir_all(dir).unwrap();
        let _ = Permission::Panes;
    }

    /// Run by `terminal_is_restored_after_a_panic` inside a pseudo-terminal.
    #[test]
    #[ignore]
    fn panic_child() {
        if std::env::var_os("LUMEN_MANAGER_TEST_PANIC").is_some() {
            run();
        }
    }

    #[test]
    fn terminal_is_restored_after_a_panic() {
        if std::process::Command::new("script").arg("--version").output().is_err() {
            eprintln!("skipped: no `script` to make a pseudo-terminal");
            return;
        }
        let me = std::env::current_exe().unwrap();
        let inner = format!(
            "LUMEN_MANAGER_TEST_PANIC=1 '{}' manager::tests::panic_child --exact --ignored --nocapture >/dev/null 2>&1; stty -a",
            me.display()
        );
        let out = std::process::Command::new("script").args(["-qec", &inner, "/dev/null"]).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains(" icanon") && text.contains(" echo"), "the terminal stayed raw:\n{text}");
    }

    #[test]
    fn keys_work_while_a_download_runs() {
        let (dir, p) = paths("manager-hang");
        let fifo = dir.join("index.fifo");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let env = Env { paths: p, index_url: fifo.display().to_string(), cache: dir.join("cache/index.json") };
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut sent = false;
        let start = std::time::Instant::now();
        let next = move |_wait: std::time::Duration| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if sent {
                return None;
            }
            sent = true;
            Some(Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)))
        };
        run_with(env, backend, next).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(1), "{:?}", start.elapsed());
        // The download thread is still stuck on the fifo; open it for
        // writing so it finishes, then clean up.
        let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod setting_tests {
    use super::*;
    use crate::registry::testing::paths;
    use serde_json::json;

    #[test]
    fn setting_saves_keep_comments_and_other_keys() {
        let (dir, p) = paths("manager-settings");
        std::fs::create_dir_all(p.config.parent().unwrap()).unwrap();
        std::fs::write(&p.config, "# my settings\nfont-size = 14\n\n[plugin.command-history]\nmax-entries = 5000 # plenty\n").unwrap();
        let save = |plugin: &str, key: &str, value: Option<serde_json::Value>| {
            apply(&p, None, &Effect::SaveSetting { plugin: plugin.into(), key: key.into(), value }).unwrap()
        };
        save("command-history", "max-entries", Some(json!(20000)));
        crate::settings::edit_config_at(&p.config, |doc| doc["font-size"] = toml_edit::value(16)).unwrap();
        save("mira/x", "hosts", Some(json!(["a", "b"])));
        let text = std::fs::read_to_string(&p.config).unwrap();
        assert!(text.contains("# my settings"), "{text}");
        assert!(text.contains("font-size = 16"), "{text}");
        assert!(text.contains("max-entries = 20000"), "{text}");
        assert!(text.contains("[plugin.\"mira/x\"]") && text.contains("hosts = [\"a\", \"b\"]"), "{text}");
        let settings = crate::settings::Settings::parse(&text);
        assert!(settings.plugin_settings.contains_key("mira/x"));
        save("mira/x", "hosts", None);
        let text = std::fs::read_to_string(&p.config).unwrap();
        assert!(!text.contains("mira/x"), "an emptied table goes too:\n{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
