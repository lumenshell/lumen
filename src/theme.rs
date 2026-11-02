//! Color themes. Each theme colors both the terminal grid and the app chrome.

use gpui::{Hsla, Rgba, rgb};

#[derive(Clone)]
pub struct Theme {
    pub name: &'static str,
    pub is_light: bool,
    /// Window shell behind the panes.
    pub shell: Hsla,
    /// Terminal / pane background.
    pub pane: Hsla,
    /// Raised surfaces: active tab, palette, hover.
    pub raised: Hsla,
    /// Hairline borders.
    pub border: Hsla,
    pub text: Hsla,
    pub subtext: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
    pub ansi: [Hsla; 16],
}

fn c(hex: u32) -> Hsla {
    let rgba: Rgba = rgb(hex);
    rgba.into()
}

fn ansi(hex: [u32; 16]) -> [Hsla; 16] {
    hex.map(c)
}

fn with_alpha(color: Hsla, a: f32) -> Hsla {
    Hsla { a, ..color }
}

pub fn themes() -> Vec<Theme> {
    vec![
        {
            let accent = c(0xc4a7e7);
            Theme {
                name: "Rosé Pine",
                is_light: false,
                shell: c(0x12101b),
                pane: c(0x191724),
                raised: c(0x26233a),
                border: c(0x2a2738),
                text: c(0xe0def4),
                subtext: c(0x908caa),
                muted: c(0x6e6a86),
                accent,
                cursor: c(0xebbcba),
                selection: with_alpha(c(0x403d52), 0.9),
                ansi: ansi([
                    0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
                    0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
                ]),
            }
        },
        {
            Theme {
                name: "Rosé Pine Moon",
                is_light: false,
                shell: c(0x1a1829),
                pane: c(0x232136),
                raised: c(0x393552),
                border: c(0x34304b),
                text: c(0xe0def4),
                subtext: c(0x908caa),
                muted: c(0x6e6a86),
                accent: c(0x9ccfd8),
                cursor: c(0xea9a97),
                selection: with_alpha(c(0x44415a), 0.9),
                ansi: ansi([
                    0x393552, 0xeb6f92, 0x3e8fb0, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4,
                    0x6e6a86, 0xeb6f92, 0x3e8fb0, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4,
                ]),
            }
        },
        {
            Theme {
                name: "Catppuccin Mocha",
                is_light: false,
                shell: c(0x11111b),
                pane: c(0x1e1e2e),
                raised: c(0x313244),
                border: c(0x2a2b3c),
                text: c(0xcdd6f4),
                subtext: c(0xa6adc8),
                muted: c(0x6c7086),
                accent: c(0xcba6f7),
                cursor: c(0xf5e0dc),
                selection: with_alpha(c(0x45475a), 0.9),
                ansi: ansi([
                    0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
                    0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
                ]),
            }
        },
        {
            Theme {
                name: "Tokyo Night",
                is_light: false,
                shell: c(0x111219),
                pane: c(0x1a1b26),
                raised: c(0x292e42),
                border: c(0x232433),
                text: c(0xc0caf5),
                subtext: c(0xa9b1d6),
                muted: c(0x565f89),
                accent: c(0x7aa2f7),
                cursor: c(0xc0caf5),
                selection: with_alpha(c(0x283457), 0.95),
                ansi: ansi([
                    0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
                    0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
                ]),
            }
        },
        {
            Theme {
                name: "Rosé Pine Dawn",
                is_light: true,
                shell: c(0xf2e9e1),
                pane: c(0xfaf4ed),
                raised: c(0xfffaf3),
                border: c(0xdfdad9),
                text: c(0x575279),
                subtext: c(0x797593),
                muted: c(0x9893a5),
                accent: c(0x907aa9),
                cursor: c(0xd7827e),
                selection: with_alpha(c(0xdfdad9), 0.95),
                ansi: ansi([
                    0xf2e9e1, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
                    0x9893a5, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
                ]),
            }
        },
    ]
}

/// Resolve one of the 256 indexed colors (indices 16..=255 are the xterm cube and grays).
pub fn indexed(theme: &Theme, idx: u8) -> Hsla {
    let idx = idx as u32;
    if idx < 16 {
        return theme.ansi[idx as usize];
    }
    if idx < 232 {
        let i = idx - 16;
        let level = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
        let (r, g, b) = (level(i / 36), level((i / 6) % 6), level(i % 6));
        return c((r << 16) | (g << 8) | b);
    }
    let v = 8 + (idx - 232) * 10;
    c((v << 16) | (v << 8) | v)
}

pub fn alpha(color: Hsla, a: f32) -> Hsla {
    with_alpha(color, a)
}
