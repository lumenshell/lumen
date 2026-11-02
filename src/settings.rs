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

use crate::theme::{Theme, themes};

pub const MONO_FONT: &str = "JetBrains Mono";
pub const UI_FONT: &str = "Inter";
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

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
    /// Cursor shape used until a program picks one.
    pub cursor_shape: CursorShape,
    /// Program and arguments for new panes; `None` uses `$SHELL`.
    pub shell: Option<Vec<String>>,
    /// Load Lumen's shell integration into bash, zsh and fish.
    pub shell_integration: bool,
    /// Problems found in the config file, shown until it's fixed.
    pub problems: Vec<String>,
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
            cursor_shape: CursorShape::Block,
            shell: None,
            shell_integration: true,
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
        CursorStyle { shape: self.cursor_shape, blinking: false }
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
                other => Some(format!("unknown setting `{other}`")),
            };
            s.problems.extend(problem);
        }
        s
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
             \n\
             # Cursor shape until a program picks one: \"block\", \"beam\" or \"underline\".\n\
             cursor-style = \"{cursor}\"\n\
             \n\
             # Program (and arguments) to run in new panes. Uses $SHELL when unset.\n\
             {shell}\n\
             \n\
             # Let bash, zsh and fish mark prompts and commands, for jumping between\n\
             # commands, copying output and marking failures. Applies to new panes.\n\
             shell-integration = {integration}\n",
            themes = names.join(", "),
            theme = self.theme().name,
            family = self.font_family,
            size = self.font_size,
            line_height = self.line_height,
            padding = self.padding,
            scrollback = self.scrollback,
            integration = self.shell_integration,
        )
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
        // A file that doesn't parse at all keeps the current settings.
        if new.problems.iter().any(|p| p.starts_with(PARSE_ERROR)) {
            let problems = new.problems;
            cx.global_mut::<Settings>().problems = problems;
        } else if *Self::get(cx) != new {
            cx.set_global(new);
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

    #[test]
    fn template_round_trips() {
        let s = Settings { font_size: 15.5, padding: 8.0, scrollback: 500, cursor_shape: CursorShape::Beam, ..Default::default() };
        let parsed = Settings::parse(&s.template());
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert!(parsed == s);
    }

    #[test]
    fn reads_every_setting() {
        let s = Settings::parse(
            "theme = \"tokyo night\"\nfont-family = \"Fira Code\"\nfont-size = 16\nline-height = 1.5\n\
             padding = 20\nscrollback = 2000\ncursor-style = \"underline\"\nshell = \"/bin/zsh -l\"\n",
        );
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert_eq!(s.theme().name, "Tokyo Night");
        assert_eq!(s.font_family, "Fira Code");
        assert_eq!((s.font_size, s.line_height, s.padding, s.scrollback), (16.0, 1.5, 20.0, 2000));
        assert_eq!(s.cursor_shape, CursorShape::Underline);
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
    fn legacy_file_converts() {
        let s = Settings::parse(&legacy_to_toml("# old\ntheme = Tokyo Night\nfont-size = 16\nline-height = 1.4\n"));
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert_eq!((s.theme().name, s.font_size), ("Tokyo Night", 16.0));
    }
}
