//! What the manager shows: every plugin of every kind, the registry's
//! index, and what needs attention. Nothing here draws or waits on a key.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::plugin_settings::Spec;
use crate::plugins::{self, BUILTINS, Decision, Manifest, Permission};
use crate::registry::{self, Entry, Index, Installed, Paths};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    BuiltIn,
    Local,
    Registry,
}

/// Why a plugin is marked `!`.
#[derive(Clone, Debug, PartialEq)]
pub enum Attention {
    NotInstalled,
    NotApproved,
    /// It failed to start, or Lumen stopped it: the status Lumen gave.
    Failed(String),
    /// Values in its `[plugin.NAME]` table that aren't valid.
    Settings(Vec<String>),
}

/// A newer version an installed registry plugin can update to.
#[derive(Clone, Debug, PartialEq)]
pub struct Upgrade {
    pub version: String,
    /// What it needs that isn't approved yet: updating asks about these.
    pub new_permissions: Vec<Permission>,
}

/// One plugin, of any kind, as the manager lists it.
#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub title: String,
    pub kind: Kind,
    pub on: bool,
    pub version: Option<String>,
    pub update: Option<Upgrade>,
    pub attention: Option<Attention>,
    pub description: String,
    pub permissions: Vec<Permission>,
    /// Its manifest, when it could be read.
    pub manifest: Option<Manifest>,
    /// Its registry entry, when the index has one.
    pub entry: Option<Entry>,
    pub settings: Vec<Spec>,
    /// Each setting's value in use: config.toml's where valid, else the default.
    pub values: Map<String, Value>,
}

/// Everything the manager reads, at one moment.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// `plugins` in config.toml.
    pub enabled: Vec<String>,
    pub installed: BTreeMap<String, Installed>,
    /// Your own plugin folders.
    pub local: Vec<Manifest>,
    pub index: Option<Index>,
    /// config.toml's text.
    pub config: String,
    /// What Lumen says about each running plugin: its status, and whether
    /// that's trouble.
    pub statuses: BTreeMap<String, (String, bool)>,
    pub approvals: PathBuf,
}

impl Snapshot {
    pub fn load(p: &Paths, index: Option<Index>, statuses: BTreeMap<String, (String, bool)>) -> Snapshot {
        let config = std::fs::read_to_string(&p.config).unwrap_or_default();
        let enabled = crate::settings::Settings::parse(&config).plugins;
        let local = plugins::plugins_dir()
            .and_then(|d| std::fs::read_dir(d).ok())
            .map(|entries| {
                let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|d| d.join("plugin.toml").is_file()).collect();
                dirs.sort();
                dirs.iter().filter_map(|d| plugins::check_folder(d).ok()).collect()
            })
            .unwrap_or_default();
        Snapshot {
            enabled,
            installed: registry::load_installed_from(&p.installed),
            local,
            index,
            config,
            statuses,
            approvals: p.approvals.clone(),
        }
    }

    fn entry(&self, name: &str) -> Option<&Entry> {
        self.index.as_ref()?.plugins.iter().find(|e| e.name == name)
    }

    fn on(&self, name: &str) -> bool {
        self.enabled.iter().any(|n| n == name)
    }
}

/// Every plugin: built-in ones in Lumen's order, then yours, then installed
/// registry plugins, then those config.toml names that aren't installed.
pub fn installed(s: &Snapshot) -> Vec<Item> {
    let tables = crate::settings::Settings::parse(&s.config).plugin_settings;
    let item = |name: &str, title: &str, kind: Kind, manifest: Result<Manifest, String>| {
        let on = kind == Kind::Local || s.on(name);
        let mut attention = None;
        let (mut settings, mut values, mut permissions, mut description, mut version) =
            (Vec::new(), Map::new(), Vec::new(), String::new(), None);
        match &manifest {
            Ok(m) => {
                let (v, problems) = crate::plugin_settings::resolve(name, &m.settings, tables.get(name).map(|t| &t.0));
                settings = m.settings.clone();
                values = v;
                permissions = m.permissions.clone();
                description = m.description.clone();
                if kind != Kind::BuiltIn && !m.version.is_empty() {
                    version = Some(m.version.clone());
                }
                if kind == Kind::Registry && on && plugins::decision_at(&s.approvals, m) != Decision::Allowed {
                    attention = Some(Attention::NotApproved);
                } else if let Some((status, true)) = s.statuses.get(name) {
                    attention = Some(Attention::Failed(status.clone()));
                } else if !problems.is_empty() {
                    attention = Some(Attention::Settings(problems));
                }
            }
            Err(e) => attention = Some(Attention::Failed(format!("its plugin.toml can't be read: {e}"))),
        }
        let entry = s.entry(name).cloned();
        if description.is_empty()
            && let Some(e) = &entry
        {
            description = e.description.clone();
        }
        Item {
            name: name.into(),
            title: title.into(),
            kind,
            on,
            version,
            update: None,
            attention,
            description,
            permissions,
            manifest: manifest.ok(),
            entry,
            settings,
            values,
        }
    };
    let mut items: Vec<Item> = BUILTINS.iter().map(|b| item(b.name, b.title, Kind::BuiltIn, b.parsed())).collect();
    items.extend(s.local.iter().map(|m| item(&m.name, &m.name, Kind::Local, Ok(m.clone()))));
    for (name, i) in &s.installed {
        let manifest = plugins::check_folder(&i.dir).map(|mut m| {
            m.name = name.clone();
            m.registry = true;
            m
        });
        let mut it = item(name, name, Kind::Registry, manifest);
        if it.version.is_none() {
            it.version = Some(i.version.clone());
        }
        it.update = s
            .entry(name)
            .and_then(|e| registry::best(e, registry::LUMEN_VERSION))
            .filter(|r| registry::newer(&r.version, &i.version))
            .map(|r| {
                let allowed = plugins::registry_approval_in(&s.approvals, name);
                Upgrade {
                    version: r.version.clone(),
                    new_permissions: r.permissions.iter().copied().filter(|p| !allowed.contains(p)).collect(),
                }
            });
        items.push(it);
    }
    let mut missing: Vec<&String> =
        s.enabled.iter().filter(|n| registry::valid_name(n) && !s.installed.contains_key(*n)).collect();
    missing.sort();
    missing.dedup();
    for name in missing {
        let entry = s.entry(name).cloned();
        items.push(Item {
            name: name.clone(),
            title: name.clone(),
            kind: Kind::Registry,
            on: true,
            version: None,
            update: None,
            attention: Some(Attention::NotInstalled),
            description: entry.as_ref().map(|e| e.description.clone()).unwrap_or_default(),
            permissions: entry
                .as_ref()
                .and_then(|e| registry::best(e, registry::LUMEN_VERSION))
                .map(|r| r.permissions.clone())
                .unwrap_or_default(),
            manifest: None,
            entry,
            settings: Vec::new(),
            values: Map::new(),
        });
    }
    items
}

/// The registry's plugins matching `filter`, most stars first.
pub fn browse<'a>(s: &'a Snapshot, filter: &str) -> Vec<&'a Entry> {
    s.index.as_ref().map(|i| registry::search(i, filter)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::Permission;
    use crate::registry::testing::{entry, paths, release};
    use crate::registry::{Index, Installed};

    /// Installed registry plugins with a newer compatible version.
    fn updates(s: &Snapshot) -> Vec<Item> {
        installed(s).into_iter().filter(|i| i.update.is_some()).collect()
    }
    use std::collections::BTreeMap;

    /// A registry plugin installed under `p.root`, with `toml` as its
    /// plugin.toml.
    fn install(p: &crate::registry::Paths, name: &str, version: &str, perms: Vec<Permission>, toml: &str) -> Installed {
        let dir = p.root.join(name).join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), toml).unwrap();
        Installed { version: version.into(), commit: "c".repeat(40), permissions: perms, dir }
    }

    fn snapshot(p: &crate::registry::Paths, config: &str, installed: BTreeMap<String, Installed>, index: Option<Index>) -> Snapshot {
        let settings = crate::settings::Settings::parse(config);
        Snapshot {
            enabled: settings.plugins.clone(),
            installed,
            local: Vec::new(),
            index,
            config: config.into(),
            statuses: BTreeMap::new(),
            approvals: p.approvals.clone(),
        }
    }

    const X: &str = "version = \"1.0.0\"\ndescription = \"X things\"\ncommand = \"python3 main.py\"\npermissions = [\"panes\"]\n";

    fn approve(p: &crate::registry::Paths, name: &str, perms: &[Permission]) {
        std::fs::create_dir_all(p.approvals.parent().unwrap()).unwrap();
        crate::plugins::approve_registry_in(&p.approvals, name, perms);
    }

    #[test]
    fn every_kind_is_listed() {
        let (dir, p) = paths("data-kinds");
        let x = install(&p, "mira/x", "1.0.0", vec![Permission::Panes], X);
        approve(&p, "mira/x", &[Permission::Panes]);
        let mut s = snapshot(&p, "plugins = [\"recent-dirs\", \"mira/x\", \"mira/y\"]\n", BTreeMap::from([("mira/x".into(), x)]), None);
        let mut local = crate::plugins::parse_manifest(&dir.join("where"), "command = \"./where\"\nversion = \"0.1.0\"\n").unwrap();
        local.name = "where".into();
        s.local = vec![local];
        let items = installed(&s);
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(&titles[..7], ["Recent Directories", "Command History", "Switch Git Branch", "SSH to Host", "Projects & Tasks", "Build Status", "Pomodoro Timer"]);
        assert_eq!(&titles[7..], ["where", "mira/x", "mira/y"]);
        let by = |n: &str| items.iter().find(|i| i.name == n).unwrap();
        assert!(by("recent-dirs").on && !by("pomodoro").on);
        assert_eq!(by("where").kind, Kind::Local);
        assert!(by("mira/x").on && by("mira/x").attention.is_none(), "{:?}", by("mira/x").attention);
        assert_eq!(by("mira/x").version.as_deref(), Some("1.0.0"));
        assert_eq!(by("mira/x").description, "X things");
        assert_eq!(by("mira/y").attention, Some(Attention::NotInstalled));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unapproved_registry_plugins_need_attention() {
        let (dir, p) = paths("data-unapproved");
        let x = install(&p, "mira/x", "1.0.0", vec![Permission::Panes], X);
        let s = snapshot(&p, "plugins = [\"mira/x\"]\n", BTreeMap::from([("mira/x".into(), x)]), None);
        let items = installed(&s);
        assert_eq!(items.iter().find(|i| i.name == "mira/x").unwrap().attention, Some(Attention::NotApproved));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_plugins_need_attention() {
        let (dir, p) = paths("data-failed");
        let x = install(&p, "mira/x", "1.0.0", vec![Permission::Panes], X);
        approve(&p, "mira/x", &[Permission::Panes]);
        let mut s = snapshot(&p, "plugins = [\"mira/x\"]\n", BTreeMap::from([("mira/x".into(), x)]), None);
        s.statuses.insert("mira/x".into(), ("exited with status 1".into(), true));
        s.statuses.insert("recent-dirs".into(), ("running".into(), false));
        let items = installed(&s);
        assert_eq!(items.iter().find(|i| i.name == "mira/x").unwrap().attention, Some(Attention::Failed("exited with status 1".into())));
        assert_eq!(items.iter().find(|i| i.name == "recent-dirs").unwrap().attention, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bad_settings_need_attention() {
        let (dir, p) = paths("data-settings");
        let toml = format!("{X}\n[settings.n]\ntype = \"integer\"\ndefault = 1\ndescription = \"n\"\n");
        let x = install(&p, "mira/x", "1.0.0", vec![Permission::Panes], &toml);
        approve(&p, "mira/x", &[Permission::Panes]);
        let s = snapshot(&p, "plugins = [\"mira/x\"]\n\n[plugin.\"mira/x\"]\nn = \"x\"\n", BTreeMap::from([("mira/x".into(), x)]), None);
        let items = installed(&s);
        let x = items.iter().find(|i| i.name == "mira/x").unwrap();
        match &x.attention {
            Some(Attention::Settings(v)) => assert!(v[0].contains("`n`"), "{v:?}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(x.values["n"], 1);
        assert_eq!(x.settings.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn index(versions: Vec<crate::registry::Release>) -> Index {
        let mut e = entry("mira/x", 7, &[]);
        e.versions = versions;
        Index { version: 1, plugins: vec![e] }
    }

    #[test]
    fn updates_know_new_permissions() {
        let (dir, p) = paths("data-updates");
        let x = install(&p, "mira/x", "1.0.0", vec![Permission::Panes], X);
        approve(&p, "mira/x", &[Permission::Panes]);
        let idx = index(vec![
            release("1.0.0", vec![Permission::Panes], String::new()),
            release("1.1.0", vec![Permission::Panes, Permission::Screen], String::new()),
        ]);
        let s = snapshot(&p, "plugins = [\"mira/x\"]\n", BTreeMap::from([("mira/x".into(), x)]), Some(idx));
        let up = updates(&s);
        assert_eq!(up.len(), 1);
        assert_eq!(up[0].update, Some(Upgrade { version: "1.1.0".into(), new_permissions: vec![Permission::Screen] }));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn updates_skip_versions_needing_newer_lumen() {
        let (dir, p) = paths("data-too-new");
        let x = install(&p, "mira/x", "1.0.0", vec![Permission::Panes], X);
        let mut too_new = release("1.1.0", vec![Permission::Panes], String::new());
        too_new.lumen = "99.0.0".into();
        let s = snapshot(&p, "plugins = [\"mira/x\"]\n", BTreeMap::from([("mira/x".into(), x)]), Some(index(vec![too_new])));
        assert!(updates(&s).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
