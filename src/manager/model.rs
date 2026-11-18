//! The manager's state, and what each key does to it. Keys that change
//! files return [`Effect`]s for the runner to carry out off the screen's
//! thread; nothing here touches the disk or the network.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::manager::data::{self, Attention, Item, Kind, Snapshot};
use crate::manager::settings_edit::{Editor, EditorResult};
use crate::plugins::{Manifest, Permission};
use crate::registry::{self, Entry};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tab {
    Installed,
    Browse,
    Updates,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Installed, Tab::Browse, Tab::Updates];
}

/// Something the runner does to files (or the network), then hands back a
/// fresh [`Snapshot`].
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Install `name`; `allowed` is what you were asked about and allowed.
    /// If the plugin now needs more (the index changed meanwhile), nothing
    /// is installed.
    Install { name: String, allowed: Vec<Permission> },
    Update { name: String, allowed: Vec<Permission> },
    /// Update each of these plugins (the ones shown with updates), with
    /// what was allowed for each.
    UpdateAll(Vec<(String, Vec<Permission>)>),
    Remove { name: String, delete_data: bool },
    /// Turn a plugin on or off; `approve` is a built-in's manifest whose
    /// permissions you just allowed.
    SetEnabled { name: String, on: bool, approve: Option<Manifest> },
    /// Set (or with `None`, reset) one of a plugin's settings.
    SaveSetting { plugin: String, key: String, value: Option<serde_json::Value> },
    RefreshIndex,
    Quit,
}

/// A question over the list.
#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    /// Allow `name` to use `permissions`, then do `then`.
    Approve { name: String, permissions: Vec<Permission>, then: Box<Effect> },
    /// Removing `name`: keep its data, or delete it too?
    RemoveData { name: String },
    /// The keys and what they do.
    Keys,
}

/// One line of the list: a plugin you have, or one in the registry.
#[derive(Clone, Copy, Debug)]
pub enum Row<'a> {
    Item(&'a Item),
    Entry(&'a Entry),
}

impl Row<'_> {
    pub fn name(&self) -> &str {
        match self {
            Row::Item(i) => &i.name,
            Row::Entry(e) => &e.name,
        }
    }
}

pub struct Model {
    pub tab: Tab,
    pub selected: usize,
    /// Text the list is narrowed to.
    pub filter: Option<String>,
    /// Keys go into the filter rather than acting.
    pub typing: bool,
    /// Details fill the screen (on a narrow terminal).
    pub details: bool,
    pub dialog: Option<Dialog>,
    /// The last thing that happened, or went wrong.
    pub status: Option<String>,
    /// What the runner is doing now; most keys wait for it.
    pub busy: Option<String>,
    /// Why the index couldn't be loaded, when it couldn't.
    pub index_error: Option<String>,
    /// A plugin's settings, being edited.
    pub editing: Option<Editor>,
    pub snapshot: Snapshot,
    /// [`data::installed`] and [`data::updates`] of the snapshot.
    pub items: Vec<Item>,
    pub updates: Vec<Item>,
}

impl Model {
    pub fn new(snapshot: Snapshot) -> Model {
        let items = data::installed(&snapshot);
        let updates = items.iter().filter(|i| i.update.is_some()).cloned().collect();
        Model {
            tab: Tab::Installed,
            selected: 0,
            filter: None,
            typing: false,
            details: false,
            dialog: None,
            status: None,
            busy: None,
            index_error: None,
            editing: None,
            snapshot,
            items,
            updates,
        }
    }

    /// The list for the current tab, narrowed by the filter.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let text = self.filter.as_deref().unwrap_or("").trim().to_lowercase();
        let matches = |i: &Item| {
            text.is_empty()
                || i.name.to_lowercase().contains(&text)
                || i.title.to_lowercase().contains(&text)
                || i.description.to_lowercase().contains(&text)
        };
        match self.tab {
            Tab::Installed => self.items.iter().filter(|i| matches(i)).map(Row::Item).collect(),
            Tab::Updates => self.updates.iter().filter(|i| matches(i)).map(Row::Item).collect(),
            Tab::Browse => data::browse(&self.snapshot, &text).into_iter().map(Row::Entry).collect(),
        }
    }

    pub fn selected_row(&self) -> Option<Row<'_>> {
        self.rows().get(self.selected).copied()
    }

    /// The count shown beside a tab's name, if it has one.
    pub fn count(&self, tab: Tab) -> Option<usize> {
        match tab {
            Tab::Installed => Some(self.items.len()),
            Tab::Updates => Some(self.updates.len()),
            Tab::Browse => None,
        }
    }

    fn set_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.editing = None;
            self.tab = tab;
            self.selected = 0;
            self.details = false;
        }
    }

    fn step(&mut self, by: i64) {
        let last = self.rows().len().saturating_sub(1) as i64;
        self.selected = (self.selected as i64 + by).clamp(0, last) as usize;
    }

    pub fn scroll(&mut self, lines: i32) {
        self.step(i64::from(lines));
    }

    pub fn click(&mut self, row: Option<usize>, tab: Option<Tab>) {
        if self.dialog.is_some() {
            return;
        }
        if let Some(tab) = tab {
            self.set_tab(tab);
        }
        if let Some(row) = row.filter(|r| *r < self.rows().len()) {
            self.selected = row;
        }
    }

    /// The index loaded in the background (when the manager opened): a
    /// fresh snapshot, and only a problem worth saying. What you started
    /// is still running, so `busy` stays.
    pub fn loaded(&mut self, result: Result<String, String>, snapshot: Snapshot) {
        let (busy, status) = (self.busy.take(), self.status.take());
        self.done(result.clone(), snapshot);
        self.busy = busy;
        if result.is_ok() {
            self.status = status;
        }
    }

    /// The runner finished an effect.
    pub fn done(&mut self, result: Result<String, String>, snapshot: Snapshot) {
        let name = self.selected_row().map(|r| r.name().to_string());
        self.status = Some(match result {
            Ok(m) | Err(m) => m,
        })
        .filter(|s| !s.is_empty());
        self.busy = None;
        self.items = data::installed(&snapshot);
        self.updates = self.items.iter().filter(|i| i.update.is_some()).cloned().collect();
        self.snapshot = snapshot;
        if let Some(i) = name.and_then(|n| self.rows().iter().position(|r| r.name() == n)) {
            self.selected = i;
        }
        self.step(0);
    }

    pub fn key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if let Some(dialog) = self.dialog.take() {
            return self.answer(dialog, key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return vec![Effect::Quit];
        }
        if let Some(editor) = self.editing.as_mut() {
            return match editor.key(key) {
                EditorResult::Stay => Vec::new(),
                EditorResult::Close => {
                    self.editing = None;
                    Vec::new()
                }
                EditorResult::Problem(p) => self.say(p),
                EditorResult::Save(..) if self.busy.is_some() => {
                    let busy = self.busy.clone().unwrap_or_default();
                    self.say(format!("Wait for \"{busy}\" to finish"))
                }
                EditorResult::Save(key, value) => {
                    let plugin = editor.plugin.clone();
                    self.status = None;
                    self.start(Effect::SaveSetting { plugin, key, value })
                }
            };
        }
        if self.typing {
            match key.code {
                KeyCode::Char(c) => self.filter.get_or_insert_default().push(c),
                KeyCode::Backspace => {
                    self.filter.get_or_insert_default().pop();
                }
                KeyCode::Enter | KeyCode::Down | KeyCode::Up => self.typing = false,
                KeyCode::Esc => {
                    self.typing = false;
                    self.filter = None;
                }
                _ => {}
            }
            self.selected = 0;
            return Vec::new();
        }
        match key.code {
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Esc => {
                if self.details {
                    self.details = false;
                } else if self.filter.is_some() {
                    self.filter = None;
                    self.selected = 0;
                } else {
                    return vec![Effect::Quit];
                }
            }
            KeyCode::Tab => self.set_tab(Tab::ALL[(self.tab as usize + 1) % 3]),
            KeyCode::BackTab => self.set_tab(Tab::ALL[(self.tab as usize + 2) % 3]),
            KeyCode::Char(c @ '1'..='3') => self.set_tab(Tab::ALL[c as usize - '1' as usize]),
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::PageDown => self.step(10),
            KeyCode::PageUp => self.step(-10),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.step(i64::MAX / 2),
            KeyCode::Enter => self.details = !self.details,
            KeyCode::Char('/') => {
                self.typing = true;
                self.filter.get_or_insert_default();
            }
            KeyCode::Char('?') => self.dialog = Some(Dialog::Keys),
            KeyCode::Char('s') => {
                let item = match self.selected_row() {
                    Some(Row::Item(i)) => Some(i.clone()),
                    Some(Row::Entry(e)) => self.items.iter().find(|i| i.name == e.name).cloned(),
                    None => None,
                };
                match item {
                    Some(i) => match Editor::new(&i) {
                        Some(e) => self.editing = Some(e),
                        None => return self.say(format!("{} has no settings", i.name)),
                    },
                    None => return self.say("Install it first to change its settings"),
                }
            }
            KeyCode::Char(c @ (' ' | 'i' | 'u' | 'U' | 'x' | 'r')) => return self.act(c),
            _ => {}
        }
        Vec::new()
    }

    fn answer(&mut self, dialog: Dialog, key: KeyEvent) -> Vec<Effect> {
        match (dialog, key.code) {
            (Dialog::Approve { then, .. }, KeyCode::Char('y') | KeyCode::Enter) => self.start(*then),
            (Dialog::Approve { .. }, KeyCode::Char('n') | KeyCode::Esc) => Vec::new(),
            (Dialog::RemoveData { name }, KeyCode::Char('y')) => self.start(Effect::Remove { name, delete_data: true }),
            (Dialog::RemoveData { name }, KeyCode::Char('n') | KeyCode::Enter) => {
                self.start(Effect::Remove { name, delete_data: false })
            }
            (Dialog::RemoveData { .. }, KeyCode::Esc) | (Dialog::Keys, _) => Vec::new(),
            (dialog, _) => {
                self.dialog = Some(dialog);
                Vec::new()
            }
        }
    }

    /// Hand `effect` to the runner, noting what's happening.
    fn start(&mut self, effect: Effect) -> Vec<Effect> {
        self.busy = match &effect {
            Effect::Install { name, .. } => Some(format!("Installing {name}…")),
            Effect::Update { name, .. } => Some(format!("Updating {name}…")),
            Effect::UpdateAll(_) => Some("Updating plugins…".into()),
            Effect::Remove { name, .. } => Some(format!("Removing {name}…")),
            Effect::SetEnabled { name, on, .. } => Some(format!("Turning {name} {}…", if *on { "on" } else { "off" })),
            Effect::SaveSetting { plugin, .. } => Some(format!("Saving {plugin}'s settings…")),
            Effect::RefreshIndex => Some("Refreshing the plugin index…".into()),
            Effect::Quit => None,
        };
        vec![effect]
    }

    /// Ask about `permissions` first, unless there are none.
    fn ask(&mut self, name: &str, permissions: Vec<Permission>, then: Effect) -> Vec<Effect> {
        if permissions.is_empty() {
            return self.start(then);
        }
        self.dialog = Some(Dialog::Approve { name: name.into(), permissions, then: Box::new(then) });
        Vec::new()
    }

    fn say(&mut self, text: impl Into<String>) -> Vec<Effect> {
        self.status = Some(text.into());
        Vec::new()
    }

    fn act(&mut self, c: char) -> Vec<Effect> {
        if let Some(busy) = &self.busy {
            return self.say(format!("Wait for \"{busy}\" to finish"));
        }
        match c {
            'U' => return self.update_all(),
            'r' => return self.start(Effect::RefreshIndex),
            _ => {}
        }
        let Some(row) = self.selected_row() else { return Vec::new() };
        let (name, item) = match row {
            Row::Item(i) => (i.name.clone(), Some(i.clone())),
            Row::Entry(e) => (e.name.clone(), self.items.iter().find(|i| i.name == e.name).cloned()),
        };
        let needs_install =
            item.as_ref().is_none_or(|i| matches!(i.attention, Some(Attention::NotInstalled | Attention::NotApproved)));
        match (c, item) {
            ('i', _) | (' ', None) if needs_install => self.install(&name),
            ('i', _) | (' ', None) => self.say(format!("{name} is installed already")),
            (' ', Some(i)) if needs_install && i.kind == Kind::Registry => self.install(&name),
            (' ', Some(i)) => match i.kind {
                Kind::Local => {
                    self.say("Local plugins are always on; move its folder out of ~/.config/lumen/plugins to turn it off")
                }
                Kind::BuiltIn if !i.on => {
                    // Approved before (and unchanged since): no need to ask again.
                    let approved = i.manifest.as_ref().is_some_and(|m| {
                        crate::plugins::decision_at(&self.snapshot.approvals, m) == crate::plugins::Decision::Allowed
                    });
                    if approved {
                        return self.start(Effect::SetEnabled { name, on: true, approve: None });
                    }
                    let approve = i.manifest.clone().filter(|m| !m.permissions.is_empty());
                    let effect = Effect::SetEnabled { name: name.clone(), on: true, approve };
                    self.ask(&name, i.permissions.clone(), effect)
                }
                _ => self.start(Effect::SetEnabled { name, on: !i.on, approve: None }),
            },
            ('u', Some(Item { update: Some(up), .. })) => {
                let effect = Effect::Update { name: name.clone(), allowed: up.new_permissions.clone() };
                self.ask(&name, up.new_permissions, effect)
            }
            ('u', _) => self.say(format!("{name} is up to date")),
            // Built-ins come with Lumen: off is as removed as they get.
            ('x', Some(i)) if i.kind == Kind::BuiltIn && i.on => {
                self.start(Effect::SetEnabled { name, on: false, approve: None })
            }
            ('x', Some(i)) if i.kind == Kind::BuiltIn => {
                self.say(format!("{name} is built in, so it can't be removed; it's off already"))
            }
            ('x', Some(i)) if i.kind == Kind::Local => {
                self.say("Local plugins are removed by deleting their folder from ~/.config/lumen/plugins")
            }
            ('x', Some(i)) if i.attention != Some(Attention::NotInstalled) => {
                self.dialog = Some(Dialog::RemoveData { name });
                Vec::new()
            }
            // Listed in config.toml but not installed: take it off the list.
            ('x', Some(i)) if i.on => self.start(Effect::SetEnabled { name, on: false, approve: None }),
            ('x', _) => self.say(format!("{name} isn't installed")),
            _ => Vec::new(),
        }
    }

    fn install(&mut self, name: &str) -> Vec<Effect> {
        let Some(release) = self
            .snapshot
            .index
            .as_ref()
            .and_then(|i| i.plugins.iter().find(|e| e.name == name))
            .and_then(|e| registry::best(e, registry::LUMEN_VERSION))
        else {
            return self.say(format!("{name} isn't in the plugin index for this Lumen; press r to refresh it"));
        };
        let allowed = crate::plugins::registry_approval_in(&self.snapshot.approvals, name);
        let new: Vec<Permission> = release.permissions.iter().copied().filter(|p| !allowed.contains(p)).collect();
        self.ask(name, new.clone(), Effect::Install { name: name.into(), allowed: new })
    }

    fn update_all(&mut self) -> Vec<Effect> {
        if self.updates.is_empty() {
            return self.say("Everything is up to date");
        }
        let mut names = Vec::new();
        let mut permissions: Vec<Permission> = Vec::new();
        let mut each = Vec::new();
        for i in &self.updates {
            let new = i.update.as_ref().map(|u| u.new_permissions.clone()).unwrap_or_default();
            if !new.is_empty() {
                names.push(i.name.clone());
                permissions.extend(&new);
            }
            each.push((i.name.clone(), new));
        }
        permissions.sort();
        permissions.dedup();
        self.ask(&names.join(", "), permissions, Effect::UpdateAll(each))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::data::{Attention, Item, Kind, Snapshot, Upgrade};
    use crate::plugins::Permission;
    use crate::registry::testing::{entry, release};
    use crate::registry::Index;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(m: &mut Model, code: KeyCode) -> Vec<Effect> {
        m.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ch(m: &mut Model, c: char) -> Vec<Effect> {
        key(m, KeyCode::Char(c))
    }

    fn typed(m: &mut Model, text: &str) {
        for c in text.chars() {
            ch(m, c);
        }
    }

    fn item(name: &str, kind: Kind, on: bool, perms: Vec<Permission>) -> Item {
        let mut manifest = crate::plugins::parse_manifest(std::path::Path::new(name), "command = \"x\"\n").unwrap();
        manifest.permissions = perms.clone();
        manifest.builtin = kind == Kind::BuiltIn;
        Item {
            name: name.into(),
            title: name.into(),
            kind,
            on,
            version: Some("1.0.0".into()),
            update: None,
            attention: None,
            description: String::new(),
            permissions: perms,
            manifest: Some(manifest),
            entry: None,
            settings: Vec::new(),
            values: Default::default(),
        }
    }

    /// The registry offers `mira/docker` (needs panes, 9 stars), `mira/x`
    /// (needs nothing, 5 stars) and `b/git` (1 star).
    fn model() -> Model {
        let mut docker = entry("mira/docker", 9, &[]);
        docker.description = "containers in panes".into();
        docker.versions = vec![release("1.0.0", vec![Permission::Panes], String::new())];
        let mut x = entry("mira/x", 5, &[]);
        x.versions = vec![release("1.0.0", vec![], String::new())];
        let mut git = entry("b/git", 1, &[]);
        git.versions = vec![release("1.0.0", vec![], String::new())];
        let snapshot = Snapshot { index: Some(Index { version: 1, plugins: vec![docker, x, git] }), ..Default::default() };
        let mut m = Model::new(snapshot);
        m.items = vec![
            item("recent-dirs", Kind::BuiltIn, true, vec![]),
            item("git-branch", Kind::BuiltIn, false, vec![]),
            item("ssh-hosts", Kind::BuiltIn, false, vec![Permission::Panes]),
            item("where", Kind::Local, true, vec![]),
            item("mira/on", Kind::Registry, true, vec![]),
        ];
        m
    }

    fn select(m: &mut Model, name: &str) {
        m.selected = m.rows().iter().position(|r| r.name() == name).unwrap();
    }

    fn with_updates(m: &mut Model) {
        let mut a = item("mira/a", Kind::Registry, true, vec![]);
        a.update = Some(Upgrade { version: "1.1.0".into(), new_permissions: vec![] });
        let mut b = item("mira/b", Kind::Registry, true, vec![]);
        b.update = Some(Upgrade { version: "2.0.0".into(), new_permissions: vec![Permission::Screen] });
        m.items.extend([a.clone(), b.clone()]);
        m.updates = vec![a, b];
    }

    #[test]
    fn tabs_switch_and_selection_stays_in_range() {
        let mut m = model();
        assert_eq!(m.tab, Tab::Installed);
        key(&mut m, KeyCode::Tab);
        assert_eq!(m.tab, Tab::Browse);
        key(&mut m, KeyCode::BackTab);
        assert_eq!(m.tab, Tab::Installed);
        ch(&mut m, '3');
        assert_eq!(m.tab, Tab::Updates);
        ch(&mut m, '1');
        for _ in 0..20 {
            ch(&mut m, 'j');
        }
        assert_eq!(m.selected, m.rows().len() - 1);
        ch(&mut m, 'k');
        assert_eq!(m.selected, m.rows().len() - 2);
        key(&mut m, KeyCode::Home);
        assert_eq!(m.selected, 0);
        key(&mut m, KeyCode::End);
        assert_eq!(m.selected, m.rows().len() - 1);
        ch(&mut m, '2');
        assert_eq!(m.selected, 0, "a new tab starts at the top");
    }

    #[test]
    fn filter_narrows_browse() {
        let mut m = model();
        ch(&mut m, '2');
        assert_eq!(m.rows().len(), 3);
        ch(&mut m, '/');
        typed(&mut m, "dock");
        let names: Vec<String> = m.rows().iter().map(|r| r.name().to_string()).collect();
        assert_eq!(names, ["mira/docker"]);
        typed(&mut m, "q");
        assert_eq!(m.filter.as_deref(), Some("dockq"), "q types while filtering");
        key(&mut m, KeyCode::Backspace);
        key(&mut m, KeyCode::Esc);
        assert_eq!(m.filter, None);
        assert_eq!(m.rows().len(), 3);
        assert_eq!(key(&mut m, KeyCode::Esc), [Effect::Quit]);
    }

    #[test]
    fn install_without_permissions_does_not_ask() {
        let mut m = model();
        ch(&mut m, '2');
        select(&mut m, "mira/x");
        assert_eq!(ch(&mut m, 'i'), [Effect::Install { name: "mira/x".into(), allowed: vec![] }]);
        assert!(m.dialog.is_none());
        assert_eq!(m.busy.as_deref(), Some("Installing mira/x…"));
    }

    #[test]
    fn install_with_permissions_asks_first() {
        let mut m = model();
        ch(&mut m, '2');
        select(&mut m, "mira/docker");
        assert!(ch(&mut m, 'i').is_empty());
        match &m.dialog {
            Some(Dialog::Approve { name, permissions, .. }) => {
                assert_eq!(name, "mira/docker");
                assert_eq!(permissions, &[Permission::Panes]);
            }
            other => panic!("{other:?}"),
        }
        assert!(ch(&mut m, 'n').is_empty());
        assert!(m.dialog.is_none() && m.busy.is_none());
        ch(&mut m, 'i');
        assert_eq!(ch(&mut m, 'y'), [Effect::Install { name: "mira/docker".into(), allowed: vec![Permission::Panes] }]);
        assert!(m.dialog.is_none());
    }

    #[test]
    fn update_asks_only_for_new_permissions() {
        let mut m = model();
        with_updates(&mut m);
        ch(&mut m, '3');
        select(&mut m, "mira/a");
        assert_eq!(ch(&mut m, 'u'), [Effect::Update { name: "mira/a".into(), allowed: vec![] }]);
        m.busy = None;
        select(&mut m, "mira/b");
        assert!(ch(&mut m, 'u').is_empty());
        match &m.dialog {
            Some(Dialog::Approve { permissions, .. }) => assert_eq!(permissions, &[Permission::Screen]),
            other => panic!("{other:?}"),
        }
        assert_eq!(key(&mut m, KeyCode::Enter), [Effect::Update { name: "mira/b".into(), allowed: vec![Permission::Screen] }]);
    }

    #[test]
    fn update_all_asks_once_for_everything_new() {
        let mut m = model();
        with_updates(&mut m);
        let all = Effect::UpdateAll(vec![("mira/a".into(), vec![]), ("mira/b".into(), vec![Permission::Screen])]);
        assert!(ch(&mut m, 'U').is_empty());
        match &m.dialog {
            Some(Dialog::Approve { name, permissions, then }) => {
                assert_eq!(name, "mira/b");
                assert_eq!(permissions, &[Permission::Screen]);
                assert_eq!(**then, all.clone());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(ch(&mut m, 'y'), [all]);
    }

    #[test]
    fn remove_asks_about_data() {
        let mut m = model();
        select(&mut m, "mira/on");
        assert!(ch(&mut m, 'x').is_empty());
        assert_eq!(m.dialog, Some(Dialog::RemoveData { name: "mira/on".into() }));
        assert_eq!(key(&mut m, KeyCode::Enter), [Effect::Remove { name: "mira/on".into(), delete_data: false }]);
        m.busy = None;
        ch(&mut m, 'x');
        assert_eq!(ch(&mut m, 'y'), [Effect::Remove { name: "mira/on".into(), delete_data: true }]);
        m.busy = None;
        ch(&mut m, 'x');
        assert!(key(&mut m, KeyCode::Esc).is_empty());
        assert!(m.dialog.is_none());
        select(&mut m, "where");
        assert!(ch(&mut m, 'x').is_empty());
        assert!(m.status.as_deref().unwrap().contains("folder"), "{:?}", m.status);
    }

    #[test]
    fn removing_a_built_in_turns_it_off() {
        let mut m = model();
        select(&mut m, "recent-dirs");
        assert_eq!(ch(&mut m, 'x'), [Effect::SetEnabled { name: "recent-dirs".into(), on: false, approve: None }]);
        m.busy = None;
        select(&mut m, "git-branch");
        assert!(ch(&mut m, 'x').is_empty());
        assert_eq!(m.status.as_deref(), Some("git-branch is built in, so it can't be removed; it's off already"));
    }

    #[test]
    fn removing_a_plugin_that_isn_t_installed_takes_it_off_the_list() {
        let mut m = model();
        let mut missing = item("sn/pomodoro", Kind::Registry, true, vec![]);
        missing.attention = Some(Attention::NotInstalled);
        missing.manifest = None;
        m.items.push(missing);
        select(&mut m, "sn/pomodoro");
        assert_eq!(ch(&mut m, 'x'), [Effect::SetEnabled { name: "sn/pomodoro".into(), on: false, approve: None }]);
    }

    #[test]
    fn space_toggles_and_local_explains() {
        let mut m = model();
        select(&mut m, "git-branch");
        assert_eq!(ch(&mut m, ' '), [Effect::SetEnabled { name: "git-branch".into(), on: true, approve: None }]);
        m.busy = None;
        select(&mut m, "ssh-hosts");
        assert!(ch(&mut m, ' ').is_empty());
        assert!(matches!(m.dialog, Some(Dialog::Approve { .. })));
        let manifest = m.items.iter().find(|i| i.name == "ssh-hosts").unwrap().manifest.clone();
        assert_eq!(ch(&mut m, 'y'), [Effect::SetEnabled { name: "ssh-hosts".into(), on: true, approve: manifest }]);
        m.busy = None;
        select(&mut m, "mira/on");
        assert_eq!(ch(&mut m, ' '), [Effect::SetEnabled { name: "mira/on".into(), on: false, approve: None }]);
        m.busy = None;
        select(&mut m, "where");
        assert!(ch(&mut m, ' ').is_empty());
        assert_eq!(
            m.status.as_deref(),
            Some("Local plugins are always on; move its folder out of ~/.config/lumen/plugins to turn it off")
        );
    }

    #[test]
    fn not_installed_items_install_on_space() {
        let mut m = model();
        let mut missing = item("mira/docker", Kind::Registry, true, vec![]);
        missing.attention = Some(Attention::NotInstalled);
        missing.manifest = None;
        m.items.push(missing);
        select(&mut m, "mira/docker");
        assert!(ch(&mut m, ' ').is_empty(), "it needs panes, so it asks");
        assert_eq!(ch(&mut m, 'y'), [Effect::Install { name: "mira/docker".into(), allowed: vec![Permission::Panes] }]);
    }

    #[test]
    fn keys_are_ignored_while_busy() {
        let mut m = model();
        m.busy = Some("Installing mira/x…".into());
        select(&mut m, "git-branch");
        for c in ['i', 'u', 'x', ' ', 'r', 'U'] {
            assert!(ch(&mut m, c).is_empty(), "{c}");
        }
        assert_eq!(m.status.as_deref(), Some("Wait for \"Installing mira/x…\" to finish"));
        assert_eq!(ch(&mut m, 'q'), [Effect::Quit]);
    }

    #[test]
    fn s_edits_settings_and_saves_them() {
        let mut m = model();
        select(&mut m, "git-branch");
        assert!(ch(&mut m, 's').is_empty());
        assert_eq!(m.status.as_deref(), Some("git-branch has no settings"));
        let i = m.items.iter_mut().find(|i| i.name == "git-branch").unwrap();
        i.settings = vec![crate::plugin_settings::Spec {
            key: "sound".into(),
            kind: crate::plugin_settings::Kind::Boolean,
            default: serde_json::json!(true),
            description: "ring".into(),
        }];
        ch(&mut m, 's');
        assert!(m.editing.is_some());
        assert_eq!(
            ch(&mut m, ' '),
            [Effect::SaveSetting { plugin: "git-branch".into(), key: "sound".into(), value: Some(serde_json::json!(false)) }]
        );
        m.busy = None;
        assert!(ch(&mut m, 'q').is_empty(), "q leaves the editor, not the manager");
        assert!(m.editing.is_none());
    }

    #[test]
    fn a_background_load_leaves_busy_alone() {
        let mut m = model();
        m.busy = Some("Installing mira/x…".into());
        m.status = Some("reading this".into());
        let snapshot = m.snapshot.clone();
        m.loaded(Ok(String::new()), snapshot.clone());
        assert_eq!(m.busy.as_deref(), Some("Installing mira/x…"), "the install is still queued");
        assert_eq!(m.status.as_deref(), Some("reading this"));
        m.loaded(Err("Couldn't load the plugin index: offline".into()), snapshot);
        assert_eq!(m.busy.as_deref(), Some("Installing mira/x…"));
        assert_eq!(m.status.as_deref(), Some("Couldn't load the plugin index: offline"));
    }

    #[test]
    fn approved_builtins_turn_on_without_asking_again() {
        let mut m = model();
        let dir = crate::registry::testing::temp("model-approved");
        m.snapshot.approvals = dir.join("approvals.json");
        let manifest = m.items.iter().find(|i| i.name == "ssh-hosts").unwrap().manifest.clone().unwrap();
        crate::plugins::record_decision_at(&m.snapshot.approvals, &manifest, true).unwrap();
        select(&mut m, "ssh-hosts");
        assert_eq!(ch(&mut m, ' '), [Effect::SetEnabled { name: "ssh-hosts".into(), on: true, approve: None }]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn done_keeps_the_selection_by_name() {
        let mut m = model();
        select(&mut m, "git-branch");
        assert_eq!(m.selected, 1);
        m.busy = Some("x".into());
        let mut snapshot = m.snapshot.clone();
        snapshot.enabled = vec!["git-branch".into()];
        m.done(Err("no network".into()), snapshot);
        assert_eq!(m.busy, None);
        assert_eq!(m.status.as_deref(), Some("no network"));
        assert_eq!(m.rows()[m.selected].name(), "git-branch");
        assert_eq!(m.selected, 2, "the built-ins are all listed now, Switch Git Branch third");
    }
}
