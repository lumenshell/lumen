//! Color themes. Each theme colors both the terminal grid and the app chrome.

use gpui::{Hsla, Rgba, SharedString, rgb};

#[derive(Clone, PartialEq, Debug)]
pub struct Theme {
    pub name: SharedString,
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
                name: SharedString::new_static("Rosé Pine"),
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
                name: SharedString::new_static("Rosé Pine Moon"),
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
                name: SharedString::new_static("Catppuccin Mocha"),
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
                name: SharedString::new_static("Tokyo Night"),
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
                name: SharedString::new_static("Rosé Pine Dawn"),
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
        // The rest, from each project's published palette (all MIT licensed).
        Theme {
            name: SharedString::new_static("Catppuccin Frappé"),
            is_light: false,
            shell: c(0x232634),
            pane: c(0x303446),
            raised: c(0x414559),
            border: c(0x393c51),
            text: c(0xc6d0f5),
            subtext: c(0xa5adce),
            muted: c(0x737994),
            accent: c(0xca9ee6),
            cursor: c(0xf2d5cf),
            selection: with_alpha(c(0x51576d), 0.9),
            ansi: ansi([
                0x51576d, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xb5bfe2,
                0x626880, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xa5adce,
            ]),
        },
        Theme {
            name: SharedString::new_static("Catppuccin Macchiato"),
            is_light: false,
            shell: c(0x181926),
            pane: c(0x24273a),
            raised: c(0x363a4f),
            border: c(0x2f3247),
            text: c(0xcad3f5),
            subtext: c(0xa5adcb),
            muted: c(0x6e738d),
            accent: c(0xc6a0f6),
            cursor: c(0xf4dbd6),
            selection: with_alpha(c(0x494d64), 0.9),
            ansi: ansi([
                0x494d64, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xb8c0e0,
                0x5b6078, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xa5adcb,
            ]),
        },
        Theme {
            name: SharedString::new_static("Catppuccin Latte"),
            is_light: true,
            shell: c(0xdce0e8),
            pane: c(0xeff1f5),
            raised: c(0xf8f9fb),
            border: c(0xccd0da),
            text: c(0x4c4f69),
            subtext: c(0x6c6f85),
            muted: c(0x9ca0b0),
            accent: c(0x8839ef),
            cursor: c(0xdc8a78),
            selection: with_alpha(c(0xccd0da), 0.9),
            ansi: ansi([
                0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be,
                0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc,
            ]),
        },
        Theme {
            name: SharedString::new_static("Tokyo Night Storm"),
            is_light: false,
            shell: c(0x1f2335),
            pane: c(0x24283b),
            raised: c(0x2f3549),
            border: c(0x2a2f45),
            text: c(0xc0caf5),
            subtext: c(0xa9b1d6),
            muted: c(0x565f89),
            accent: c(0x7aa2f7),
            cursor: c(0xc0caf5),
            selection: with_alpha(c(0x2e3c64), 0.9),
            ansi: ansi([
                0x1d202f, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
                0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
            ]),
        },
        Theme {
            name: SharedString::new_static("Tokyo Night Day"),
            is_light: true,
            shell: c(0xd0d5e3),
            pane: c(0xe1e2e7),
            raised: c(0xe9e9ed),
            border: c(0xc4c8da),
            text: c(0x3760bf),
            subtext: c(0x6172b0),
            muted: c(0x8990b3),
            accent: c(0x2e7de9),
            cursor: c(0x3760bf),
            selection: with_alpha(c(0xb7c1e3), 0.9),
            ansi: ansi([
                0xe9e9ed, 0xf52a65, 0x587539, 0x8c6c3e, 0x2e7de9, 0x9854f1, 0x007197, 0x6172b0,
                0xa1a6c5, 0xf52a65, 0x587539, 0x8c6c3e, 0x2e7de9, 0x9854f1, 0x007197, 0x3760bf,
            ]),
        },
        Theme {
            name: SharedString::new_static("Nord"),
            is_light: false,
            shell: c(0x272c36),
            pane: c(0x2e3440),
            raised: c(0x3b4252),
            border: c(0x353b49),
            text: c(0xd8dee9),
            subtext: c(0xaab3c2),
            muted: c(0x616e88),
            accent: c(0x88c0d0),
            cursor: c(0xd8dee9),
            selection: with_alpha(c(0x434c5e), 0.9),
            ansi: ansi([
                0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
                0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
            ]),
        },
        Theme {
            name: SharedString::new_static("Gruvbox Dark"),
            is_light: false,
            shell: c(0x1d2021),
            pane: c(0x282828),
            raised: c(0x3c3836),
            border: c(0x32302f),
            text: c(0xebdbb2),
            subtext: c(0xd5c4a1),
            muted: c(0x928374),
            accent: c(0xfabd2f),
            cursor: c(0xebdbb2),
            selection: with_alpha(c(0x504945), 0.9),
            ansi: ansi([
                0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
                0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
            ]),
        },
        Theme {
            name: SharedString::new_static("Gruvbox Light"),
            is_light: true,
            shell: c(0xf2e5bc),
            pane: c(0xfbf1c7),
            raised: c(0xf9f5d7),
            border: c(0xebdbb2),
            text: c(0x3c3836),
            subtext: c(0x504945),
            muted: c(0x928374),
            accent: c(0x076678),
            cursor: c(0x3c3836),
            selection: with_alpha(c(0xd5c4a1), 0.9),
            ansi: ansi([
                0xfbf1c7, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0x7c6f64,
                0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836,
            ]),
        },
        Theme {
            name: SharedString::new_static("Kanagawa"),
            is_light: false,
            shell: c(0x16161d),
            pane: c(0x1f1f28),
            raised: c(0x2a2a37),
            border: c(0x252530),
            text: c(0xdcd7ba),
            subtext: c(0xc8c093),
            muted: c(0x727169),
            accent: c(0x957fb8),
            cursor: c(0xc8c093),
            selection: with_alpha(c(0x2d4f67), 0.9),
            ansi: ansi([
                0x090618, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093,
                0x727169, 0xe82424, 0x98bb6c, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xdcd7ba,
            ]),
        },
        Theme {
            name: SharedString::new_static("Everforest Dark"),
            is_light: false,
            shell: c(0x232a2e),
            pane: c(0x2d353b),
            raised: c(0x343f44),
            border: c(0x3d484d),
            text: c(0xd3c6aa),
            subtext: c(0x9da9a0),
            muted: c(0x7a8478),
            accent: c(0xa7c080),
            cursor: c(0xd3c6aa),
            selection: with_alpha(c(0x475258), 0.9),
            ansi: ansi([
                0x343f44, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
                0x475258, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
            ]),
        },
        Theme {
            name: SharedString::new_static("Everforest Light"),
            is_light: true,
            shell: c(0xefebd4),
            pane: c(0xfdf6e3),
            raised: c(0xfffbef),
            border: c(0xe6e2cc),
            text: c(0x5c6a72),
            subtext: c(0x708089),
            muted: c(0x939f91),
            accent: c(0x3a94c5),
            cursor: c(0x5c6a72),
            selection: with_alpha(c(0xe6e2cc), 0.9),
            ansi: ansi([
                0x5c6a72, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xe6e2cc,
                0x829181, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xf4f0d9,
            ]),
        },
        Theme {
            name: SharedString::new_static("GitHub Dark"),
            is_light: false,
            shell: c(0x010409),
            pane: c(0x0d1117),
            raised: c(0x161b22),
            border: c(0x21262d),
            text: c(0xe6edf3),
            subtext: c(0x9198a1),
            muted: c(0x6e7681),
            accent: c(0x58a6ff),
            cursor: c(0x58a6ff),
            selection: with_alpha(c(0x264f78), 0.9),
            ansi: ansi([
                0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4,
                0x6e7681, 0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xffffff,
            ]),
        },
        Theme {
            name: SharedString::new_static("GitHub Light"),
            is_light: true,
            shell: c(0xeff2f5),
            pane: c(0xffffff),
            raised: c(0xf6f8fa),
            border: c(0xd0d7de),
            text: c(0x1f2328),
            subtext: c(0x59636e),
            muted: c(0x8c959f),
            accent: c(0x0969da),
            cursor: c(0x0969da),
            selection: with_alpha(c(0xb6e3ff), 0.9),
            ansi: ansi([
                0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781,
                0x57606a, 0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
            ]),
        },
        Theme {
            name: SharedString::new_static("Flexoki Dark"),
            is_light: false,
            shell: c(0x080707),
            pane: c(0x100f0f),
            raised: c(0x282726),
            border: c(0x343331),
            text: c(0xcecdc3),
            subtext: c(0x878580),
            muted: c(0x575653),
            accent: c(0x4385be),
            cursor: c(0xcecdc3),
            selection: with_alpha(c(0x403e3c), 0.9),
            ansi: ansi([
                0x100f0f, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97, 0x3aa99f, 0x878580,
                0x575653, 0xaf3029, 0x66800b, 0xad8301, 0x205ea6, 0xa02f6f, 0x24837b, 0xcecdc3,
            ]),
        },
        Theme {
            name: SharedString::new_static("Flexoki Light"),
            is_light: true,
            shell: c(0xf2f0e5),
            pane: c(0xfffcf0),
            raised: c(0xffffff),
            border: c(0xe6e4d9),
            text: c(0x100f0f),
            subtext: c(0x6f6e69),
            muted: c(0xb7b5ac),
            accent: c(0x205ea6),
            cursor: c(0x100f0f),
            selection: with_alpha(c(0xdad8ce), 0.9),
            ansi: ansi([
                0x100f0f, 0xaf3029, 0x66800b, 0xad8301, 0x205ea6, 0xa02f6f, 0x24837b, 0x6f6e69,
                0xb7b5ac, 0xd14d41, 0x879a39, 0xd0a215, 0x4385be, 0xce5d97, 0x3aa99f, 0xf2f0e5,
            ]),
        },
        Theme {
            name: SharedString::new_static("Solarized Dark"),
            is_light: false,
            shell: c(0x00212b),
            pane: c(0x002b36),
            raised: c(0x073642),
            border: c(0x0b3b47),
            text: c(0x839496),
            subtext: c(0x657b83),
            muted: c(0x586e75),
            accent: c(0x268bd2),
            cursor: c(0x93a1a1),
            selection: with_alpha(c(0x073642), 0.9),
            ansi: ansi([
                0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
                0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
            ]),
        },
        Theme {
            name: SharedString::new_static("Solarized Light"),
            is_light: true,
            shell: c(0xeee8d5),
            pane: c(0xfdf6e3),
            raised: c(0xfffaf0),
            border: c(0xddd6c1),
            text: c(0x586e75),
            subtext: c(0x657b83),
            muted: c(0x93a1a1),
            accent: c(0x268bd2),
            cursor: c(0x586e75),
            selection: with_alpha(c(0xeee8d5), 0.9),
            ansi: ansi([
                0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
                0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
            ]),
        },
        Theme {
            name: SharedString::new_static("Dracula"),
            is_light: false,
            shell: c(0x21222c),
            pane: c(0x282a36),
            raised: c(0x44475a),
            border: c(0x343746),
            text: c(0xf8f8f2),
            subtext: c(0xbfc0cc),
            muted: c(0x6272a4),
            accent: c(0xbd93f9),
            cursor: c(0xf8f8f2),
            selection: with_alpha(c(0x44475a), 0.9),
            ansi: ansi([
                0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
                0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
            ]),
        },
        Theme {
            name: SharedString::new_static("One Dark"),
            is_light: false,
            shell: c(0x21252b),
            pane: c(0x282c34),
            raised: c(0x333842),
            border: c(0x2c313a),
            text: c(0xabb2bf),
            subtext: c(0x9da5b4),
            muted: c(0x5c6370),
            accent: c(0x61afef),
            cursor: c(0x528bff),
            selection: with_alpha(c(0x3e4451), 0.9),
            ansi: ansi([
                0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
            ]),
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

/// `a` moved `t` of the way toward `b`, in RGB.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let m = |x: f32, y: f32| x + (y - x) * t;
    Rgba { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: 1.0 }.into()
}

/// `fg` moved up to `t` of the way toward `bg`, but no further than keeps a
/// 3:1 contrast with `bg` (or `fg` itself, if even that is lower).
fn readable_mix(fg: Hsla, bg: Hsla, t: f32) -> Hsla {
    let mut t = t;
    while t > 0.0 {
        let c = mix(fg, bg, t);
        if contrast(c, bg) >= 3.0 {
            return c;
        }
        t -= 0.05;
    }
    fg
}

/// WCAG relative luminance.
fn luminance(color: Hsla) -> f32 {
    let c = Rgba::from(color);
    let lin = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG contrast ratio between two colours, from 1 to 21.
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Whether two theme names are the same, ignoring case ("ROSÉ PINE" is
/// "Rosé Pine").
pub fn same_name(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}

/// The built-in themes with those from files: a file whose name matches a
/// built-in (ignoring case) replaces it in place, and the rest follow, in the
/// order given.
pub fn merge(mut builtins: Vec<Theme>, files: Vec<Theme>) -> Vec<Theme> {
    for file in files {
        match builtins.iter().position(|t| same_name(&t.name, &file.name)) {
            Some(i) => builtins[i] = file,
            None => builtins.push(file),
        }
    }
    builtins
}

/// Chrome colours a theme file may give; the rest are worked out.
#[derive(Clone, Copy, Default)]
pub struct PartialChrome {
    pub shell: Option<Hsla>,
    pub raised: Option<Hsla>,
    pub border: Option<Hsla>,
    pub subtext: Option<Hsla>,
    pub muted: Option<Hsla>,
    pub accent: Option<Hsla>,
}

/// A whole theme from a terminal palette, working out any chrome colours
/// not given so tabs, palette and borders match.
pub fn derive(
    name: &str,
    background: Hsla,
    foreground: Hsla,
    palette: [Hsla; 16],
    cursor: Option<Hsla>,
    selection: Option<Hsla>,
    chrome: PartialChrome,
) -> Theme {
    let is_light = background.l > 0.5;
    // A little darker than the panes, on light themes too (as Rosé Pine Dawn).
    let shell = Hsla { l: (background.l - 0.03).clamp(0.0, 1.0), ..background };
    let accent = if contrast(palette[5], background) >= 3.0 { palette[5] } else { palette[4] };
    Theme {
        name: SharedString::from(name.to_string()),
        is_light,
        shell: chrome.shell.unwrap_or(shell),
        pane: background,
        raised: chrome.raised.unwrap_or(mix(background, foreground, 0.12)),
        border: chrome.border.unwrap_or(mix(background, foreground, 0.08)),
        text: foreground,
        subtext: chrome.subtext.unwrap_or_else(|| readable_mix(foreground, background, 0.35)),
        muted: chrome.muted.unwrap_or(mix(foreground, background, 0.55)),
        accent: chrome.accent.unwrap_or(accent),
        cursor: cursor.unwrap_or(foreground),
        selection: selection.map_or(with_alpha(foreground, 0.2), |s| with_alpha(s, 0.9)),
        ansi: palette,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_subtext_stays_readable() {
        // Solarized Light as a Ghostty file gives: low-contrast foreground.
        let (bg, fg) = (c(0xfdf6e3), c(0x657b83));
        let t = derive("Sol", bg, fg, themes()[0].ansi, None, None, PartialChrome::default());
        assert!(contrast(t.subtext, t.pane) >= 3.0, "{:.2}", contrast(t.subtext, t.pane));
        // Plenty of contrast: subtext still sits between text and background.
        let t = derive("Dark", c(0x101010), c(0xeeeeee), themes()[0].ansi, None, None, PartialChrome::default());
        assert!(t.subtext != t.text);
    }

    #[test]
    fn builtin_themes() {
        let all = themes();
        assert_eq!(all.len(), 24);
        let mut names: Vec<&str> = all.iter().map(|t| t.name.as_ref()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 24, "names are unique");
        assert_eq!(all.iter().filter(|t| t.is_light).count(), 8);
        for t in &all {
            assert!(contrast(t.text, t.pane) >= 4.5, "{}: text {:.2}", t.name, contrast(t.text, t.pane));
            assert!(contrast(t.subtext, t.pane) >= 3.0, "{}: subtext {:.2}", t.name, contrast(t.subtext, t.pane));
            assert_eq!(t.is_light, t.pane.l > 0.5, "{}: is_light", t.name);
        }
    }

    #[test]
    fn file_replaces_builtin_case_insensitively() {
        let mut file = themes()[0].clone();
        file.name = "rosé pine".into();
        file.pane = c(0x000000);
        let extra = Theme { name: "Aaa".into(), ..themes()[1].clone() };
        let merged = merge(themes(), vec![file, extra]);
        assert_eq!(merged.len(), themes().len() + 1);
        assert_eq!(merged.iter().filter(|t| t.name.eq_ignore_ascii_case("Rosé Pine")).count(), 1);
        assert_eq!(merged[0].pane, c(0x000000), "replaced in place");
        assert_eq!(merged.last().unwrap().name.as_ref(), "Aaa", "new ones go after the built-ins");
    }
}

