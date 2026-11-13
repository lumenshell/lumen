//! App-wide settings, stored as a GPUI global and read from
//! `~/.config/lumen/config.toml`.
//!
//! The file is watched: saving it applies the changes to every window and
//! pane without a restart. Mistakes don't take effect; they're listed in a
//! banner until fixed. When Lumen itself changes a setting (switching theme,
//! zooming the font) it edits just that key, keeping the file's comments and
//! layout.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use gpui::{App, Global, Pixels, SharedString, px};
use toml_edit::{DocumentMut, Item, value};

use crate::keymap;
use crate::theme::{Theme, themes};

pub const MONO_FONT: &str = "JetBrains Mono";
pub const UI_FONT: &str = "Inter";
/// Mac terminals default smaller (Terminal 11, Ghostty 13), and 14 looks big beside them.
#[cfg(target_os = "macos")]
pub const DEFAULT_FONT_SIZE: f32 = 13.0;
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// The UI font from the settings, for code that draws chrome without an `App`
/// at hand. Set by [`publish_ui_font`] whenever the settings change.
static UI_FAMILY: std::sync::RwLock<Option<SharedString>> = std::sync::RwLock::new(None);

/// The font for tabs, palette and banners (`ui-font-family`).
pub fn ui_font() -> SharedString {
    UI_FAMILY.read().ok().and_then(|f| f.clone()).unwrap_or_else(|| UI_FONT.into())
}

pub fn publish_ui_font(settings: &Settings) {
    if let Ok(mut f) = UI_FAMILY.write() {
        *f = Some(settings.ui_font_family.clone());
    }
}

/// Prefix for a file that couldn't be read at all.
const PARSE_ERROR: &str = "Couldn't read the file at";

#[derive(Clone, PartialEq)]
pub struct Settings {
    pub themes: Vec<Theme>,
    pub theme_index: usize,
    /// Font for terminal text (the UI keeps its own fonts).
    pub font_family: SharedString,
    pub font_size: f32,
    pub line_height: f32,
    /// Space between a pane's edge and its text, in pixels.
    pub padding: f32,
    /// Lines of history kept per pane.
    pub scrollback: usize,
    /// Show a scrollbar, with a mark for each command, while you scroll.
    pub scrollbar: bool,
    /// The quick terminal's height, as a share of the monitor's (0.2 to 1).
    pub quick_terminal_height: f32,
    /// Hide the quick terminal when you click into another window.
    pub quick_terminal_autohide: bool,
    /// A key Lumen grabs from anywhere to toggle the quick terminal (X11),
    /// like "f12" or "ctrl-grave"; empty for none.
    pub quick_terminal_key: String,
    /// Cursor shape used until a program picks one.
    pub cursor_shape: CursorShape,
    /// Blink the cursor. Off, nothing blinks, even when a program asks.
    pub cursor_blink: bool,
    /// How opaque the window background is, from 0.3 to 1 (fully opaque).
    pub background_opacity: f32,
    /// Program and arguments for new panes; `None` uses `$SHELL`.
    pub shell: Option<Vec<String>>,
    /// Load Lumen's shell integration into bash, zsh and fish.
    pub shell_integration: bool,
    /// Let scripts control Lumen over a socket (`lumen msg`).
    pub socket_api: bool,
    /// Notify when a command that ran at least this many seconds finishes
    /// out of sight; 0 turns it off.
    pub notify_after: u64,
    /// Shortcuts from `[keys]`: normalized keys and the action they run
    /// (`None` frees the key), in file order.
    pub keys: Vec<(String, Option<String>)>,
    /// Built-in plugins to run, by name (the Plugin Gallery edits this).
    pub plugins: Vec<String>,
    /// Bold text in colours 0-7 uses the bright 8-15.
    pub bold_is_bright: bool,
    /// Shape operator runs together so ligatures form.
    pub ligatures: bool,
    /// How long the cursor stays on, then off, while blinking.
    pub cursor_blink_interval: Duration,
    /// Selecting also copies to the clipboard.
    pub copy_on_select: bool,
    /// Characters that end a word for double-click; `None` keeps alacritty's.
    pub word_separators: Option<String>,
    /// Lines the mouse wheel scrolls per notch.
    pub scroll_speed: f32,
    pub right_click: RightClick,
    /// The key held to click links.
    pub link_modifier: LinkModifier,
    /// (macOS) Option sends Alt instead of typing special characters.
    pub option_as_alt: bool,
    /// Let programs turn on the kitty keyboard protocol.
    pub kitty_keyboard: bool,
    /// Size of a new window when there's no saved one.
    pub window_width: f32,
    pub window_height: f32,
    /// Restore windows on launch, and save them.
    pub restore_session: bool,
    /// Where new tabs, splits and windows start.
    pub working_directory: WorkingDirectory,
    /// How much to dim panes other than the focused one (0 = not at all);
    /// `None` keeps the theme's own amount.
    pub unfocused_dim: Option<f32>,
    /// Space between split panes, in pixels.
    pub pane_gap: f32,
    /// Font for tabs, palette and banners.
    pub ui_font_family: SharedString,
    pub bell: Bell,
    /// Problems found in the config file, shown until it's fixed.
    pub problems: Vec<String>,
}

/// What a right click does when no program has asked for the mouse.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RightClick {
    None,
    Paste,
    /// Copy a selection if there is one, otherwise paste.
    CopyOrPaste,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LinkModifier {
    Ctrl,
    Cmd,
    Alt,
    /// A plain click opens links.
    None,
}

impl Default for LinkModifier {
    fn default() -> Self {
        if cfg!(target_os = "macos") { LinkModifier::Cmd } else { LinkModifier::Ctrl }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum WorkingDirectory {
    /// The focused pane's folder.
    Inherit,
    Home,
    Path(PathBuf),
}

/// What a bell does in a pane.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Bell {
    Off,
    /// Flash the pane, and mark its tab if you're not looking.
    Flash,
}

impl Global for Settings {}

impl Default for Settings {
    fn default() -> Self {
        Self {
            themes: themes(),
            theme_index: 0,
            font_family: MONO_FONT.into(),
            font_size: DEFAULT_FONT_SIZE,
            line_height: 1.35,
            padding: 12.0,
            scrollback: 10_000,
            scrollbar: true,
            quick_terminal_height: 0.45,
            quick_terminal_autohide: false,
            quick_terminal_key: String::new(),
            cursor_shape: CursorShape::Block,
            cursor_blink: true,
            background_opacity: 1.0,
            shell: None,
            shell_integration: true,
            socket_api: true,
            notify_after: 10,
            keys: Vec::new(),
            plugins: Vec::new(),
            bold_is_bright: true,
            ligatures: true,
            cursor_blink_interval: Duration::from_millis(530),
            copy_on_select: false,
            word_separators: None,
            scroll_speed: 3.0,
            right_click: RightClick::None,
            link_modifier: LinkModifier::default(),
            option_as_alt: false,
            kitty_keyboard: true,
            window_width: 1100.0,
            window_height: 720.0,
            restore_session: true,
            working_directory: WorkingDirectory::Inherit,
            unfocused_dim: None,
            pane_gap: 8.0,
            ui_font_family: UI_FONT.into(),
            bell: Bell::Off,
            problems: Vec::new(),
        }
    }
}

fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("lumen"))
}

pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.toml"))
}

/// The `key = value` file used before config.toml.
fn legacy_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config"))
}

/// Store a value; no problem.
fn set<T>(slot: &mut T, v: T) -> Option<String> {
    *slot = v;
    None
}

/// A true/false setting, or the problem with it.
fn flag(item: &Item, slot: &mut bool, bad: impl Fn(&str) -> String) -> Option<String> {
    match item.as_bool() {
        Some(v) => set(slot, v),
        None => Some(bad("true or false")),
    }
}

/// `~/x` → `$HOME/x`.
fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ if p == "~" => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(p)),
        _ => PathBuf::from(p),
    }
}

fn number(item: &Item) -> Option<f64> {
    item.as_float().or_else(|| item.as_integer().map(|i| i as f64))
}

impl Settings {
    pub fn get(cx: &App) -> &Settings {
        cx.global::<Settings>()
    }

    pub fn theme(&self) -> &Theme {
        &self.themes[self.theme_index]
    }

    pub fn font_size(&self) -> Pixels {
        px(self.font_size)
    }

    pub fn cursor_style(&self) -> CursorStyle {
        CursorStyle { shape: self.cursor_shape, blinking: self.cursor_blink }
    }

    /// Settings from config text. Problems are collected rather than fatal:
    /// each bad value keeps its default.
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        let doc = match text.parse::<DocumentMut>() {
            Ok(doc) => doc,
            Err(e) => {
                let line = e.span().map(|r| text[..r.start.min(text.len())].lines().count().max(1));
                let message = e.message().trim().trim_end_matches('.');
                s.problems.push(match line {
                    Some(n) => format!("{PARSE_ERROR} line {n}: {message}"),
                    None => format!("{PARSE_ERROR} {message}"),
                });
                return s;
            }
        };
        for (key, item) in doc.iter() {
            if key == "keys" {
                s.parse_keys(item);
                continue;
            }
            let bad = |what: &str| format!("`{key}` should be {what}");
            let problem = match key {
                "theme" => match item.as_str() {
                    Some(name) => match s.themes.iter().position(|t| t.name.eq_ignore_ascii_case(name)) {
                        Some(i) => {
                            s.theme_index = i;
                            None
                        }
                        None => {
                            let names: Vec<_> = s.themes.iter().map(|t| format!("\"{}\"", t.name)).collect();
                            Some(format!("unknown theme \"{name}\"; choose one of {}", names.join(", ")))
                        }
                    },
                    None => Some(bad("a theme name in quotes")),
                },
                "font-family" => match item.as_str() {
                    Some(f) if !f.trim().is_empty() => {
                        s.font_family = f.trim().to_string().into();
                        None
                    }
                    _ => Some(bad("a font name in quotes")),
                },
                "font-size" => match number(item) {
                    Some(v) if (6.0..=72.0).contains(&v) => {
                        s.font_size = v as f32;
                        None
                    }
                    _ => Some(bad("a number from 6 to 72")),
                },
                "line-height" => match number(item) {
                    Some(v) if (1.0..=3.0).contains(&v) => {
                        s.line_height = v as f32;
                        None
                    }
                    _ => Some(bad("a number from 1.0 to 3.0")),
                },
                "padding" => match number(item) {
                    Some(v) if (0.0..=64.0).contains(&v) => {
                        s.padding = v as f32;
                        None
                    }
                    _ => Some(bad("a number of pixels from 0 to 64")),
                },
                "scrollback" => match item.as_integer() {
                    Some(v) if (0..=1_000_000).contains(&v) => {
                        s.scrollback = v as usize;
                        None
                    }
                    _ => Some(bad("a whole number of lines from 0 to 1000000")),
                },
                "cursor-style" => match item.as_str().map(str::to_ascii_lowercase).as_deref() {
                    Some("block") => {
                        s.cursor_shape = CursorShape::Block;
                        None
                    }
                    Some("beam" | "bar") => {
                        s.cursor_shape = CursorShape::Beam;
                        None
                    }
                    Some("underline") => {
                        s.cursor_shape = CursorShape::Underline;
                        None
                    }
                    _ => Some(bad("\"block\", \"beam\" or \"underline\"")),
                },
                "quick-terminal-height" => match number(item) {
                    Some(v) if (0.2..=1.0).contains(&v) => {
                        s.quick_terminal_height = v as f32;
                        None
                    }
                    _ => Some(bad("a number from 0.2 to 1")),
                },
                "quick-terminal-autohide" => match item.as_bool() {
                    Some(v) => {
                        s.quick_terminal_autohide = v;
                        None
                    }
                    _ => Some(bad("true or false")),
                },
                "quick-terminal-key" => match item.as_str() {
                    Some(v) => match crate::hotkey::parse(v) {
                        Ok(_) => {
                            s.quick_terminal_key = v.trim().to_lowercase();
                            None
                        }
                        Err(e) => Some(bad(&e)),
                    },
                    _ => Some(bad("a key in quotes, like \"f12\" or \"ctrl-grave\"")),
                },
                "scrollbar" => match item.as_bool() {
                    Some(v) => {
                        s.scrollbar = v;
                        None
                    }
                    _ => Some(bad("true or false")),
                },
                "cursor-blink" => match item.as_bool() {
                    Some(v) => {
                        s.cursor_blink = v;
                        None
                    }
                    _ => Some(bad("true or false")),
                },
                "background-opacity" => match number(item) {
                    Some(v) if (0.3..=1.0).contains(&v) => {
                        s.background_opacity = v as f32;
                        None
                    }
                    _ => Some(bad("a number from 0.3 to 1")),
                },
                "shell" => match item.as_str().map(|v| v.split_whitespace().map(String::from).collect::<Vec<_>>()) {
                    Some(parts) if !parts.is_empty() => {
                        s.shell = Some(parts);
                        None
                    }
                    _ => Some(bad("a command in quotes, such as \"/usr/bin/fish --login\"")),
                },
                "shell-integration" => match item.as_bool() {
                    Some(v) => {
                        s.shell_integration = v;
                        None
                    }
                    None => Some(bad("true or false")),
                },
                "notify-after" => match item.as_integer() {
                    Some(v) if (0..=86_400).contains(&v) => {
                        s.notify_after = v as u64;
                        None
                    }
                    _ => Some(bad("a number of seconds from 0 (off) to 86400")),
                },
                "plugins" => match item.as_array() {
                    Some(list) => {
                        let mut bad_names = Vec::new();
                        for v in list {
                            match v.as_str() {
                                Some(n) if crate::plugins::builtin(n).is_some() => {
                                    if !s.plugins.iter().any(|p| p == n) {
                                        s.plugins.push(n.to_string());
                                    }
                                }
                                other => bad_names.push(format!("\"{}\"", other.unwrap_or("?"))),
                            }
                        }
                        (!bad_names.is_empty()).then(|| {
                            let names: Vec<_> = crate::plugins::BUILTINS.iter().map(|b| format!("\"{}\"", b.name)).collect();
                            format!("unknown built-in plugin {}; they are {}", bad_names.join(", "), names.join(", "))
                        })
                    }
                    None => Some(bad("a list of built-in plugin names, like [\"recent-dirs\"]")),
                },
                "socket-api" => match item.as_bool() {
                    Some(v) => {
                        s.socket_api = v;
                        None
                    }
                    None => Some(bad("true or false")),
                },
                "bold-is-bright" => flag(item, &mut s.bold_is_bright, bad),
                "ligatures" => flag(item, &mut s.ligatures, bad),
                "copy-on-select" => flag(item, &mut s.copy_on_select, bad),
                "option-as-alt" => flag(item, &mut s.option_as_alt, bad),
                "kitty-keyboard" => flag(item, &mut s.kitty_keyboard, bad),
                "restore-session" => flag(item, &mut s.restore_session, bad),
                "cursor-blink-interval" => match item.as_integer() {
                    Some(v) if (200..=2000).contains(&v) => {
                        s.cursor_blink_interval = Duration::from_millis(v as u64);
                        None
                    }
                    _ => Some(bad("a number of milliseconds from 200 to 2000")),
                },
                "word-separators" => match item.as_str() {
                    Some(v) => {
                        s.word_separators = Some(v.to_string());
                        None
                    }
                    None => Some(bad("the characters that end a word, in quotes")),
                },
                "scroll-speed" => match number(item) {
                    Some(v) if (1.0..=20.0).contains(&v) => {
                        s.scroll_speed = v as f32;
                        None
                    }
                    _ => Some(bad("a number of lines from 1 to 20")),
                },
                "right-click" => match item.as_str() {
                    Some("none") => set(&mut s.right_click, RightClick::None),
                    Some("paste") => set(&mut s.right_click, RightClick::Paste),
                    Some("copy-or-paste") => set(&mut s.right_click, RightClick::CopyOrPaste),
                    _ => Some(bad("\"none\", \"paste\" or \"copy-or-paste\"")),
                },
                "link-modifier" => match item.as_str() {
                    Some("ctrl") => set(&mut s.link_modifier, LinkModifier::Ctrl),
                    Some("cmd") => set(&mut s.link_modifier, LinkModifier::Cmd),
                    Some("alt") => set(&mut s.link_modifier, LinkModifier::Alt),
                    Some("none") => set(&mut s.link_modifier, LinkModifier::None),
                    _ => Some(bad("\"ctrl\", \"cmd\", \"alt\" or \"none\"")),
                },
                "window-width" => match number(item) {
                    Some(v) if (420.0..=8000.0).contains(&v) => set(&mut s.window_width, v as f32),
                    _ => Some(bad("a number of pixels from 420 to 8000")),
                },
                "window-height" => match number(item) {
                    Some(v) if (260.0..=8000.0).contains(&v) => set(&mut s.window_height, v as f32),
                    _ => Some(bad("a number of pixels from 260 to 8000")),
                },
                "working-directory" => match item.as_str().map(str::trim) {
                    Some("inherit") => set(&mut s.working_directory, WorkingDirectory::Inherit),
                    Some("home") => set(&mut s.working_directory, WorkingDirectory::Home),
                    Some(p) if !p.is_empty() => set(&mut s.working_directory, WorkingDirectory::Path(expand_home(p))),
                    _ => Some(bad("\"inherit\", \"home\" or a folder in quotes")),
                },
                "unfocused-dim" => match number(item) {
                    Some(v) if (0.0..=0.5).contains(&v) => set(&mut s.unfocused_dim, Some(v as f32)),
                    _ => Some(bad("a number from 0 (off) to 0.5")),
                },
                "pane-gap" => match number(item) {
                    Some(v) if (0.0..=24.0).contains(&v) => set(&mut s.pane_gap, v as f32),
                    _ => Some(bad("a number of pixels from 0 to 24")),
                },
                "ui-font-family" => match item.as_str() {
                    Some(f) if !f.trim().is_empty() => set(&mut s.ui_font_family, f.trim().to_string().into()),
                    _ => Some(bad("a font name in quotes")),
                },
                "bell" => match item.as_str() {
                    Some("off") => set(&mut s.bell, Bell::Off),
                    Some("flash") => set(&mut s.bell, Bell::Flash),
                    _ => Some(bad("\"off\" or \"flash\"")),
                },
                other => Some(format!("unknown setting `{other}`")),
            };
            s.problems.extend(problem);
        }
        s
    }

    fn parse_keys(&mut self, item: &Item) {
        let Some(table) = item.as_table_like() else {
            self.problems.push("`keys` should be a table: a `[keys]` line followed by `key = \"action\"` lines".into());
            return;
        };
        for (keys, item) in table.iter() {
            let checked = keymap::normalize_keys(keys).and_then(|k| {
                let name = item.as_str().ok_or_else(|| "should be an action name in quotes".to_string())?;
                Ok((k, keymap::check_action(name)?))
            });
            match checked {
                Ok((k, action)) => {
                    // The same key written two ways: the later line wins.
                    self.keys.retain(|(existing, _)| *existing != k);
                    self.keys.push((k, action));
                }
                Err(e) => self.problems.push(format!("[keys] `{keys}`: {e}")),
            }
        }
    }

    /// Read the config file, creating it on first run (carrying over values
    /// from the old `config` file if there is one).
    pub fn load() -> Self {
        let Some(path) = config_path() else { return Self::default() };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(_) => {
                let s = legacy_path()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .map(|t| Self::parse(&legacy_to_toml(&t)))
                    .map(|s| Settings { problems: Vec::new(), ..s })
                    .unwrap_or_default();
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&path, s.template());
                s
            }
        }
    }

    /// A commented config file holding these settings' values.
    fn template(&self) -> String {
        let names: Vec<_> = self.themes.iter().map(|t| format!("\"{}\"", t.name)).collect();
        let cursor = match self.cursor_shape {
            CursorShape::Beam => "beam",
            CursorShape::Underline => "underline",
            _ => "block",
        };
        let shell = match &self.shell {
            Some(parts) => format!("shell = \"{}\"", parts.join(" ")),
            None => "# shell = \"/usr/bin/fish --login\"".to_string(),
        };
        let plugins_list =
            format!("[{}]", self.plugins.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join(", "));
        format!(
            "# Lumen settings. Changes apply as soon as you save this file.\n\
             # Delete a line to go back to its default.\n\
             \n\
             # One of: {themes}\n\
             theme = \"{theme}\"\n\
             \n\
             # Font for terminal text: any installed monospace font. JetBrains Mono is built in.\n\
             font-family = \"{family}\"\n\
             font-size = {size}\n\
             # Line height as a multiple of the font size.\n\
             line-height = {line_height}\n\
             \n\
             # Space between a pane's edge and its text, in pixels.\n\
             padding = {padding}\n\
             \n\
             # Lines of history kept per pane.\n\
             scrollback = {scrollback}\n\
             # Show a scrollbar on a pane's right edge while you scroll, with a mark for\n\
             # each command (red if it failed). Click or drag it to move through history.\n\
             scrollbar = {scrollbar}\n\
             \n\
             # The quick terminal drops down from the top of the screen and hides again\n\
             # with the same key, keeping its shells running. Bind `lumen msg quick-terminal`\n\
             # to a key in your desktop's shortcut settings, or (X11) set a key here for\n\
             # Lumen to catch from any app, like \"f12\" or \"ctrl-grave\".\n\
             quick-terminal-key = \"{quick_key}\"\n\
             # Its height, as a share of the screen (0.2 to 1).\n\
             quick-terminal-height = {quick_height}\n\
             # Hide it when you click into another window.\n\
             quick-terminal-autohide = {quick_autohide}\n\
             \n\
             # Cursor shape until a program picks one: \"block\", \"beam\" or \"underline\".\n\
             cursor-style = \"{cursor}\"\n\
             # Blink the cursor. When false it never blinks, even if a program asks it to.\n\
             cursor-blink = {blink}\n\
             \n\
             # How opaque the window background is, from 0.3 to 1. Below 1 shows what's\n\
             # behind the window (on Linux this needs a compositor).\n\
             background-opacity = {opacity}\n\
             \n\
             # Program (and arguments) to run in new panes. Uses $SHELL when unset.\n\
             {shell}\n\
             \n\
             # Bold text in the first eight colours uses their bright versions.\n\
             bold-is-bright = {bold_is_bright}\n\
             # Join operators like -> and != into ligatures when the font has them.\n\
             ligatures = {ligatures}\n\
             # How long the cursor stays on, then off, when it blinks (milliseconds).\n\
             cursor-blink-interval = {blink_ms}\n\
             \n\
             # Selecting text also copies it to the clipboard.\n\
             copy-on-select = {copy_on_select}\n\
             # Characters that end a word when you double-click to select it.\n\
             {word_separators}\n\
             # Lines the mouse wheel scrolls per notch (1 to 20).\n\
             scroll-speed = {scroll_speed}\n\
             # Right click: \"none\", \"paste\", or \"copy-or-paste\" (copy a selection, else paste).\n\
             right-click = \"{right_click}\"\n\
             # Key to hold when clicking a link: \"ctrl\", \"cmd\", \"alt\" or \"none\" (a plain click).\n\
             link-modifier = \"{link_modifier}\"\n\
             # (macOS) Option sends Alt to programs instead of typing special characters.\n\
             option-as-alt = {option_as_alt}\n\
             # Let programs turn on the kitty keyboard protocol (used by Neovim, Helix).\n\
             kitty-keyboard = {kitty_keyboard}\n\
             # What a bell does: \"off\", or \"flash\" the pane and mark its tab.\n\
             bell = \"{bell}\"\n\
             \n\
             # Size of a new window when there's no saved one, in pixels. Next launch.\n\
             window-width = {window_width}\n\
             window-height = {window_height}\n\
             # Bring back your windows, tabs and splits next launch. Next launch.\n\
             restore-session = {restore_session}\n\
             # Where new tabs, splits and windows start: \"inherit\" (the focused pane's\n\
             # folder), \"home\", or a folder such as \"~/src\".\n\
             working-directory = {working_directory}\n\
             # Dim panes other than the focused one, from 0 (off) to 0.5. Unset, it\n\
             # follows the theme (0.35 on dark themes, 0.25 on light ones).\n\
             {unfocused_dim}\n\
             # Space between split panes, in pixels (0 to 24).\n\
             pane-gap = {pane_gap}\n\
             # Font for tabs, the palette and banners.\n\
             ui-font-family = \"{ui_font_family}\"\n\
             \n\
             # Let bash, zsh and fish mark prompts and commands, for jumping between\n\
             # commands, copying output and marking failures. Applies to new panes.\n\
             shell-integration = {integration}\n\
             \n\
             # Notify when a command that ran this many seconds or longer finishes while\n\
             # you're not looking at its pane (needs shell integration). 0 turns it off.\n\
             notify-after = {notify_after}\n\
             \n\
             # Let scripts and plugins control Lumen with `lumen msg` (a socket only you\n\
             # can open). Turning it on takes a restart; turning it off applies at once.\n\
             socket-api = {socket_api}\n\
             \n\
             # Built-in plugins to run; turn them on and off with Plugin Gallery in the\n\
             # palette. Your own plugins go in the plugins folder next to this file.\n\
             plugins = {plugins}\n\
             \n\
             # Keyboard shortcuts. Each line binds a key to an action, replacing what that\n\
             # key did before; \"none\" frees a key so programs in the terminal get it.\n\
             # An action's other keys keep working. Keys look like ctrl-shift-t, alt-1,\n\
             # cmd-k (macOS), f5, shift-insert or ctrl-pageup.\n\
             # Actions:\n\
             {actions}\n\
             {keys}",
            themes = names.join(", "),
            theme = self.theme().name,
            family = self.font_family,
            size = self.font_size,
            line_height = self.line_height,
            padding = self.padding,
            scrollback = self.scrollback,
            scrollbar = self.scrollbar,
            quick_key = self.quick_terminal_key,
            quick_height = self.quick_terminal_height,
            quick_autohide = self.quick_terminal_autohide,
            blink = self.cursor_blink,
            opacity = self.background_opacity,
            integration = self.shell_integration,
            socket_api = self.socket_api,
            notify_after = self.notify_after,
            plugins = plugins_list,
            bold_is_bright = self.bold_is_bright,
            ligatures = self.ligatures,
            blink_ms = self.cursor_blink_interval.as_millis(),
            copy_on_select = self.copy_on_select,
            word_separators = match &self.word_separators {
                Some(w) => format!("word-separators = {}", toml_edit::Value::from(w.as_str())),
                None => format!(
                    "# word-separators = {}",
                    toml_edit::Value::from(alacritty_terminal::term::SEMANTIC_ESCAPE_CHARS)
                ),
            },
            scroll_speed = self.scroll_speed,
            right_click = match self.right_click {
                RightClick::None => "none",
                RightClick::Paste => "paste",
                RightClick::CopyOrPaste => "copy-or-paste",
            },
            link_modifier = match self.link_modifier {
                LinkModifier::Ctrl => "ctrl",
                LinkModifier::Cmd => "cmd",
                LinkModifier::Alt => "alt",
                LinkModifier::None => "none",
            },
            option_as_alt = self.option_as_alt,
            kitty_keyboard = self.kitty_keyboard,
            bell = match self.bell {
                Bell::Off => "off",
                Bell::Flash => "flash",
            },
            window_width = self.window_width,
            window_height = self.window_height,
            restore_session = self.restore_session,
            working_directory = match &self.working_directory {
                WorkingDirectory::Inherit => toml_edit::Value::from("inherit"),
                WorkingDirectory::Home => toml_edit::Value::from("home"),
                WorkingDirectory::Path(p) => toml_edit::Value::from(p.display().to_string()),
            },
            unfocused_dim = match self.unfocused_dim {
                Some(v) => format!("unfocused-dim = {v}"),
                None => "# unfocused-dim = 0.35".to_string(),
            },
            pane_gap = self.pane_gap,
            ui_font_family = self.ui_font_family,
            actions = wrap_comment(&keymap::ACTIONS.join(", "), 80),
            keys = self.keys_template(),
        )
    }

    fn keys_template(&self) -> String {
        if self.keys.is_empty() {
            return "#\n# [keys]\n# ctrl-alt-t = \"new-tab\"\n# ctrl-tab = \"none\"\n".into();
        }
        let lines: Vec<_> = self
            .keys
            .iter()
            .map(|(k, a)| format!("\"{k}\" = \"{}\"", a.as_deref().unwrap_or(keymap::NONE)))
            .collect();
        format!("[keys]\n{}\n", lines.join("\n"))
    }

    /// Write the settings Lumen changes itself (theme and font size) into the
    /// file, editing only those keys. A file that doesn't parse is left alone
    /// so a half-finished edit is never overwritten.
    fn save(&self) {
        let Some(path) = config_path() else { return };
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let Ok(mut doc) = text.parse::<DocumentMut>() else { return };
        let mut changed = false;
        if doc.get("theme").and_then(Item::as_str) != Some(self.theme().name) {
            doc["theme"] = value(self.theme().name);
            changed = true;
        }
        if doc.get("font-size").and_then(number).map(|v| v as f32) != Some(self.font_size) {
            doc["font-size"] = if self.font_size.fract() == 0.0 {
                value(self.font_size as i64)
            } else {
                value(self.font_size as f64)
            };
            changed = true;
        }
        let listed: Vec<String> = doc
            .get("plugins")
            .and_then(Item::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if listed != self.plugins {
            let mut list = toml_edit::Array::new();
            for p in &self.plugins {
                list.push(p.as_str());
            }
            doc["plugins"] = value(list);
            changed = true;
        }
        if changed {
            let tmp = path.with_extension("toml.tmp");
            if std::fs::write(&tmp, doc.to_string()).is_ok() {
                let _ = std::fs::rename(tmp, &path);
            }
        }
    }

    /// Mutate settings from inside Lumen, persist them, and redraw.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut Settings)) {
        let settings = cx.global_mut::<Settings>();
        f(settings);
        settings.font_size = settings.font_size.clamp(6.0, 72.0);
        settings.save();
        cx.refresh_windows();
    }

    /// Re-read the file and apply it if anything changed.
    fn reload(cx: &mut App) {
        let Some(text) = config_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return;
        };
        let mut new = Self::parse(&text);
        if !cx.text_system().all_font_names().iter().any(|f| f.eq_ignore_ascii_case(&new.font_family)) {
            new.problems.push(format!("font \"{}\" isn't installed; using {MONO_FONT}", new.font_family));
            new.font_family = MONO_FONT.into();
        }
        if !cx.text_system().all_font_names().iter().any(|f| f.eq_ignore_ascii_case(&new.ui_font_family)) {
            new.problems.push(format!("font \"{}\" isn't installed; using {UI_FONT}", new.ui_font_family));
            new.ui_font_family = UI_FONT.into();
        }
        // A file that doesn't parse at all keeps the current settings.
        if new.problems.iter().any(|p| p.starts_with(PARSE_ERROR)) {
            let problems = new.problems;
            cx.global_mut::<Settings>().problems = problems;
        } else if *Self::get(cx) != new {
            let keys_changed = Self::get(cx).keys != new.keys;
            publish_ui_font(&new);
            cx.set_global(new);
            if keys_changed {
                keymap::apply(cx);
            }
        } else {
            return;
        }
        cx.refresh_windows();
    }

    /// Validate the loaded font now that the text system exists, then poll
    /// the file for changes.
    pub fn watch(cx: &mut App) {
        Self::reload(cx);
        let modified = || {
            config_path().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok())
        };
        let mut last: Option<SystemTime> = modified();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let now = modified();
                if now != last {
                    last = now;
                    if cx.update(Self::reload).is_err() {
                        break;
                    }
                }
            }
        })
        .detach();
    }
}

/// `text` as `#   ` comment lines no wider than `width`, broken after commas.
fn wrap_comment(text: &str, width: usize) -> String {
    let mut lines = vec![String::from("#  ")];
    for word in text.split(' ') {
        let line = lines.last_mut().unwrap();
        if line.len() + 1 + word.len() > width {
            lines.push(format!("#   {word}"));
        } else {
            line.push(' ');
            line.push_str(word);
        }
    }
    lines.join("\n")
}

/// Convert the old `key = value` file (unquoted values) to TOML.
fn legacy_to_toml(text: &str) -> String {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let (key, value) = (key.trim(), value.trim());
            match key {
                "theme" => Some(format!("theme = \"{value}\"")),
                "font-size" | "line-height" => Some(format!("{key} = {value}")),
                _ => None,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn non_default() -> &'static str {
        "bold-is-bright = false\nligatures = false\ncursor-blink-interval = 800\ncopy-on-select = true\n\
         word-separators = \" ,│\"\nscroll-speed = 5\nright-click = \"copy-or-paste\"\nlink-modifier = \"none\"\n\
         option-as-alt = true\nkitty-keyboard = false\nwindow-width = 1400\nwindow-height = 900\n\
         restore-session = false\nworking-directory = \"~/src\"\nunfocused-dim = 0.25\npane-gap = 0\n\
         ui-font-family = \"IBM Plex Sans\"\nbell = \"flash\"\n"
    }

    #[test]
    fn new_settings_default_to_todays_behaviour() {
        let s = Settings::parse("");
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert!(s.bold_is_bright && s.ligatures && s.kitty_keyboard && s.restore_session);
        assert!(!s.copy_on_select && !s.option_as_alt);
        assert_eq!(s.cursor_blink_interval, Duration::from_millis(530));
        assert_eq!(s.word_separators, None);
        assert_eq!(s.scroll_speed, 3.0);
        assert_eq!(s.right_click, RightClick::None);
        let expected = if cfg!(target_os = "macos") { LinkModifier::Cmd } else { LinkModifier::Ctrl };
        assert_eq!(s.link_modifier, expected);
        assert_eq!((s.window_width, s.window_height), (1100.0, 720.0));
        assert_eq!(s.working_directory, WorkingDirectory::Inherit);
        assert_eq!(s.unfocused_dim, None);
        assert_eq!(s.pane_gap, 8.0);
        assert_eq!(s.ui_font_family.as_ref(), UI_FONT);
        assert_eq!(s.bell, Bell::Off);
    }

    #[test]
    fn new_settings_parse() {
        let s = Settings::parse(non_default());
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert!(!s.bold_is_bright && !s.ligatures && !s.kitty_keyboard && !s.restore_session);
        assert!(s.copy_on_select && s.option_as_alt);
        assert_eq!(s.cursor_blink_interval, Duration::from_millis(800));
        assert_eq!(s.word_separators.as_deref(), Some(" ,│"));
        assert_eq!(s.scroll_speed, 5.0);
        assert_eq!(s.right_click, RightClick::CopyOrPaste);
        assert_eq!(s.link_modifier, LinkModifier::None);
        assert_eq!((s.window_width, s.window_height), (1400.0, 900.0));
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(s.working_directory, WorkingDirectory::Path(home.join("src")));
        assert_eq!(s.unfocused_dim, Some(0.25));
        assert_eq!(s.pane_gap, 0.0);
        assert_eq!(s.ui_font_family.as_ref(), "IBM Plex Sans");
        assert_eq!(s.bell, Bell::Flash);
        let s = Settings::parse("working-directory = \"home\"\nright-click = \"paste\"\nlink-modifier = \"alt\"");
        assert_eq!(s.working_directory, WorkingDirectory::Home);
        assert_eq!(s.right_click, RightClick::Paste);
        assert_eq!(s.link_modifier, LinkModifier::Alt);
    }

    #[test]
    fn new_settings_reject_bad_values() {
        let s = Settings::parse(
            "scroll-speed = 0\ncursor-blink-interval = 50\nright-click = \"menu\"\nlink-modifier = \"shift\"\n\
             unfocused-dim = 0.9\npane-gap = 30\nbell = \"loud\"\nworking-directory = \"\"\n",
        );
        assert_eq!(s.problems.len(), 8, "{:?}", s.problems);
        for key in ["scroll-speed", "cursor-blink-interval", "right-click", "link-modifier", "unfocused-dim", "pane-gap", "bell", "working-directory"] {
            assert!(s.problems.iter().any(|p| p.contains(&format!("`{key}`"))), "{key}: {:?}", s.problems);
        }
        let d = Settings::default();
        assert_eq!(s.scroll_speed, d.scroll_speed);
        assert_eq!(s.cursor_blink_interval, d.cursor_blink_interval);
        assert_eq!(s.right_click, d.right_click);
        assert_eq!(s.unfocused_dim, d.unfocused_dim);
        assert_eq!(s.pane_gap, d.pane_gap);
        assert_eq!(s.bell, d.bell);
        assert_eq!(s.working_directory, d.working_directory);
    }

    #[test]
    fn template_round_trips_new_settings() {
        let s = Settings::parse(non_default());
        let parsed = Settings::parse(&s.template());
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert!(parsed == s);
    }

    #[test]
    fn builtin_plugins_are_listed_by_name() {
        let s = Settings::parse("plugins = [\"recent-dirs\", \"command-history\", \"recent-dirs\"]");
        assert_eq!(s.plugins, ["recent-dirs", "command-history"]);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        let s = Settings::parse("plugins = [\"recent-dir\"]");
        assert!(s.problems[0].contains("unknown built-in plugin \"recent-dir\""), "{:?}", s.problems);
        assert!(s.problems[0].contains("\"recent-dirs\""));
        assert!(!Settings::parse("plugins = \"recent-dirs\"").problems.is_empty());
    }

    #[test]
    fn template_round_trips() {
        let s = Settings { font_size: 15.5, padding: 8.0, scrollback: 500, cursor_shape: CursorShape::Beam, cursor_blink: false, background_opacity: 0.85, ..Default::default() };
        let parsed = Settings::parse(&s.template());
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert!(parsed == s);
        let s = Settings {
            keys: vec![("ctrl-alt-t".into(), Some("new-tab".into())), ("ctrl-tab".into(), None)],
            ..Default::default()
        };
        let parsed = Settings::parse(&s.template());
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.keys, s.keys);
    }

    #[test]
    fn reads_every_setting() {
        let s = Settings::parse(
            "theme = \"tokyo night\"\nfont-family = \"Fira Code\"\nfont-size = 16\nline-height = 1.5\n\
             padding = 20\nscrollback = 2000\ncursor-style = \"underline\"\nshell = \"/bin/zsh -l\"\n\
             cursor-blink = false\nbackground-opacity = 0.9\n",
        );
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert_eq!(s.theme().name, "Tokyo Night");
        assert_eq!(s.font_family, "Fira Code");
        assert_eq!((s.font_size, s.line_height, s.padding, s.scrollback), (16.0, 1.5, 20.0, 2000));
        assert_eq!(s.cursor_shape, CursorShape::Underline);
        assert_eq!((s.cursor_blink, s.background_opacity), (false, 0.9));
        assert!(!s.cursor_style().blinking);
        assert!(!Settings::parse("background-opacity = 0.1").problems.is_empty());
        assert_eq!(s.shell, Some(vec!["/bin/zsh".to_string(), "-l".to_string()]));
    }

    #[test]
    fn mistakes_are_reported_and_keep_defaults() {
        let s = Settings::parse("font-size = \"big\"\nthem = \"x\"\ntheme = \"Nope\"\nscrollback = -1\n");
        assert_eq!(s.problems.len(), 4, "{:?}", s.problems);
        assert_eq!(s.font_size, DEFAULT_FONT_SIZE);
        assert_eq!(s.scrollback, 10_000);
        assert!(s.problems.iter().any(|p| p.contains("unknown setting `them`")));
    }

    #[test]
    fn broken_toml_is_a_problem_not_a_panic() {
        let s = Settings::parse("font-size = \n");
        assert_eq!(s.problems.len(), 1);
        assert!(s.problems[0].starts_with("Couldn't read the file at line 1:"), "{:?}", s.problems);
    }

    #[test]
    fn reads_keys_and_reports_bad_ones() {
        let s = Settings::parse(
            "font-size = 15\n[keys]\nCtrl-Alt-T = \"New-Tab\"\nctrl-tab = \"none\"\nctrl-shift-tbs = \"find\"\n\
             alt-x = \"new-tb\"\nalt-y = 3\nalt-shift-t = \"quit\"\nshift-alt-t = \"find\"\n",
        );
        assert_eq!(s.font_size, 15.0);
        assert_eq!(
            s.keys,
            vec![
                ("ctrl-alt-t".to_string(), Some("new-tab".to_string())),
                ("ctrl-tab".to_string(), None),
                ("alt-shift-t".to_string(), Some("find".to_string())),
            ]
        );
        assert_eq!(s.problems.len(), 3, "{:?}", s.problems);
        assert!(s.problems.iter().any(|p| p.contains("unknown key `tbs`")));
        assert!(s.problems.iter().any(|p| p.contains("did you mean \"new-tab\"")));
        assert!(s.problems.iter().any(|p| p.contains("`alt-y`: should be an action name")));
        assert!(Settings::parse("keys = 3\n").problems[0].contains("should be a table"));
    }

    #[test]
    fn legacy_file_converts() {
        let s = Settings::parse(&legacy_to_toml("# old\ntheme = Tokyo Night\nfont-size = 16\nline-height = 1.4\n"));
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert_eq!((s.theme().name, s.font_size), ("Tokyo Night", 16.0));
    }
}
