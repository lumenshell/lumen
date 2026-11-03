//! Keyboard shortcuts: the built-in defaults plus the `[keys]` table in
//! config.toml, which binds keys to actions by name.
//!
//! A key in `[keys]` replaces whatever that key did by default; binding it to
//! `"none"` frees it so it reaches the program in the terminal instead. The
//! other default keys for an action keep working.

use gpui::{Action, App, DummyKeyboardMapper, KeyBinding, Keystroke};

use crate::settings::Settings;
use crate::*;

/// Action names for `[keys]`, in the order they're documented.
pub const ACTIONS: &[&str] = &[
    "new-tab",
    "new-window",
    "close-pane",
    "split-right",
    "split-down",
    "next-tab",
    "prev-tab",
    "tab-1",
    "tab-2",
    "tab-3",
    "tab-4",
    "tab-5",
    "tab-6",
    "tab-7",
    "tab-8",
    "tab-9",
    "rename-tab",
    "next-pane",
    "focus-left",
    "focus-right",
    "focus-up",
    "focus-down",
    "toggle-zoom",
    "command-palette",
    "next-theme",
    "increase-font",
    "decrease-font",
    "reset-font",
    "copy",
    "paste",
    "find",
    "prev-command",
    "next-command",
    "copy-last-output",
    "select-mode",
    "quit",
];

/// Frees a key: it's sent to the terminal instead of running an action.
pub const NONE: &str = "none";

pub fn action(name: &str) -> Option<Box<dyn Action>> {
    if let Some(rest) = name.strip_prefix("plugin:") {
        let (plugin, id) = rest.split_once(':').filter(|(p, i)| !p.is_empty() && !i.is_empty())?;
        return Some(Box::new(RunPluginCommand { plugin: plugin.into(), id: id.into() }));
    }
    if let Some(n) = name.strip_prefix("tab-") {
        let n: usize = n.parse().ok().filter(|n| (1..=9).contains(n))?;
        return Some(Box::new(ActivateTab(n - 1)));
    }
    Some(match name {
        "new-tab" => Box::new(NewTab),
        "new-window" => Box::new(NewWindow),
        "close-pane" => Box::new(ClosePane),
        "split-right" => Box::new(SplitRight),
        "split-down" => Box::new(SplitDown),
        "next-tab" => Box::new(NextTab),
        "prev-tab" => Box::new(PrevTab),
        "rename-tab" => Box::new(RenameTab),
        "next-pane" => Box::new(NextPane),
        "focus-left" => Box::new(FocusLeft),
        "focus-right" => Box::new(FocusRight),
        "focus-up" => Box::new(FocusUp),
        "focus-down" => Box::new(FocusDown),
        "toggle-zoom" => Box::new(ToggleZoom),
        "command-palette" => Box::new(TogglePalette),
        "next-theme" => Box::new(NextTheme),
        "increase-font" => Box::new(IncreaseFont),
        "decrease-font" => Box::new(DecreaseFont),
        "reset-font" => Box::new(ResetFont),
        "copy" => Box::new(Copy),
        "paste" => Box::new(Paste),
        "find" => Box::new(Find),
        "prev-command" => Box::new(PrevPrompt),
        "next-command" => Box::new(NextPrompt),
        "copy-last-output" => Box::new(CopyLastOutput),
        "select-mode" => Box::new(ToggleSelectMode),
        "quit" => Box::new(Quit),
        _ => return None,
    })
}

/// The built-in shortcuts as (keys, action name). macOS uses cmd; Linux uses
/// ctrl-shift so plain ctrl keys reach the shell, except a few everyone
/// expects from a terminal app (Ctrl+T, Ctrl+1–9).
fn defaults() -> Vec<(String, &'static str)> {
    let mac = cfg!(target_os = "macos");
    let m = |mac_key: &str, linux_key: &str| if mac { mac_key } else { linux_key }.to_string();
    let mut b = vec![
        (m("cmd-t", "ctrl-shift-t"), "new-tab"),
        (m("cmd-n", "ctrl-shift-n"), "new-window"),
        (m("cmd-w", "ctrl-shift-w"), "close-pane"),
        (m("cmd-d", "ctrl-shift-d"), "split-right"),
        (m("cmd-shift-d", "ctrl-shift-e"), "split-down"),
        ("ctrl-tab".into(), "next-tab"),
        ("ctrl-shift-tab".into(), "prev-tab"),
        (m("cmd-shift-]", "ctrl-pagedown"), "next-tab"),
        (m("cmd-shift-[", "ctrl-pageup"), "prev-tab"),
        (m("cmd-,", "ctrl-,"), "rename-tab"),
        // Ctrl+Shift+]: GPUI reports it on Linux as the `}` it types.
        (m("cmd-]", "ctrl-}"), "next-pane"),
        (m("cmd-left", "ctrl-left"), "focus-left"),
        (m("cmd-right", "ctrl-right"), "focus-right"),
        (m("cmd-up", "ctrl-up"), "focus-up"),
        (m("cmd-down", "ctrl-down"), "focus-down"),
        (m("cmd-alt-left", "alt-shift-left"), "focus-left"),
        (m("cmd-alt-right", "alt-shift-right"), "focus-right"),
        (m("cmd-alt-up", "alt-shift-up"), "focus-up"),
        (m("cmd-alt-down", "alt-shift-down"), "focus-down"),
        (m("cmd-shift-enter", "ctrl-shift-enter"), "toggle-zoom"),
        (m("cmd-shift-p", "ctrl-shift-p"), "command-palette"),
        (m("cmd-shift-y", "ctrl-shift-y"), "next-theme"),
        (m("cmd-=", "ctrl-="), "increase-font"),
        (m("cmd-+", "ctrl-+"), "increase-font"),
        (m("cmd--", "ctrl--"), "decrease-font"),
        (m("cmd-0", "ctrl-0"), "reset-font"),
        (m("cmd-c", "ctrl-shift-c"), "copy"),
        (m("cmd-v", "ctrl-shift-v"), "paste"),
        (m("cmd-f", "ctrl-shift-f"), "find"),
        (m("cmd-shift-up", "ctrl-shift-up"), "prev-command"),
        (m("cmd-shift-down", "ctrl-shift-down"), "next-command"),
        (m("cmd-shift-o", "ctrl-shift-o"), "copy-last-output"),
        (m("cmd-shift-space", "ctrl-shift-space"), "select-mode"),
        ("shift-insert".into(), "paste"),
        (m("cmd-q", "ctrl-shift-q"), "quit"),
    ];
    const TABS: [&str; 9] = ["tab-1", "tab-2", "tab-3", "tab-4", "tab-5", "tab-6", "tab-7", "tab-8", "tab-9"];
    if !mac {
        b.push(("ctrl-t".into(), "new-tab"));
    }
    for (i, name) in TABS.iter().enumerate() {
        if mac {
            b.push((format!("cmd-{}", i + 1), name));
        } else {
            b.push((format!("ctrl-{}", i + 1), name));
            b.push((format!("alt-{}", i + 1), name));
        }
    }
    b
}

/// Named keys accepted besides single characters.
fn is_named_key(key: &str) -> bool {
    const NAMED: &[&str] = &[
        "enter", "tab", "space", "backspace", "escape", "delete", "insert", "home", "end", "pageup", "pagedown",
        "up", "down", "left", "right",
    ];
    NAMED.contains(&key)
        || key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=24).contains(&n))
}

/// Check a key and write it the way GPUI does, so `Ctrl-Shift-T` and
/// `shift-ctrl-t` count as the same key.
pub fn normalize_keys(source: &str) -> Result<String, String> {
    let parts: Vec<_> = source.split_whitespace().collect();
    match parts.len() {
        0 => return Err("an empty key".into()),
        1 => {}
        // GPUI holds the first key of a sequence back while it waits for the
        // next; until that's checked against the terminal's own key handling
        // (tmux's ctrl-a, say), only single keys are allowed.
        _ => return Err("key sequences aren't supported yet; bind a single key".into()),
    }
    parts
        .iter()
        .map(|part| {
            // Case-insensitive: GPUI would read `T` as shift-t, which surprises
            // people writing `Ctrl-Alt-T`.
            let ks = Keystroke::parse(&part.to_ascii_lowercase()).map_err(|_| format!("`{part}` isn't a key GPUI understands"))?;
            if ks.key_char.is_some() || !(ks.key.chars().count() == 1 || is_named_key(&ks.key)) {
                return Err(format!("unknown key `{}` in `{part}`", ks.key));
            }
            Ok(linux_shifted(ks).unparse())
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|v| v.join(" "))
}

/// On Linux GPUI reports Shift with a symbol key as the symbol it types
/// (Ctrl+Shift+] arrives as `ctrl-}`), so a binding written with Shift and
/// the unshifted symbol would never match. Rewrite it the way it arrives,
/// for a US layout.
fn linux_shifted(mut ks: Keystroke) -> Keystroke {
    const PAIRS: &str = "1!2@3#4$5%6^7&8*9(0)-_=+[{]}\\|;:'\",<.>/?`~";
    if cfg!(target_os = "linux") && ks.modifiers.shift && ks.key.chars().count() == 1 {
        let chars: Vec<char> = PAIRS.chars().collect();
        if let Some(pair) = chars.chunks(2).find(|p| p[0].to_string() == ks.key) {
            ks.key = pair[1].to_string();
            ks.modifiers.shift = false;
        }
    }
    ks
}

/// Check an action name from `[keys]`: Ok(None) frees the key.
pub fn check_action(name: &str) -> Result<Option<String>, String> {
    let name = name.trim();
    // Plugin commands keep their case: plugin:<plugin>:<command id>.
    if name.starts_with("plugin:") {
        return match action(name) {
            Some(_) => Ok(Some(name.to_string())),
            None => Err(format!("\"{name}\" should look like \"plugin:<plugin>:<command>\"")),
        };
    }
    let name = name.to_ascii_lowercase();
    if name == NONE {
        return Ok(None);
    }
    if action(&name).is_some() {
        return Ok(Some(name));
    }
    let hint = ACTIONS
        .iter()
        .map(|a| (distance(&name, a), a))
        .min()
        .filter(|(d, _)| *d <= 3)
        .map(|(_, a)| format!("; did you mean \"{a}\"?"))
        .unwrap_or_else(|| "; the actions are listed in the README".into());
    Err(format!("unknown action \"{name}\"{hint}"))
}

/// Edit distance, for suggesting the action someone meant.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// The bindings to install: defaults, then keys plugins asked for, minus
/// keys the user rebound or freed, plus the user's own. `user` holds
/// normalized keys and checked actions; `plugin` (keys, plugin, command).
fn bindings(user: &[(String, Option<String>)], plugin: &[(String, String, String)]) -> Vec<KeyBinding> {
    let taken = |keys: &str| {
        let keys = normalize_keys(keys).unwrap_or_default();
        user.iter().any(|(k, _)| *k == keys)
    };
    defaults()
        .into_iter()
        .map(|(keys, name)| (keys, name.to_string()))
        .chain(plugin.iter().map(|(k, p, id)| (k.clone(), format!("plugin:{p}:{id}"))))
        .filter(|(keys, _)| !taken(keys))
        .chain(user.iter().filter_map(|(k, a)| Some((k.clone(), a.clone()?))))
        .filter_map(|(keys, name)| {
            KeyBinding::load(&keys, action(&name)?, None, false, None, &DummyKeyboardMapper).ok()
        })
        .collect()
}

/// Install the shortcuts for the current settings, replacing any before.
pub fn apply(cx: &mut App) {
    let bindings = bindings(&Settings::get(cx).keys, &crate::plugins::key_bindings());
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_of(bindings: &[KeyBinding], name: &str) -> Vec<String> {
        let target = action(name).unwrap();
        bindings
            .iter()
            .filter(|b| b.action().partial_eq(target.as_ref()))
            .map(|b| b.keystrokes().iter().map(|k| k.inner().unparse()).collect::<Vec<_>>().join(" "))
            .collect()
    }

    #[test]
    fn every_action_name_resolves_and_defaults_are_valid() {
        for name in ACTIONS {
            assert!(action(name).is_some(), "{name}");
        }
        for (keys, name) in defaults() {
            assert!(normalize_keys(&keys).is_ok(), "{keys}");
            assert!(ACTIONS.contains(&name), "{name}");
        }
        assert_eq!(bindings(&[], &[]).len(), defaults().len());
    }

    #[test]
    fn shifted_symbols_match_how_linux_reports_them() {
        let want = if cfg!(target_os = "linux") { "ctrl-}" } else { "ctrl-shift-]" };
        assert_eq!(normalize_keys("ctrl-shift-]").unwrap(), want);
        assert_eq!(normalize_keys("ctrl-shift-t").unwrap(), "ctrl-shift-t");
        assert_eq!(normalize_keys("ctrl-shift-up").unwrap(), "ctrl-shift-up");
    }

    #[test]
    fn arrows_with_ctrl_or_cmd_focus_panes() {
        let b = bindings(&[], &[]);
        let ctrl = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
        for dir in ["left", "right", "up", "down"] {
            assert!(keys_of(&b, &format!("focus-{dir}")).contains(&format!("{ctrl}-{dir}")), "{dir}");
        }
        // No key is bound to two actions.
        let mut keys: Vec<_> = defaults().into_iter().map(|(k, _)| k).collect();
        let n = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), n);
    }

    #[test]
    fn number_keys_go_to_tabs() {
        let keys: Vec<String> = defaults().into_iter().filter(|(_, a)| *a == "tab-3").map(|(k, _)| k).collect();
        if cfg!(target_os = "macos") {
            assert_eq!(keys, ["cmd-3"]);
        } else {
            assert_eq!(keys, ["ctrl-3", "alt-3"]);
        }
    }

    #[test]
    fn keys_are_normalized() {
        assert_eq!(normalize_keys("Shift-Ctrl-T").unwrap(), normalize_keys("ctrl-shift-t").unwrap());
        assert!(normalize_keys("ctrl-k ctrl-s").unwrap_err().contains("sequences"));
        assert!(normalize_keys("ctrl-shift-tbs").unwrap_err().contains("unknown key `tbs`"));
        assert!(normalize_keys("ctrl-shfit-t").is_err());
        assert!(normalize_keys("f13").is_ok());
        assert!(normalize_keys("f25").is_err());
        assert!(normalize_keys(" ").is_err());
    }

    #[test]
    fn actions_are_checked_with_suggestions() {
        assert_eq!(check_action("New-Tab").unwrap(), Some("new-tab".into()));
        assert_eq!(check_action("none").unwrap(), None);
        assert!(check_action("new-tb").unwrap_err().contains("did you mean \"new-tab\""));
        assert!(check_action("frobnicate").unwrap_err().contains("README"));
        assert!(check_action("tab-10").is_err());
    }

    #[test]
    fn user_keys_replace_and_free_defaults() {
        let next = if cfg!(target_os = "macos") { "cmd-shift-]" } else { "ctrl-pagedown" };
        let user = vec![
            (normalize_keys("ctrl-tab").unwrap(), None),
            (normalize_keys("ctrl-alt-t").unwrap(), Some("new-tab".to_string())),
            (normalize_keys(next).unwrap(), Some("find".to_string())),
        ];
        let b = bindings(&user, &[("alt-g".into(), "git".into(), "switch".into())]);
        assert_eq!(keys_of(&b, "plugin:git:switch"), vec!["alt-g".to_string()]);
        assert_eq!(check_action("plugin:git:Switch").unwrap(), Some("plugin:git:Switch".into()));
        assert!(check_action("plugin:git").is_err());
        // Ctrl+Tab is freed; next-tab keeps no key, since its other default was rebound.
        assert!(keys_of(&b, "next-tab").is_empty());
        assert!(keys_of(&b, "new-tab").contains(&"ctrl-alt-t".to_string()));
        let defaults = if cfg!(target_os = "macos") { 1 } else { 2 };
        assert_eq!(keys_of(&b, "new-tab").len(), defaults + 1, "the default keys still work");
        assert!(keys_of(&b, "find").contains(&normalize_keys(next).unwrap()));
    }
}
