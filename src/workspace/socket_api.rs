//! The socket API's requests, answered by the window: finding the pane meant, listing panes, and each method.

use super::*;

impl Workspace {
    fn find_pane(&self, id: u64) -> Option<(usize, Entity<TerminalView>)> {
        self.tabs
            .iter()
            .enumerate()
            .find_map(|(i, t)| t.leaves().into_iter().find(|l| l.entity_id().as_u64() == id).map(|l| (i, l)))
    }

    /// The pane named by `params.pane`, or the focused one.
    fn target_pane(&self, params: &Value) -> Result<(usize, Entity<TerminalView>), ApiError> {
        match params.get("pane") {
            None | Some(Value::Null) => self
                .active_terminal()
                .map(|t| (self.active_tab, t.clone()))
                .ok_or_else(|| ApiError::failed("there are no panes")),
            Some(v) => {
                let id = v.as_u64().ok_or_else(|| ApiError::invalid_params("`pane` should be a pane id"))?;
                self.find_pane(id).ok_or_else(|| ApiError::failed(format!("no pane {id}; `lumen msg list` shows them")))
            }
        }
    }

    /// Whether pane `id` is in this window.
    pub fn has_pane(&self, id: u64) -> bool {
        self.find_pane(id).is_some()
    }

    pub fn api_list(&self, cx: &App) -> Value {
        let focused = self.active_terminal().map(|t| t.entity_id());
        let tabs: Vec<Value> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let panes: Vec<Value> = tab
                    .leaves()
                    .iter()
                    .map(|p| {
                        let view = p.read(cx);
                        let (cols, rows) = view.grid_size();
                        json!({
                            "id": p.entity_id().as_u64(),
                            "title": view.title,
                            "cwd": view.cwd(),
                            "active": *p == tab.active,
                            "focused": Some(p.entity_id()) == focused,
                            "cols": cols,
                            "rows": rows,
                            "last_exit": view.last_exit(),
                            "plugin_pane": view.is_plugin_pane(),
                            "label": view.label,
                            "color": view.label_color.map(TabColor::name),
                        })
                    })
                    .collect();
                json!({
                    "index": i,
                    "title": tab.title.clone().unwrap_or_else(|| tab.active.read(cx).title.clone()),
                    "custom_title": tab.title.is_some(),
                    "active": i == self.active_tab,
                    "zoomed": tab.zoomed,
                    "badge": tab.badge,
                    "short_badge": tab.short_badge,
                    "color": shown_tab_color(tab.own_color, tab.color).map(TabColor::name),
                    "panes": panes,
                })
            })
            .collect();
        json!({ "tabs": tabs, "focused_pane": focused.map(|f| f.as_u64()) })
    }

    /// Run one socket API request. Methods and their parameters are described
    /// in docs/scripting.md.
    pub fn handle_api(
        &mut self,
        caller: &api::Caller,
        method: &str,
        params: &Value,
        origin: Option<&api::Origin>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ApiResult {
        let string = |name: &str| -> Result<Option<String>, ApiError> {
            match params.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) => Ok(Some(s.clone())),
                Some(_) => Err(ApiError::invalid_params(format!("`{name}` should be a string"))),
            }
        };
        let required =
            |name: &str| string(name)?.ok_or_else(|| ApiError::invalid_params(format!("`{name}` is required")));
        let cwd = || -> Result<Option<std::path::PathBuf>, ApiError> {
            match string("cwd")? {
                None => Ok(None),
                Some(dir) => {
                    let path = std::path::PathBuf::from(&dir);
                    if path.is_dir() {
                        Ok(Some(path))
                    } else {
                        Err(ApiError::failed(format!("{dir} isn't a directory")))
                    }
                }
            }
        };
        // `plugin_pane`: a pane the caller draws, instead of a shell.
        let owner = || -> Result<Option<Owner>, ApiError> {
            match params.get("plugin_pane") {
                None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(None),
                Some(Value::Bool(true)) => {
                    if params.get("run").is_some_and(|r| !r.is_null())
                        || params.get("cwd").is_some_and(|c| !c.is_null())
                    {
                        return Err(ApiError::invalid_params(
                            "a plugin pane runs no program, so takes no `run` or `cwd`",
                        ));
                    }
                    let origin =
                        origin.ok_or_else(|| ApiError::failed("plugin panes need a connection to draw them"))?;
                    let name = match caller {
                        api::Caller::Plugin { name, .. } => name.clone(),
                        api::Caller::Socket => "script".into(),
                    };
                    Ok(Some(Owner { conn: origin.conn, out: origin.out.clone(), name }))
                }
                Some(_) => Err(ApiError::invalid_params("`plugin_pane` should be true or false")),
            }
        };
        // Typed into a new pane's shell, which reads it once it's ready.
        let run = |term: &Entity<TerminalView>, cx: &App| -> Result<(), ApiError> {
            if let Some(cmd) = string("run")? {
                term.read(cx).send_text(&format!("{cmd}\r"));
            }
            Ok(())
        };

        match method {
            "hello" => Ok(json!({
                "protocol": api::PROTOCOL,
                "version": env!("CARGO_PKG_VERSION"),
                "pid": std::process::id(),
                "events": api::EVENTS,
                "actions": keymap::ACTIONS,
            })),
            "list" => Ok(self.api_list(cx)),
            "plugin-status" => Ok(plugin_status_json(&crate::plugins::list())),
            "new-tab" => {
                let what = match owner()? {
                    Some(owner) => NewPane::Plugin(owner),
                    None => NewPane::Shell(cwd()?.or_else(|| self.current_cwd(cx))),
                };
                string("run")?;
                let term = self.open_tab(what, window, cx);
                run(&term, cx)?;
                Ok(json!({ "pane": term.entity_id().as_u64(), "tab": self.active_tab }))
            }
            "split" => {
                let axis = match string("direction")?.as_deref() {
                    None | Some("right") => Axis::Row,
                    Some("down") => Axis::Column,
                    Some(other) => {
                        return Err(ApiError::invalid_params(format!(
                            "`direction` should be \"right\" or \"down\", not \"{other}\""
                        )));
                    }
                };
                let focus = match params.get("focus") {
                    None | Some(Value::Null) => true,
                    Some(v) => {
                        v.as_bool().ok_or_else(|| ApiError::invalid_params("`focus` should be true or false"))?
                    }
                };
                // The new pane's share of the space: a fraction, or a number
                // of rows (down) or columns (right) of the pane split.
                let share = match params.get("size") {
                    None | Some(Value::Null) => None,
                    Some(v) => {
                        let n = v.as_f64().filter(|n| *n > 0.0).ok_or_else(|| {
                            ApiError::invalid_params(
                                "`size` should be a fraction (like 0.3) or a number of rows or columns",
                            )
                        })?;
                        Some(n)
                    }
                };
                let owner = owner()?;
                string("run")?;
                let (_, target) = self.target_pane(params)?;
                let what = match owner {
                    Some(owner) => NewPane::Plugin(owner),
                    None => NewPane::Shell(cwd()?.or_else(|| target.read(cx).cwd())),
                };
                let (cols, rows) = target.read(cx).grid_size();
                let new = self
                    .split_pane(target.entity_id(), axis, what, focus, window, cx)
                    .ok_or_else(|| ApiError::failed("that pane just closed"))?;
                if let Some(n) = share {
                    let whole = if axis == Axis::Row { cols } else { rows } as f64;
                    // The new pane's frame and the divider take about two
                    // rows (or columns) of what it's given.
                    let fraction = if n < 1.0 { n } else { (n + 2.0) / whole.max(1.0) };
                    let id = new.entity_id();
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.leaves().iter().any(|l| l.entity_id() == id)) {
                        tab.root.set_share(id, fraction.clamp(0.1, 0.9) as f32);
                    }
                }
                run(&new, cx)?;
                Ok(json!({ "pane": new.entity_id().as_u64() }))
            }
            "focus" => {
                let (tab, pane) = self.target_pane(params)?;
                self.tabs[tab].active = pane;
                self.active_tab = tab;
                self.focus_active(window, cx);
                window.activate_window();
                cx.notify();
                Ok(json!({}))
            }
            "close" => {
                let (_, pane) = self.target_pane(params)?;
                self.close_pane_entity(pane.entity_id(), window, cx);
                Ok(json!({}))
            }
            "send-text" => {
                let text = required("text")?;
                let (_, pane) = self.target_pane(params)?;
                pane.read(cx).send_text(&text);
                Ok(json!({}))
            }
            "read-screen" => {
                let lines = match params.get("lines") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(
                        v.as_u64()
                            .filter(|n| *n > 0)
                            .ok_or_else(|| ApiError::invalid_params("`lines` should be a positive number"))?
                            as usize,
                    ),
                };
                let (_, pane) = self.target_pane(params)?;
                Ok(json!({ "text": pane.read(cx).screen_text(lines) }))
            }
            "move-tab" => {
                let to = params
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| ApiError::invalid_params("`index` (a tab position, from 0) is required"))?
                    as usize;
                let (from, _) = self.target_pane(params)?;
                if to >= self.tabs.len() {
                    return Err(ApiError::invalid_params(format!("there are only {} tabs", self.tabs.len())));
                }
                self.move_tab(from, to, window, cx);
                Ok(json!({}))
            }
            "set-title" => {
                let title = string("title")?.filter(|t| !t.trim().is_empty());
                let (tab, _) = self.target_pane(params)?;
                self.tabs[tab].title = title;
                cx.notify();
                Ok(json!({}))
            }
            "set-tab-style" => {
                let (tab, _) = self.target_pane(params)?;
                // Fields left out stay as they are; null clears them.
                let badge = match params.get("badge") {
                    None => self.tabs[tab].badge.clone(),
                    Some(Value::Null) => None,
                    Some(Value::String(b)) => {
                        let b = b.trim();
                        if b.chars().count() > 24 {
                            return Err(ApiError::invalid_params("`badge` should be at most 24 characters"));
                        }
                        (!b.is_empty()).then(|| b.to_string())
                    }
                    Some(_) => return Err(ApiError::invalid_params("`badge` should be a string or null")),
                };
                // A new badge drops the old short form unless it brings one.
                let short_badge = match params.get("short_badge") {
                    None if params.get("badge").is_some() => None,
                    None => self.tabs[tab].short_badge.clone(),
                    Some(Value::Null) => None,
                    Some(Value::String(b)) => {
                        let b = b.trim();
                        if b.chars().count() > 8 {
                            return Err(ApiError::invalid_params("`short_badge` should be at most 8 characters"));
                        }
                        (!b.is_empty()).then(|| b.to_string())
                    }
                    Some(_) => return Err(ApiError::invalid_params("`short_badge` should be a string or null")),
                };
                let short_badge = short_badge.filter(|_| badge.is_some());
                let color = match params.get("color") {
                    None => self.tabs[tab].color,
                    Some(Value::Null) => None,
                    Some(Value::String(c)) => Some(TabColor::parse(c).map_err(ApiError::invalid_params)?),
                    Some(_) => return Err(ApiError::invalid_params("`color` should be a string or null")),
                };
                let owner = match caller {
                    api::Caller::Plugin { .. } => origin.map(|o| o.conn),
                    api::Caller::Socket => None,
                };
                let t = &mut self.tabs[tab];
                t.style_owner = if badge.is_none() && color.is_none() { None } else { owner };
                t.badge = badge;
                t.short_badge = short_badge;
                t.color = color;
                cx.notify();
                Ok(json!({}))
            }
            "set-pane-style" => {
                let (_, pane) = self.target_pane(params)?;
                let (label, color) = {
                    let view = pane.read(cx);
                    parse_pane_style(params, view.label.clone(), view.label_color)?
                };
                let owner = match caller {
                    api::Caller::Plugin { .. } => origin.map(|o| o.conn),
                    api::Caller::Socket => None,
                };
                pane.update(cx, |view, _| {
                    view.style_owner = pane_style_owner(&label, &color, owner);
                    view.label = label;
                    view.label_color = color;
                });
                cx.notify();
                Ok(json!({}))
            }
            "write" => {
                let text = required("text")?;
                let (_, pane) = self.target_pane(params)?;
                match pane.read(cx).owner_conn() {
                    None => Err(ApiError::failed(
                        "that pane runs a program; `write` is for plugin panes (`send-text` types into it)",
                    )),
                    Some(conn) if Some(conn) != origin.map(|o| o.conn) => {
                        Err(ApiError::failed("that plugin pane belongs to another connection"))
                    }
                    Some(_) => {
                        pane.update(cx, |view, _| view.feed(&text));
                        Ok(json!({}))
                    }
                }
            }
            // A connection closed: so do the plugin panes it drew.
            "_disconnected" => {
                let conn = params.get("conn").and_then(Value::as_u64);
                let orphans: Vec<EntityId> = self
                    .tabs
                    .iter()
                    .flat_map(|t| t.leaves())
                    .filter(|l| conn.is_some() && l.read(cx).owner_conn() == conn)
                    .map(|l| l.entity_id())
                    .collect();
                for id in orphans {
                    self.close_pane_entity(id, window, cx);
                }
                for tab in &mut self.tabs {
                    if conn.is_some() && tab.style_owner == conn {
                        (tab.badge, tab.short_badge, tab.color, tab.style_owner) = (None, None, None, None);
                    }
                }
                let styled: Vec<_> = self
                    .tabs
                    .iter()
                    .flat_map(|t| t.leaves())
                    .filter(|l| clears_on_disconnect(l.read(cx).style_owner, conn))
                    .collect();
                for pane in styled {
                    pane.update(cx, |view, _| {
                        (view.label, view.label_color, view.style_owner) = (None, None, None);
                    });
                }
                cx.notify();
                Ok(json!({}))
            }
            "run-action" => {
                let name = required("action")?;
                let name = keymap::check_action(&name)
                    .map_err(ApiError::invalid_params)?
                    .ok_or_else(|| ApiError::invalid_params("\"none\" isn't an action"))?;
                let action = keymap::action(&name).expect("checked above");
                if params.get("pane").is_some_and(|p| !p.is_null()) {
                    let (tab, pane) = self.target_pane(params)?;
                    self.tabs[tab].active = pane;
                    self.active_tab = tab;
                    self.focus_active(window, cx);
                }
                window.dispatch_action(action, cx);
                Ok(json!({}))
            }
            "register-command" => {
                let result = crate::plugins::register_command(caller, params)?;
                keymap::apply(cx);
                Ok(result)
            }
            // A plugin stopped; its commands and keys are gone.
            "_plugin-commands-changed" => {
                keymap::apply(cx);
                Ok(json!({}))
            }
            other => Err(ApiError(-32601, format!("unknown method \"{other}\""))),
        }
    }
}

/// The `placeholder`, `initial` text and `message` of a `show-prompt`: each
/// a string, or left out (null too).
pub(super) fn parse_prompt(params: &Value) -> Result<(String, String, Option<String>), ApiError> {
    let get = |key: &str| match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(ApiError::invalid_params(format!("`{key}` should be a string"))),
    };
    let placeholder = get("placeholder")?.unwrap_or("Type here…").to_string();
    let initial = get("initial")?.unwrap_or_default().to_string();
    let message = get("message")?.map(str::trim).filter(|m| !m.is_empty()).map(str::to_string);
    Ok((placeholder, initial, message))
}

/// A pane's label and colour after a `set-pane-style` request: fields left
/// out stay as they are; null (or a blank label) clears them.
fn parse_pane_style(
    params: &Value,
    label: Option<String>,
    color: Option<TabColor>,
) -> Result<(Option<String>, Option<TabColor>), ApiError> {
    let label = match params.get("label") {
        None => label,
        Some(Value::Null) => None,
        Some(Value::String(l)) => {
            let l = l.trim();
            if l.chars().count() > 32 {
                return Err(ApiError::invalid_params("`label` should be at most 32 characters"));
            }
            (!l.is_empty()).then(|| l.to_string())
        }
        Some(_) => return Err(ApiError::invalid_params("`label` should be a string or null")),
    };
    let color = match params.get("color") {
        None => color,
        Some(Value::Null) => None,
        Some(Value::String(c)) => Some(TabColor::parse(c).map_err(ApiError::invalid_params)?),
        Some(_) => return Err(ApiError::invalid_params("`color` should be a string or null")),
    };
    Ok((label, color))
}

/// Who owns a pane's style after a `set-pane-style`: the caller's plugin
/// connection (`None` for a socket client), or nobody once both fields are
/// cleared. The last writer wins.
fn pane_style_owner(label: &Option<String>, color: &Option<TabColor>, caller: Option<u64>) -> Option<u64> {
    if label.is_none() && color.is_none() { None } else { caller }
}

/// Whether a closed connection's disconnect clears this pane's style: only
/// styles that connection set.
fn clears_on_disconnect(style_owner: Option<u64>, conn: Option<u64>) -> bool {
    conn.is_some() && style_owner == conn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_styles_keep_clear_and_check() {
        let cur = (Some("db".to_string()), Some(TabColor::Ansi(2)));
        let p = |v: Value| parse_pane_style(&v, cur.0.clone(), cur.1);
        assert_eq!(p(json!({})).unwrap(), cur); // left out stays
        assert_eq!(p(json!({"label": null})).unwrap(), (None, cur.1)); // null clears
        assert_eq!(p(json!({"label": "  "})).unwrap(), (None, cur.1)); // blank clears
        assert_eq!(
            p(json!({"label": " web ", "color": "red"})).unwrap(),
            (Some("web".into()), Some(TabColor::Ansi(1)))
        );
        assert!(p(json!({"label": "x".repeat(33)})).is_err());
        assert!(p(json!({"color": "orange"})).is_err());
        assert!(p(json!({"label": 3})).is_err());
    }

    #[test]
    fn prompt_params_are_strings() {
        let ok = parse_prompt(&json!({})).unwrap();
        assert_eq!(ok, ("Type here…".to_string(), String::new(), None));
        let ok =
            parse_prompt(&json!({"placeholder": "Command", "initial": "npm ", "message": "  Runs it.  "})).unwrap();
        assert_eq!(ok, ("Command".to_string(), "npm ".to_string(), Some("Runs it.".to_string())));
        // A blank message is none; null counts as left out.
        assert_eq!(parse_prompt(&json!({"message": " ", "initial": null})).unwrap().2, None);
        assert!(parse_prompt(&json!({"placeholder": 3})).is_err());
        assert!(parse_prompt(&json!({"initial": ["a"]})).is_err());
        assert!(parse_prompt(&json!({"message": true})).is_err());
    }

    #[test]
    fn pane_style_ownership_and_disconnect() {
        let (label, color) = (Some("db".to_string()), Some(TabColor::Ansi(2)));
        // A plugin's connection owns what it styled; a socket client owns nothing.
        assert_eq!(pane_style_owner(&label, &None, Some(7)), Some(7));
        assert_eq!(pane_style_owner(&None, &color, Some(7)), Some(7));
        assert_eq!(pane_style_owner(&label, &color, None), None);
        // Clearing both fields resets the owner.
        assert_eq!(pane_style_owner(&None, &None, Some(7)), None);
        // Last writer wins.
        assert_eq!(pane_style_owner(&label, &color, Some(8)), Some(8));
        // Disconnect clears only that connection's panes.
        assert!(clears_on_disconnect(Some(7), Some(7)));
        assert!(!clears_on_disconnect(Some(8), Some(7)));
        assert!(!clears_on_disconnect(None, Some(7))); // socket client's styles stay
        assert!(!clears_on_disconnect(None, None));
        assert!(!clears_on_disconnect(Some(7), None));
    }
}
