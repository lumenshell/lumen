//! Themes from files in `~/.config/lumen/themes/`: Lumen's own TOML format,
//! or Ghostty theme files as they are.

use gpui::Hsla;

use crate::theme::{PartialChrome, Theme, derive, themes};

/// A theme from a file's text: Lumen's TOML format when the text is TOML with
/// a `background`, otherwise a Ghostty theme (`key = value` lines). The error
/// says what's wrong, for the banner.
pub fn parse(name: &str, text: &str) -> Result<Theme, String> {
    match text.parse::<toml_edit::DocumentMut>() {
        Ok(doc) if doc.contains_key("background") => parse_lumen(name, &doc),
        _ => parse_ghostty(name, text),
    }
}

/// `#rrggbb` or `rrggbb`.
pub fn color(text: &str) -> Option<Hsla> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    let rgba: gpui::Rgba = gpui::rgb(v);
    Some(rgba.into())
}

fn parse_lumen(name: &str, doc: &toml_edit::DocumentMut) -> Result<Theme, String> {
    let get = |table: &toml_edit::Table, key: &str| -> Result<Option<Hsla>, String> {
        match table.get(key) {
            None => Ok(None),
            Some(item) => item
                .as_str()
                .and_then(color)
                .map(Some)
                .ok_or_else(|| format!("`{key}` should be a colour like \"#1e1e2e\"")),
        }
    };
    let root = doc.as_table();
    let background = get(root, "background")?.ok_or("`background` is missing")?;
    let foreground = get(root, "foreground")?.ok_or("`foreground` is missing")?;
    let palette = match root.get("palette") {
        None => themes()[0].ansi,
        Some(item) => {
            let list = item.as_array().ok_or("`palette` should be a list of 16 colours")?;
            let colors: Option<Vec<Hsla>> = list.iter().map(|v| v.as_str().and_then(color)).collect();
            let colors = colors.ok_or("`palette` should be a list of 16 colours like \"#1e1e2e\"")?;
            colors.try_into().map_err(|v: Vec<Hsla>| format!("`palette` has {} colours; it needs 16", v.len()))?
        }
    };
    let chrome = match root.get("chrome") {
        None => PartialChrome::default(),
        Some(item) => {
            let t = item.as_table().ok_or("`chrome` should be a [chrome] table")?;
            PartialChrome {
                shell: get(t, "shell")?,
                raised: get(t, "raised")?,
                border: get(t, "border")?,
                subtext: get(t, "subtext")?,
                muted: get(t, "muted")?,
                accent: get(t, "accent")?,
            }
        }
    };
    Ok(derive(name, background, foreground, palette, get(root, "cursor")?, get(root, "selection")?, chrome))
}

fn parse_ghostty(name: &str, text: &str) -> Result<Theme, String> {
    let mut palette = themes()[0].ansi;
    let (mut background, mut foreground, mut cursor, mut selection) = (None, None, None, None);
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected `key = value`", n + 1));
        };
        let (key, value) = (key.trim(), value.trim());
        let bad = || format!("line {}: `{key}` should be a colour like #1e1e2e", n + 1);
        match key {
            "palette" => {
                let (index, hex) = value.split_once('=').ok_or_else(|| format!("line {}: expected `palette = N=#rrggbb`", n + 1))?;
                let index: usize = index.trim().parse().map_err(|_| format!("line {}: bad palette number", n + 1))?;
                if index < 16 {
                    palette[index] = color(hex).ok_or_else(bad)?;
                }
            }
            "background" => background = Some(color(value).ok_or_else(bad)?),
            "foreground" => foreground = Some(color(value).ok_or_else(bad)?),
            "cursor-color" => cursor = Some(color(value).ok_or_else(bad)?),
            "selection-background" => selection = Some(color(value).ok_or_else(bad)?),
            // Other Ghostty keys (fonts, cursor text colour…) don't matter here.
            _ => {}
        }
    }
    let background = background.ok_or("`background` is missing")?;
    let foreground = foreground.ok_or("`foreground` is missing")?;
    Ok(derive(name, background, foreground, palette, cursor, selection, PartialChrome::default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Rgba, rgb};

    fn c(hex: u32) -> Hsla {
        let rgba: Rgba = rgb(hex);
        rgba.into()
    }

    const PALETTE: &str = r##"palette = ["#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
           "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4"]"##;

    fn lumen_file(chrome: bool) -> String {
        let mut text = format!(
            "background = \"#191724\"\nforeground = \"#e0def4\"\ncursor = \"#ebbcba\"\nselection = \"#403d52\"\n{PALETTE}\n"
        );
        if chrome {
            text.push_str(
                "[chrome]\nshell = \"#12101b\"\nraised = \"#26233a\"\nborder = \"#2a2738\"\n\
                 subtext = \"#908caa\"\nmuted = \"#6e6a86\"\naccent = \"#c4a7e7\"\n",
            );
        }
        text
    }

    #[test]
    fn lumen_format_parses() {
        let t = parse("Midnight", &lumen_file(true)).unwrap();
        assert_eq!(t.name.as_ref(), "Midnight");
        assert!(!t.is_light);
        assert_eq!(t.pane, c(0x191724));
        assert_eq!(t.text, c(0xe0def4));
        assert_eq!(t.cursor, c(0xebbcba));
        assert_eq!(t.ansi[1], c(0xeb6f92));
        assert_eq!(t.ansi[15], c(0xe0def4));
        assert_eq!(t.shell, c(0x12101b));
        assert_eq!(t.raised, c(0x26233a));
        assert_eq!(t.border, c(0x2a2738));
        assert_eq!(t.subtext, c(0x908caa));
        assert_eq!(t.muted, c(0x6e6a86));
        assert_eq!(t.accent, c(0xc4a7e7));
    }

    #[test]
    fn lumen_format_derives_missing_chrome() {
        let t = parse("Midnight", &lumen_file(false)).unwrap();
        let full = parse("Midnight", &lumen_file(true)).unwrap();
        let derived = derive(
            "Midnight",
            full.pane,
            full.text,
            full.ansi,
            Some(full.cursor),
            Some(c(0x403d52)),
            PartialChrome::default(),
        );
        assert!(t == derived);
    }

    #[test]
    fn ghostty_format_parses() {
        let mut text = String::new();
        for i in 0..16 {
            text.push_str(&format!("palette = {i}=#{:02x}{:02x}{:02x}\n", i * 10, i * 10, i * 10));
        }
        text.push_str("background = 1e1e2e\nforeground = #cdd6f4\ncursor-color = #f5e0dc\nselection-background = #585b70\n");
        let t = parse("Mocha", &text).unwrap();
        assert_eq!(t.pane, c(0x1e1e2e));
        assert_eq!(t.text, c(0xcdd6f4));
        assert_eq!(t.cursor, c(0xf5e0dc));
        assert_eq!(t.ansi[3], c(0x1e1e1e));
        assert_eq!(t.ansi[15], c(0x969696));
        assert!(!t.is_light);
    }

    #[test]
    fn ghostty_partial_palette_falls_back() {
        let t = parse("Partial", "background = #fafafa\nforeground = #202020\npalette = 1=#ff0000\n").unwrap();
        assert_eq!(t.ansi[1], c(0xff0000));
        let default = &themes()[0];
        assert_eq!(t.ansi[2], default.ansi[2]);
        assert!(t.is_light);
    }

    #[test]
    fn bad_files_say_why() {
        let e = parse("Bad", "background = \"#12345\"\nforeground = \"#ffffff\"\n").unwrap_err();
        assert!(e.contains("background"), "{e}");
        let short = r##"background = "#000000"
foreground = "#ffffff"
palette = ["#000000", "#000000", "#000000", "#000000", "#000000", "#000000", "#000000", "#000000",
           "#000000", "#000000", "#000000", "#000000", "#000000", "#000000", "#000000"]"##;
        let e = parse("Bad", short).unwrap_err();
        assert!(e.contains("16"), "{e}");
        let e = parse("Bad", "foreground = #ffffff\n").unwrap_err();
        assert!(e.contains("background"), "{e}");
    }
}
