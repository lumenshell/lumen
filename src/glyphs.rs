//! Glyph cache for drawing the terminal grid cell by cell.
//!
//! Shaping a whole line costs about half a millisecond on Linux, so shaping
//! every row that changed held fast output to ~25 fps. Instead each distinct
//! piece of text is shaped once per font and size, and frames are drawn from the
//! cached glyphs. Runs of operator characters (`=>`, `<=`, `===`) are shaped
//! together so the font's ligatures survive.

use std::collections::HashMap;

use gpui::{App, Font, FontId, GlyphId, Global, Hsla, Pixels, SharedString, TextRun, WindowTextSystem};

#[derive(Clone, Copy)]
pub struct Glyph {
    pub font_id: FontId,
    pub id: GlyphId,
    /// Offset from the start of the text that was shaped.
    pub x: Pixels,
    pub emoji: bool,
}

/// Characters that take part in programming ligatures.
pub fn joins(c: char) -> bool {
    matches!(
        c,
        '!' | '#' | '$' | '%' | '&' | '*' | '+' | '-' | '.' | '/' | ':' | ';' | '<' | '=' | '>'
            | '?' | '@' | '\\' | '^' | '_' | '|' | '~'
    )
}

/// Longest operator run shaped as one piece; longer runs are split.
pub const MAX_JOIN: usize = 24;

/// Upper bound on cached entries before the cache starts over, so unusual
/// output (huge numbers of distinct combining sequences) can't grow it forever.
const MAX_ENTRIES: usize = 8192;

#[derive(Default)]
pub struct GlyphCache {
    size: Option<Pixels>,
    /// Ascent and descent of the primary font, for placing the baseline.
    metrics: Option<(Pixels, Pixels)>,
    chars: HashMap<(char, u8), Box<[Glyph]>>,
    /// Indexed by font variant, so lookups can borrow the text.
    clusters: [HashMap<String, Box<[Glyph]>>; 4],
}

impl Global for GlyphCache {}

impl GlyphCache {
    pub fn get(cx: &mut App, font_size: Pixels) -> &mut Self {
        let cache = cx.default_global::<Self>();
        if cache.size != Some(font_size) {
            *cache = Self { size: Some(font_size), ..Default::default() };
        }
        cache
    }

    pub fn metrics(&mut self, text: &WindowTextSystem, font: &Font) -> (Pixels, Pixels) {
        let size = self.size.unwrap_or_default();
        *self.metrics.get_or_insert_with(|| {
            let run = TextRun {
                len: 1,
                font: font.clone(),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = text.shape_line("m".into(), size, &[run], None);
            (line.ascent, line.descent)
        })
    }

    /// Glyphs for one character in font variant `style` (an index into `fonts`).
    pub fn char(&mut self, text: &WindowTextSystem, fonts: &[Font], style: u8, c: char) -> &[Glyph] {
        if self.chars.len() > MAX_ENTRIES {
            self.chars.clear();
        }
        let size = self.size.unwrap_or_default();
        self.chars.entry((c, style)).or_insert_with(|| {
            let mut buf = [0; 4];
            shape(text, &fonts[style as usize], size, c.encode_utf8(&mut buf))
        })
    }

    /// Glyphs for a multi-character string: a character with combining marks,
    /// or a run of operator characters that may form a ligature.
    pub fn cluster(&mut self, text: &WindowTextSystem, fonts: &[Font], style: u8, s: &str) -> &[Glyph] {
        let map = &mut self.clusters[style as usize];
        if map.len() > MAX_ENTRIES {
            map.clear();
        }
        if !map.contains_key(s) {
            let size = self.size.unwrap_or_default();
            map.insert(s.to_string(), shape(text, &fonts[style as usize], size, s));
        }
        &map[s]
    }
}

fn shape(text: &WindowTextSystem, font: &Font, size: Pixels, s: &str) -> Box<[Glyph]> {
    let run = TextRun {
        len: s.len(),
        font: font.clone(),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = text.shape_line(SharedString::from(s.to_string()), size, &[run], None);
    line.runs
        .iter()
        .flat_map(|run| {
            run.glyphs.iter().map(|g| Glyph {
                font_id: run.font_id,
                id: g.id,
                x: g.position.x,
                emoji: g.is_emoji,
            })
        })
        .collect()
}
