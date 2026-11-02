//! App-wide appearance settings, stored as a GPUI global and persisted to
//! `~/.config/lumen/config` as simple `key = value` lines.

use std::path::PathBuf;

use gpui::{App, Global, Pixels, px};

use crate::theme::{Theme, themes};

pub const MONO_FONT: &str = "JetBrains Mono";
pub const UI_FONT: &str = "Inter";
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

pub struct Settings {
    pub themes: Vec<Theme>,
    pub theme_index: usize,
    pub font_size: f32,
    pub line_height: f32,
}

impl Global for Settings {}

impl Default for Settings {
    fn default() -> Self {
        Self { themes: themes(), theme_index: 0, font_size: DEFAULT_FONT_SIZE, line_height: 1.35 }
    }
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("lumen").join("config"))
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

    pub fn load() -> Self {
        let mut s = Self::default();
        let Some(text) = config_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return s;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "theme" => {
                    if let Some(i) =
                        s.themes.iter().position(|t| t.name.eq_ignore_ascii_case(value))
                    {
                        s.theme_index = i;
                    }
                }
                "font-size" => {
                    if let Ok(v) = value.parse::<f32>() {
                        s.font_size = v.clamp(8.0, 32.0);
                    }
                }
                "line-height" => {
                    if let Ok(v) = value.parse::<f32>() {
                        s.line_height = v.clamp(1.0, 2.0);
                    }
                }
                _ => {}
            }
        }
        s
    }

    fn save(&self) {
        let Some(path) = config_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = format!(
            "# Lumen settings\ntheme = {}\nfont-size = {}\nline-height = {}\n",
            self.theme().name,
            self.font_size,
            self.line_height
        );
        let _ = std::fs::write(path, text);
    }

    /// Mutate settings, persist them, and redraw every window.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut Settings)) {
        let settings = cx.global_mut::<Settings>();
        f(settings);
        settings.font_size = settings.font_size.clamp(8.0, 32.0);
        settings.save();
        cx.refresh_windows();
    }
}
