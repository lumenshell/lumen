//! Plugins in the window: starting and reloading them, asking for approval, and
//! the prompts, pickers and logs they open.

use super::*;

impl Workspace {
    /// Load the plugins folder: start what's allowed, and ask about the rest.
    pub fn start_plugins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::plugins;
        let enabled = Settings::get(cx).plugins.clone();
        plugins::set_builtins_running(enabled.clone());
        let (manifests, problems) = plugins::discover(&enabled);
        self.plugin_problems = problems;
        self.pending_approvals.clear();
        for m in manifests {
            let decision = plugins::decision(&m);
            self.load_plugin(m, decision, cx);
        }
        self.push_plugin_settings(cx);
        self.ask_next_approval(window, cx);
        cx.notify();
    }

    /// Start a plugin, or note why it isn't running.
    fn load_plugin(&mut self, m: crate::plugins::Manifest, decision: crate::plugins::Decision, cx: &mut Context<Self>) {
        use crate::plugins::{self, Decision};
        let (settings, _) = plugin_settings_for(&m, cx);
        match decision {
            Decision::Allowed => plugins::start(m, settings),
            Decision::Refused => {
                let status = plugins::refused_status(&m);
                plugins::add_stopped(m, &status, settings)
            }
            Decision::Ask => {
                plugins::add_stopped(m.clone(), "waiting for your approval", settings);
                if !self.pending_approvals.iter().any(|p| p.name == m.name) {
                    self.pending_approvals.push(m);
                }
            }
        }
    }

    /// Bring the running plugins in line with config.toml and what's
    /// installed, restarting only those that changed: turning one plugin
    /// on, or updating it, leaves the others running.
    pub fn sync_plugins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::plugins::{self, Decision};
        let enabled = Settings::get(cx).plugins.clone();
        plugins::set_builtins_running(enabled.clone());
        let (manifests, problems) = plugins::discover(&enabled);
        self.plugin_problems = problems;
        let wanted: Vec<_> = manifests.into_iter().map(|m| (plugins::decision(&m), m)).collect();
        // Waiting for approval isn't running either: compare it as refused.
        let compare: Vec<_> = wanted
            .iter()
            .map(|(d, m)| (m.clone(), if *d == Decision::Allowed { Decision::Allowed } else { Decision::Refused }))
            .collect();
        let running: Vec<_> = plugins::list()
            .into_iter()
            .map(|i| (i.manifest, if i.started { Decision::Allowed } else { Decision::Refused }))
            .collect();
        let reload = plugins::plan_reload(&running, &compare);
        for name in &reload.stop {
            plugins::stop(name);
        }
        for (decision, m) in wanted {
            if reload.start.contains(&m.name) {
                self.load_plugin(m, decision, cx);
            }
        }
        keymap::apply(cx);
        self.push_plugin_settings(cx);
        self.ask_next_approval(window, cx);
        cx.notify();
    }

    /// `config.toml` changed: give every plugin its settings as they are now.
    /// Only plugins whose values changed hear about it, and none restart.
    pub(super) fn push_plugin_settings(&mut self, cx: &mut Context<Self>) {
        let mut problems = Vec::new();
        for info in crate::plugins::list() {
            let (settings, p) = plugin_settings_for(&info.manifest, cx);
            problems.extend(p);
            crate::plugins::update_settings(&info.manifest.name, settings);
        }
        if problems != self.plugin_setting_problems {
            self.plugin_setting_problems = problems;
            cx.notify();
        }
    }

    pub(super) fn reload_plugins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::plugins::stop_all();
        crate::plugins::clear();
        keymap::apply(cx);
        self.start_plugins(window, cx);
    }

    /// Plugins: the manager (`lumen plugins`), in a new tab after this one.
    /// Quitting it closes the tab and brings this one back.
    pub(super) fn open_plugin_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe.display().to_string(),
            Err(_) => {
                self.plugin_problems.push("couldn't find Lumen's own program to open Plugins".into());
                cx.notify();
                return;
            }
        };
        self.open_tab(NewPane::Program(vec![exe, "plugins".into()]), window, cx);
        self.tabs[self.active_tab].title = Some("Plugins".into());
    }

    /// Ask about the next plugin waiting for approval, in the palette.
    pub(super) fn ask_next_approval(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_approvals.is_empty() {
            return;
        }
        let m = self.pending_approvals.remove(0);
        let wants: Vec<String> = m.permissions.iter().map(|p| p.describe().to_string()).collect();
        let what = if m.description.is_empty() { String::new() } else { format!(" ({})", m.description) };
        let question: SharedString = format!("Allow the plugin \"{}\"?", m.name).into();
        let home = std::env::var("HOME").unwrap_or_default();
        let message = format!(
            "\"{}\"{what} wants to {}. It runs `{}` in {}.",
            m.name,
            join_and(&wants),
            m.command.join(" "),
            m.dir.display().to_string().replacen(&home, "~", 1),
        );
        let ws = cx.entity().downgrade();
        let choice = |allow: bool, m: crate::plugins::Manifest, ws: gpui::WeakEntity<Workspace>| -> PaletteFn {
            Rc::new(move |window, cx| {
                if let Err(e) = crate::plugins::record_decision(&m, allow) {
                    // It still takes effect now; it just isn't remembered.
                    let problem =
                        format!("couldn't keep your answer about \"{}\", so Lumen will ask again: {e}", m.name);
                    let _ = ws.update(cx, |ws, cx| {
                        ws.plugin_problems.push(problem);
                        cx.notify();
                    });
                }
                let (settings, _) = plugin_settings_for(&m, cx);
                if allow {
                    crate::plugins::start(m.clone(), settings);
                } else {
                    crate::plugins::add_stopped(m.clone(), "not allowed", settings);
                }
                let _ = ws.update(cx, |ws, cx| ws.ask_next_approval(window, cx));
            })
        };
        let items = vec![
            PaletteItem::new(format!("Allow and start \"{}\"", m.name), choice(true, m.clone(), ws.clone()))
                .note(m.command.join(" ")),
            PaletteItem::new("Don't allow", choice(false, m.clone(), ws.clone()))
                .note("it won't run; you can allow it later from the palette"),
        ];
        // Esc: decide later (it's asked again next launch).
        let later: PaletteFn = Rc::new(move |window, cx| {
            let _ = ws.update(cx, |ws, cx| ws.ask_next_approval(window, cx));
        });
        let palette = cx.new(|cx| CommandPalette::picker(items, question, Some(later), cx).with_message(message));
        self.open_palette(palette, window, cx);
    }

    /// `show-prompt`: let a plugin or script ask for a line of text. The
    /// answer is `{"text": ...}` with what you typed (trimmed), or
    /// `{"text": null}` if you close it.
    pub fn show_prompt(
        &mut self,
        params: &Value,
        reply: futures::channel::oneshot::Sender<ApiResult>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (placeholder, initial, message) = match parse_prompt(params) {
            Ok(p) => p,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        // Sent once: on Enter, or null on any way of closing (the drop covers
        // a palette that goes away without running its cancel hook).
        let answer = Rc::new(Answer::new("text", reply));
        let submit_answer = answer.clone();
        let submit: crate::palette::SubmitFn = Rc::new(move |text, _, _| submit_answer.send(json!(text)));
        let cancel: PaletteFn = Rc::new(move |_, _| answer.send(Value::Null));
        let palette = cx.new(|cx| {
            let prompt = CommandPalette::prompt(placeholder.into(), initial, submit, cx).with_cancel(cancel);
            match message {
                Some(m) => prompt.with_message(m),
                None => prompt,
            }
        });
        self.open_palette(palette, window, cx);
    }

    /// `show-picker`: let a plugin or script offer a choice in the palette.
    /// The answer is `{"id": ...}`, or `{"id": null}` if you close it.
    pub fn show_picker(
        &mut self,
        params: &Value,
        reply: futures::channel::oneshot::Sender<ApiResult>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(list) = params.get("items").and_then(Value::as_array).filter(|l| !l.is_empty()) else {
            let _ = reply.send(Err(ApiError::invalid_params("`items` should be a non-empty list of {id, label}")));
            return;
        };
        let mut parsed = Vec::new();
        for item in list.iter().take(5000) {
            let id = item.get("id").cloned().filter(|v| v.is_string() || v.is_number());
            let label = item.get("label").and_then(Value::as_str);
            match (id, label) {
                (Some(id), Some(label)) => parsed.push((
                    id,
                    label.to_string(),
                    item.get("detail").and_then(Value::as_str).unwrap_or_default().to_string(),
                )),
                _ => {
                    let _ = reply.send(Err(ApiError::invalid_params("each item needs an `id` and a `label`")));
                    return;
                }
            }
        }
        let placeholder: SharedString =
            params.get("placeholder").and_then(Value::as_str).unwrap_or("Choose…").to_string().into();
        let answer = Rc::new(Answer::new("id", reply));
        let items = parsed
            .into_iter()
            .map(|(id, label, detail)| {
                let answer = answer.clone();
                PaletteItem::new(label, Rc::new(move |_, _| answer.send(id.clone()))).note(detail)
            })
            .collect();
        let cancel_answer = answer.clone();
        drop(answer);
        let cancel: PaletteFn = Rc::new(move |_, _| cancel_answer.send(Value::Null));
        let message = params.get("message").and_then(Value::as_str).map(str::trim).filter(|m| !m.is_empty());
        let palette = cx.new(|cx| {
            let picker = CommandPalette::picker(items, placeholder, Some(cancel), cx);
            match message {
                Some(m) => picker.with_message(m.to_string()),
                None => picker,
            }
        });
        self.open_palette(palette, window, cx);
    }

    pub(super) fn run_plugin_command(&mut self, a: &RunPluginCommand, _: &mut Window, cx: &mut Context<Self>) {
        let pane = self.active_terminal().map(|t| t.entity_id().as_u64());
        if let Err(e) = crate::plugins::run_command(&a.plugin, &a.id, pane) {
            eprintln!("lumen: {e}");
        }
        cx.notify();
    }

    /// Show a plugin's log in a new tab, following it.
    /// Pick a plugin, by how it's doing, to open its log.
    pub(super) fn open_plugin_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws = cx.entity().downgrade();
        let items = crate::plugins::list()
            .into_iter()
            .map(|info| {
                let ws = ws.clone();
                let name = info.manifest.name.clone();
                // How it's doing as a dot; the words only when something's up.
                let dot = plugin_dot(&info.status);
                let item = PaletteItem::new(
                    name.clone(),
                    Rc::new(move |window, cx| {
                        let _ = ws.update(cx, |ws, cx| ws.open_plugin_log(&name, window, cx));
                    }),
                )
                .dot(dot);
                if dot == Dot::Good { item } else { item.note(info.status.clone()) }
            })
            .collect();
        let palette = cx.new(|cx| CommandPalette::picker(items, "Plugin Logs: choose a plugin".into(), None, cx));
        self.open_palette(palette, window, cx);
    }

    fn open_plugin_log(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = crate::plugins::log_path(name) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&path);
        let quoted = format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
        let term = self.open_tab(NewPane::Shell(path.parent().map(std::path::Path::to_path_buf)), window, cx);
        term.read(cx).send_text(&format!("tail -n 200 -F {quoted}\r"));
        self.tabs[self.active_tab].title = Some(format!("{name} log"));
    }
}

/// Problems in the config file, shown at the bottom of the window until fixed.
/// A plugin's settings from its `[plugin.NAME]` table, defaults filled in,
/// and what's wrong with the table.
fn plugin_settings_for(
    m: &crate::plugins::Manifest,
    cx: &App,
) -> (serde_json::Map<String, serde_json::Value>, Vec<String>) {
    let given = Settings::get(cx).plugin_settings.get(&m.name).map(|t| &t.0);
    crate::plugin_settings::resolve(&m.name, &m.settings, given)
}

/// Answers a request that waits on the palette once: with the choice, or
/// with null when dropped unanswered.
struct Answer {
    key: &'static str,
    tx: std::cell::RefCell<Option<futures::channel::oneshot::Sender<ApiResult>>>,
}

impl Answer {
    fn new(key: &'static str, tx: futures::channel::oneshot::Sender<ApiResult>) -> Self {
        Self { key, tx: std::cell::RefCell::new(Some(tx)) }
    }

    fn send(&self, value: Value) {
        if let Some(tx) = self.tx.borrow_mut().take() {
            let _ = tx.send(Ok(json!({ self.key: value })));
        }
    }
}

impl Drop for Answer {
    fn drop(&mut self) {
        self.send(Value::Null);
    }
}

/// Every plugin's status, for `plugin-status` (what the manager shows).
/// A plugin that keeps exiting is trouble even while Lumen restarts it.
pub(super) fn plugin_status_json(list: &[crate::plugins::Info]) -> Value {
    let plugins: Vec<Value> = list
        .iter()
        .map(|p| {
            let trouble = plugin_dot(&p.status) == Dot::Bad || p.status.starts_with("exited");
            json!({"name": p.manifest.name, "status": p.status, "trouble": trouble})
        })
        .collect();
    json!({ "plugins": plugins })
}

pub(super) fn plugin_dot(status: &str) -> Dot {
    match status {
        "running" => Dot::Good,
        "starting" | "waiting for your approval" => Dot::Busy,
        s if s.contains("restarting") => Dot::Busy,
        _ => Dot::Bad,
    }
}

/// "a", "a and b", "a, b and c".
fn join_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_status_lists_every_plugin() {
        let info = |name: &str, status: &str| crate::plugins::Info {
            manifest: {
                let mut m = crate::plugins::parse_manifest(std::path::Path::new(name), "command = \"x\"").unwrap();
                m.name = name.into();
                m
            },
            status: status.into(),
            commands: Vec::new(),
            started: true,
        };
        let v = plugin_status_json(&[
            info("mira/x", "exited with status 1"),
            info("recent-dirs", "running"),
            info("loop", "exited (exit status: 1), restarting in 5s"),
            info("new", "starting"),
        ]);
        assert_eq!(
            v,
            json!({"plugins": [
                {"name": "mira/x", "status": "exited with status 1", "trouble": true},
                {"name": "recent-dirs", "status": "running", "trouble": false},
                {"name": "loop", "status": "exited (exit status: 1), restarting in 5s", "trouble": true},
                {"name": "new", "status": "starting", "trouble": false},
            ]})
        );
    }
}
