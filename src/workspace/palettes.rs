//! The command palette and the pickers built on it: themes and tab colours.

use super::*;

impl Workspace {
    pub(super) fn toggle_palette(&mut self, _: &TogglePalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.dismiss_palette(window, cx);
            return;
        }
        let color = self.tabs.get(self.active_tab).and_then(|t| shown_tab_color(t.own_color, t.color));
        let items = palette_items(window, color, cx);
        let palette = cx.new(|cx| CommandPalette::new(items, cx));
        self.open_palette(palette, window, cx);
    }

    /// Pick a theme, each one shown as you move to it; Esc puts back the one
    /// you had.
    fn open_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = Settings::get(cx);
        let original = settings.theme_choice;
        let current = settings.shown_index();
        let names: Vec<SharedString> = settings.themes.iter().map(|t| t.name.clone()).collect();
        let items = theme_picker_items(&names, current, original);
        let cancel: PaletteFn = Rc::new(move |_, cx| Settings::finish_preview(cx, original, None));
        let highlight: crate::palette::HighlightFn =
            Rc::new(|i, _, cx| Settings::preview_theme(cx, crate::settings::ThemeChoice::One(i)));
        let palette = cx.new(|cx| {
            CommandPalette::picker(items, "Pick a theme…".into(), Some(cancel), cx)
                .with_selected(current)
                .with_highlight(highlight)
        });
        self.open_palette(palette, window, cx);
    }

    /// Pick a colour for the active tab, previewed on the tab as you move;
    /// Esc puts the old one back.
    fn open_tab_color_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let original = tab.own_color;
        let current = tab_color_choice(shown_tab_color(tab.own_color, tab.color));
        let target = tab.active.downgrade();
        let ws = cx.entity().downgrade();
        // Find the tab again by its pane: tabs may move meanwhile. `done`
        // also drops a script's colour, so No Colour means none at all.
        let set = Rc::new(move |color: Option<TabColor>, done: bool, cx: &mut App| {
            let _ = ws.update(cx, |ws, cx| {
                let Some(tab) = ws.tabs.iter_mut().find(|t| t.leaves().iter().any(|l| l.downgrade() == target)) else {
                    return;
                };
                tab.own_color = color;
                if done && color.is_none() {
                    tab.color = None;
                    if tab.badge.is_none() {
                        tab.style_owner = None;
                    }
                }
                cx.notify();
            });
        });
        let items = TAB_COLOR_CHOICES
            .iter()
            .enumerate()
            .map(|(i, &(label, color))| {
                let set = set.clone();
                let item = PaletteItem::new(label, Rc::new(move |_, cx| set(color, true, cx))).dot(match color {
                    Some(TabColor::Ansi(n)) => Dot::Ansi(n),
                    _ => Dot::Hollow,
                });
                if i == current { item.note("current") } else { item }
            })
            .collect();
        let cancel: PaletteFn = {
            let set = set.clone();
            Rc::new(move |_, cx| set(original, false, cx))
        };
        let highlight: crate::palette::HighlightFn = Rc::new(move |i, _, cx| set(TAB_COLOR_CHOICES[i].1, false, cx));
        let palette = cx.new(|cx| {
            CommandPalette::picker(items, "Pick a colour for this tab…".into(), Some(cancel), cx)
                .with_selected(current)
                .with_highlight(highlight)
        });
        self.open_palette(palette, window, cx);
    }

    /// Show `palette`, replacing (and cancelling) any open one.
    pub(super) fn open_palette(
        &mut self,
        palette: Entity<CommandPalette>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_palette(window, cx);
        let sub = cx.subscribe_in(&palette, window, |this, palette, event, window, cx| match event {
            // Only if it's still the open one: a choice may have opened another.
            PaletteEvent::Dismiss if this.palette.as_ref() == Some(palette) => this.dismiss_palette(window, cx),
            PaletteEvent::Dismiss => {}
        });
        self._subs.push(sub);
        window.focus(&palette.focus_handle(cx));
        self.palette = Some(palette);
        cx.notify();
    }

    /// Close the palette; one closed without a choice runs its cancel hook.
    pub(super) fn dismiss_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.take() else { return };
        let cancel = palette.read(cx).on_cancel();
        drop(palette);
        self.focus_active(window, cx);
        cx.notify();
        // Run after this update: the hook may update the workspace itself.
        if let Some(cancel) = cancel {
            window.defer(cx, move |window, cx| cancel(window, cx));
        }
    }
}

/// A plugin's status (from `plugins::list`) as a palette dot.
/// The palette's entry for the plugin manager, with updates waiting.
fn plugins_entry_label(updates: usize) -> String {
    match updates {
        0 => "Plugins".into(),
        1 => "Plugins (1 update)".into(),
        n => format!("Plugins ({n} updates)"),
    }
}

fn shortcut_label(action: &dyn gpui::Action, window: &Window) -> SharedString {
    window
        .highest_precedence_binding_for_action(action)
        .map(|b| b.keystrokes().iter().map(|k| pretty_keystroke(&k.to_string())).collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
        .into()
}

fn pretty_keystroke(ks: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let ks = ks.replace("--", "-minus");
    ks.split('-')
        .map(|part| match (part, mac) {
            ("cmd", true) => "⌘".to_string(),
            ("shift", true) => "⇧".to_string(),
            ("alt", true) => "⌥".to_string(),
            ("ctrl", true) => "⌃".to_string(),
            ("ctrl", false) => "Ctrl".to_string(),
            ("shift", false) => "Shift".to_string(),
            ("alt", false) => "Alt".to_string(),
            ("enter", _) => "Enter".to_string(),
            ("minus", _) => "-".to_string(),
            ("tab", _) => "Tab".to_string(),
            ("pageup", _) => "PgUp".to_string(),
            ("pagedown", _) => "PgDn".to_string(),
            (k, _) => k.to_uppercase(),
        })
        .collect::<Vec<_>>()
        .join(if mac { "" } else { "+" })
}

/// The command palette's headings: Lumen's own commands, then those plugins add.
const LUMEN: &str = "Lumen";

const PLUGINS: &str = "Plugins";

/// Every command in the palette; `tab_color` is the active tab's, for its note.
fn palette_items(window: &Window, tab_color: Option<TabColor>, cx: &App) -> Vec<PaletteItem> {
    fn action<A: gpui::Action + Clone + Default>(label: &str) -> (SharedString, Box<dyn gpui::Action>) {
        (label.to_string().into(), Box::new(A::default()))
    }
    let actions: Vec<(SharedString, Box<dyn gpui::Action>)> = vec![
        action::<NewTab>("New Tab"),
        action::<NewWindow>("New Window"),
        action::<ToggleQuickTerminal>("Toggle Quick Terminal"),
        action::<SplitRight>("Split Right"),
        action::<SplitDown>("Split Down"),
        action::<ClosePane>("Close Pane"),
        action::<ToggleZoom>("Toggle Focus Mode"),
        action::<NextPane>("Focus Next Pane"),
        action::<NextTab>("Next Tab"),
        action::<PrevTab>("Previous Tab"),
        action::<RenameTab>("Rename Tab"),
        action::<NextTheme>("Cycle Theme"),
        action::<IncreaseFont>("Increase Font Size"),
        action::<DecreaseFont>("Decrease Font Size"),
        action::<ResetFont>("Reset Font Size"),
        action::<Copy>("Copy"),
        action::<Paste>("Paste"),
        action::<Find>("Find in Scrollback"),
        action::<PrevPrompt>("Jump to Previous Command"),
        action::<NextPrompt>("Jump to Next Command"),
        action::<CopyLastOutput>("Copy Last Command Output"),
        action::<ToggleSelectMode>("Keyboard Select Mode"),
        action::<Quit>("Quit"),
    ];
    let mut items: Vec<PaletteItem> = actions
        .into_iter()
        .map(|(label, act)| {
            let detail = shortcut_label(act.as_ref(), window);
            let act: Rc<dyn gpui::Action> = act.into();
            PaletteItem::new(label, Rc::new(move |window, cx| window.dispatch_action(act.boxed_clone(), cx)))
                .keys(detail)
                .section(LUMEN)
        })
        .collect();
    if let Some(path) = settings::config_path() {
        let shown = path.display().to_string().replace(&std::env::var("HOME").unwrap_or_default(), "~");
        items.push(
            PaletteItem::new(
                "Open Config File",
                Rc::new(move |_, cx| cx.open_url(&format!("file://{}", path.display()))),
            )
            .note(shown)
            .section(LUMEN),
        );
    }
    // Plugin commands, then a way to allow each plugin waiting for it.
    for info in crate::plugins::list() {
        for c in &info.commands {
            let act = RunPluginCommand { plugin: info.manifest.name.clone(), id: c.id.clone() };
            let shortcut = shortcut_label(&act, window);
            let item = PaletteItem::new(
                c.title.clone(),
                Rc::new(move |window, cx| window.dispatch_action(Box::new(act.clone()), cx)),
            );
            // Its shortcut if it has one, else which plugin it's from.
            let item = if shortcut.is_empty() { item.note(info.manifest.name.clone()) } else { item.keys(shortcut) };
            items.push(item.section(PLUGINS));
        }
    }
    let ws = window.root::<Workspace>().flatten().map(|w| w.downgrade());
    for info in crate::plugins::list() {
        let name = info.manifest.name.clone();
        let ws = ws.clone();
        if info.status == "not allowed" || info.status == "waiting for your approval" {
            let m = info.manifest.clone();
            items.push(
                PaletteItem::new(
                    format!("Allow Plugin: {name}"),
                    Rc::new(move |window, cx| {
                        if let Some(ws) = &ws {
                            let _ = ws.update(cx, |ws, cx| {
                                ws.pending_approvals.push(m.clone());
                                ws.ask_next_approval(window, cx);
                            });
                        }
                    }),
                )
                .dot(Dot::Busy)
                .note(info.status.clone())
                .section(LUMEN),
            );
        }
    }
    // One entry for every plugin's log, flagged when one of them is in trouble.
    let plugins = crate::plugins::list();
    if let Some(ws) = ws.clone().filter(|_| !plugins.is_empty()) {
        let troubled = plugins.iter().filter(|p| plugin_dot(&p.status) == Dot::Bad).count();
        let item = PaletteItem::new(
            "Plugin Logs…",
            Rc::new(move |window, cx| {
                let _ = ws.update(cx, |ws, cx| ws.open_plugin_logs(window, cx));
            }),
        );
        let item = item.section(LUMEN);
        items.push(match troubled {
            0 => item.note("see what a plugin printed"),
            1 => item.dot(Dot::Bad).note("1 plugin has a problem"),
            n => item.dot(Dot::Bad).note(format!("{n} plugins have a problem")),
        });
    }
    if let Some(ws) = ws.clone() {
        items.push(
            PaletteItem::new(
                plugins_entry_label(cx.try_global::<crate::plugins::PluginUpdates>().map_or(0, |u| u.0)),
                Rc::new(move |window, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.open_plugin_manager(window, cx));
                }),
            )
            .note("install, update and turn plugins on and off")
            .section(LUMEN),
        );
    }
    if let Some(ws) = ws.clone() {
        items.push(
            PaletteItem::new(
                "Reload Plugins",
                Rc::new(move |window, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.reload_plugins(window, cx));
                }),
            )
            .note("restart every plugin and re-read plugin.toml files")
            .section(LUMEN),
        );
    }
    if let Some(ws) = ws.clone() {
        items.push(
            PaletteItem::new(
                "Theme…",
                Rc::new(move |window, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.open_theme_picker(window, cx));
                }),
            )
            .note(Settings::get(cx).theme().name.clone())
            .section(LUMEN),
        );
    }
    if let Some(ws) = ws.clone() {
        items.push(
            PaletteItem::new(
                "Tab Color…",
                Rc::new(move |window, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.open_tab_color_picker(window, cx));
                }),
            )
            .note(match tab_color {
                Some(c) => TAB_COLOR_CHOICES
                    .iter()
                    .find(|(_, o)| *o == Some(c))
                    .map_or_else(|| c.name(), |(l, _)| l.to_string()),
                None => String::new(),
            })
            .section(LUMEN),
        );
    }
    items
}

/// The tab colour picker's rows: the theme's six colours, then none.
const TAB_COLOR_CHOICES: [(&str, Option<TabColor>); 7] = [
    ("Red", Some(TabColor::Ansi(1))),
    ("Green", Some(TabColor::Ansi(2))),
    ("Yellow", Some(TabColor::Ansi(3))),
    ("Blue", Some(TabColor::Ansi(4))),
    ("Magenta", Some(TabColor::Ansi(5))),
    ("Cyan", Some(TabColor::Ansi(6))),
    ("No Colour", None),
];

/// The row the tab colour picker opens on: the tab's colour, or the top for
/// one that isn't listed.
fn tab_color_choice(color: Option<TabColor>) -> usize {
    TAB_COLOR_CHOICES.iter().position(|(_, c)| *c == color).unwrap_or(0)
}

/// The theme picker's items: every theme, the one shown marked.
fn theme_picker_items(
    names: &[SharedString],
    current: usize,
    original: crate::settings::ThemeChoice,
) -> Vec<PaletteItem> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let item =
                PaletteItem::new(name.clone(), Rc::new(move |_, cx| Settings::finish_preview(cx, original, Some(i))));
            if i == current { item.note("current") } else { item }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_has_plugins_not_the_gallery() {
        assert_eq!(plugins_entry_label(0), "Plugins");
        assert_eq!(plugins_entry_label(1), "Plugins (1 update)");
        assert_eq!(plugins_entry_label(3), "Plugins (3 updates)");
    }

    #[test]
    fn the_tab_colour_picker_offers_the_theme_s_six_and_none() {
        let names: Vec<_> = TAB_COLOR_CHOICES.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "No Colour"]);
        assert_eq!(TAB_COLOR_CHOICES[0].1, Some(TabColor::Ansi(1)));
        assert_eq!(TAB_COLOR_CHOICES[5].1, Some(TabColor::Ansi(6)));
        assert_eq!(TAB_COLOR_CHOICES[6].1, None);
        // It opens on the tab's colour: none is the last row, and a colour not
        // in the list (a script's #rrggbb) starts at the top.
        assert_eq!(tab_color_choice(Some(TabColor::Ansi(4))), 3);
        assert_eq!(tab_color_choice(None), 6);
        assert_eq!(tab_color_choice(Some(TabColor::Rgb(1, 2, 3))), 0);
    }
}
