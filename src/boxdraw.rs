//! Box-drawing, block-element and powerline characters drawn as shapes.
//!
//! Font glyphs for these rarely fill the cell exactly (our line height is
//! taller than the font, and cells are a fractional number of pixels wide),
//! which leaves visible seams between characters that are meant to join.
//! Drawing them ourselves, snapped to whole pixels at shared cell edges,
//! makes borders, bars and powerline prompts continuous.

use gpui::{Bounds, Pixels, Point, point, px, size};

pub enum Shape {
    /// A filled rectangle at the given opacity (shades use less than 1).
    Rect(Bounds<Pixels>, f32),
    /// A rounded corner: draw `quad` with only the borders on `corner`'s two
    /// sides, clipped to `clip`.
    Arc { quad: Bounds<Pixels>, clip: Bounds<Pixels>, corner: Corner, radius: Pixels, width: Pixels },
    Polygon(Vec<Point<Pixels>>),
    Polyline(Vec<Point<Pixels>>, Pixels),
}

#[derive(Clone, Copy)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

/// Can `c` be drawn here rather than from the font?
pub fn handles(c: char) -> bool {
    matches!(c as u32, 0x2500..=0x2570 | 0x2574..=0x259F | 0xE0B0..=0xE0B3)
}

/// Shapes for `c` in the cell spanning `x0..x1` by `y0..y1`. Callers pass
/// edges already rounded to whole pixels, shared with neighbouring cells.
pub fn shapes(c: char, x0: f32, x1: f32, y0: f32, y1: f32) -> Vec<Shape> {
    let cell = Cell { x0, x1, y0, y1 };
    let code = c as u32;
    match code {
        0x2500..=0x257F => box_lines(code, &cell),
        0x2580..=0x259F => blocks(code, &cell),
        0xE0B0..=0xE0B3 => powerline(code, &cell),
        _ => Vec::new(),
    }
}

struct Cell {
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
}

impl Cell {
    fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    fn h(&self) -> f32 {
        self.y1 - self.y0
    }
    fn cx(&self) -> f32 {
        self.x0 + (self.w() / 2.0).floor()
    }
    fn cy(&self) -> f32 {
        self.y0 + (self.h() / 2.0).floor()
    }
    /// Thickness of a light line.
    fn light(&self) -> f32 {
        (self.w() * 0.12).round().max(1.0)
    }
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Shape {
    rect_alpha(x0, y0, x1, y1, 1.0)
}

fn rect_alpha(x0: f32, y0: f32, x1: f32, y1: f32, alpha: f32) -> Shape {
    let (x0, x1) = (x0.min(x1), x0.max(x1));
    let (y0, y1) = (y0.min(y1), y0.max(y1));
    Shape::Rect(Bounds::new(point(px(x0), px(y0)), size(px(x1 - x0), px(y1 - y0))), alpha)
}

// Line weights for each arm, in the order up, right, down, left.
const NONE: u8 = 0;
const HEAVY: u8 = 2;
const DOUBLE: u8 = 3;

/// Arms of U+2500..=U+257F as "urdl" digits (0 none, 1 light, 2 heavy,
/// 3 double). `-`/`|` are dashed (horizontal/vertical, followed by the
/// weight and dash count), `a` is a rounded corner, `x` a diagonal.
const BOX: [&str; 128] = [
    // 2500
    "0101", "0202", "1010", "2020", "-13", "-23", "|13", "|23",
    "-14", "-24", "|14", "|24", "0110", "0210", "0120", "0220",
    // 2510
    "0011", "0012", "0021", "0022", "1100", "1200", "2100", "2200",
    "1001", "1002", "2001", "2002", "1110", "1210", "2110", "1120",
    // 2520
    "2120", "2210", "1220", "2220", "1011", "1012", "2011", "1021",
    "2021", "2012", "1022", "2022", "0111", "0112", "0211", "0212",
    // 2530
    "0121", "0122", "0221", "0222", "1101", "1102", "1201", "1202",
    "2101", "2102", "2201", "2202", "1111", "1112", "1211", "1212",
    // 2540
    "2111", "1121", "2121", "2112", "2211", "1122", "1221", "2212",
    "1222", "2122", "2221", "2222", "-12", "-22", "|12", "|22",
    // 2550
    "0303", "3030", "0310", "0130", "0330", "0013", "0031", "0033",
    "1300", "3100", "3300", "1003", "3001", "3003", "1310", "3130",
    // 2560
    "3330", "1013", "3031", "3033", "0313", "0131", "0333", "1303",
    "3101", "3303", "1313", "3131", "3333", "a", "a", "a",
    // 2570
    "a", "x", "x", "x", "0001", "1000", "0100", "0010",
    "0002", "2000", "0200", "0020", "0201", "1020", "0102", "2010",
];

fn box_lines(code: u32, cell: &Cell) -> Vec<Shape> {
    let spec = BOX[(code - 0x2500) as usize].as_bytes();
    let t = cell.light();
    match spec[0] {
        b'-' | b'|' => {
            let weight = spec[1] - b'0';
            let count = (spec[2] - b'0') as usize;
            dashes(cell, spec[0] == b'-', if weight == HEAVY { t * 2.0 } else { t }, count)
        }
        b'a' => arc(code, cell, t),
        b'x' => Vec::new(),
        _ => {
            let arms = [spec[0] - b'0', spec[1] - b'0', spec[2] - b'0', spec[3] - b'0'];
            joined(cell, arms, t)
        }
    }
}

/// Lines from the centre to the edges for each arm, joined at the centre.
///
/// Positions are whole-pixel offsets from the cell centre. A line of
/// thickness `th` centred on offset `o` covers `band(o, th)`; every arm that
/// meets another stops exactly at the edge of that arm's band, so joins have
/// no gaps and no stubs.
fn joined(cell: &Cell, arms: [u8; 4], t: f32) -> Vec<Shape> {
    let thick = |w: u8| if w == HEAVY { t * 2.0 } else { t };
    // Centre of each line of a double arm, either side of the centre line.
    let d = t;
    let band = |o: f32, th: f32| {
        let lo = o + (-th / 2.0).floor();
        (lo, lo + th)
    };
    // Extent of an arm across its own axis.
    let extent = |w: u8| match w {
        DOUBLE => (band(-d, t).0, band(d, t).1),
        w => band(0.0, thick(w)),
    };
    let (cx, cy) = (cell.cx(), cell.cy());
    let mut out = Vec::new();

    for dir in 0..4 {
        let w = arms[dir];
        if w == NONE {
            continue;
        }
        // Arms either side (a = towards the negative across direction), the
        // opposite arm, and whether this arm points towards negative offsets.
        let (side_a, side_b, opposite, negative) = match dir {
            0 => (arms[3], arms[1], arms[2], true),  // up: left, right, down
            1 => (arms[0], arms[2], arms[3], false), // right: up, down, left
            2 => (arms[3], arms[1], arms[0], false), // down: left, right, up
            _ => (arms[0], arms[2], arms[1], true),  // left: up, down, right
        };
        let reach = match dir {
            0 => cy - cell.y0,
            1 => cell.x1 - cx,
            2 => cell.y1 - cy,
            _ => cx - cell.x0,
        };
        // The end of a line nearest the centre, given a band it should meet.
        let near_edge = |(lo, hi): (f32, f32)| if negative { hi } else { lo };

        // (band across, near end along) for each line of this arm.
        let lines: Vec<((f32, f32), f32)> = if w == DOUBLE {
            // Where a line turns depends on the arm on its own side (`near`)
            // and on the other side (`far`).
            let near = |near: u8, far: u8| {
                let inner = if negative { -d } else { d };
                if near == DOUBLE {
                    near_edge(band(inner, t))
                } else if near != NONE {
                    near_edge(band(0.0, thick(near)))
                } else if opposite != NONE {
                    0.0
                } else if far == DOUBLE {
                    near_edge(band(-inner, t))
                } else if far != NONE {
                    near_edge(band(0.0, thick(far)))
                } else {
                    0.0
                }
            };
            vec![
                (band(-d, t), near(side_a, side_b)),
                (band(d, t), near(side_b, side_a)),
            ]
        } else {
            // Reach across the centre far enough to cover perpendicular arms.
            let own = band(0.0, thick(w));
            let (lo, hi) = [side_a, side_b]
                .iter()
                .filter(|&&p| p != NONE)
                .map(|&p| extent(p))
                .fold(own, |(l, h), (pl, ph)| (l.min(pl), h.max(ph)));
            vec![(own, near_edge((lo, hi)))]
        };

        for ((a0, a1), near) in lines {
            out.push(match dir {
                0 => rect(cx + a0, cy - reach, cx + a1, cy + near),
                1 => rect(cx + near, cy + a0, cx + reach, cy + a1),
                2 => rect(cx + a0, cy + near, cx + a1, cy + reach),
                _ => rect(cx - reach, cy + a0, cx + near, cy + a1),
            });
        }
    }
    out
}

fn dashes(cell: &Cell, horizontal: bool, th: f32, count: usize) -> Vec<Shape> {
    let (start, len) = if horizontal { (cell.x0, cell.w()) } else { (cell.y0, cell.h()) };
    let step = len / count as f32;
    let a0 = (if horizontal { cell.cy() } else { cell.cx() } - th / 2.0).round();
    (0..count)
        .map(|i| {
            // Gaps sit at both ends of each step so dashes stay evenly spaced
            // across neighbouring cells.
            let s = (start + step * i as f32 + step * 0.2).round();
            let e = (start + step * (i as f32 + 1.0) - step * 0.2).round().max(s + 1.0);
            if horizontal { rect(s, a0, e, a0 + th) } else { rect(a0, s, a0 + th, e) }
        })
        .collect()
}

fn arc(code: u32, cell: &Cell, t: f32) -> Vec<Shape> {
    // The arc runs from the middle of one edge to the middle of the adjacent
    // one: a rounded border corner centred on the cell's centre lines.
    let (cx, cy) = (cell.cx(), cell.cy());
    let a = (t / 2.0).round();
    let r = (cell.w().min(cell.h()) / 2.0).floor();
    let clip = Bounds::new(point(px(cell.x0), px(cell.y0)), size(px(cell.w()), px(cell.h())));
    let big = cell.w() + cell.h();
    let (corner, x, y) = match code {
        0x256D => (Corner::TopLeft, cx - a, cy - a),                // ╭
        0x256E => (Corner::TopRight, cx - big + t - a, cy - a),     // ╮
        0x256F => (Corner::BottomRight, cx - big + t - a, cy - big + t - a), // ╯
        _ => (Corner::BottomLeft, cx - a, cy - big + t - a),        // ╰
    };
    let quad = Bounds::new(point(px(x), px(y)), size(px(big), px(big)));
    vec![Shape::Arc { quad, clip, corner, radius: px(r), width: px(t) }]
}

fn blocks(code: u32, cell: &Cell) -> Vec<Shape> {
    let Cell { x0, x1, y0, y1 } = *cell;
    let fx = |f: f32| (x0 + cell.w() * f).round();
    let fy = |f: f32| (y0 + cell.h() * f).round();
    let (mx, my) = (fx(0.5), fy(0.5));
    match code {
        0x2580 => vec![rect(x0, y0, x1, my)],
        0x2581..=0x2588 => {
            let eighths = (code - 0x2580) as f32;
            vec![rect(x0, fy(1.0 - eighths / 8.0), x1, y1)]
        }
        0x2589..=0x258F => {
            let eighths = (0x2590 - code) as f32;
            vec![rect(x0, y0, fx(eighths / 8.0), y1)]
        }
        0x2590 => vec![rect(mx, y0, x1, y1)],
        0x2591..=0x2593 => {
            let alpha = (code - 0x2590) as f32 * 0.25;
            vec![rect_alpha(x0, y0, x1, y1, alpha)]
        }
        0x2594 => vec![rect(x0, y0, x1, fy(1.0 / 8.0))],
        0x2595 => vec![rect(fx(7.0 / 8.0), y0, x1, y1)],
        _ => {
            // Quadrants: bits for upper-left, upper-right, lower-left, lower-right.
            let bits: u8 = match code {
                0x2596 => 0b0010,
                0x2597 => 0b0001,
                0x2598 => 0b1000,
                0x2599 => 0b1011,
                0x259A => 0b1001,
                0x259B => 0b1110,
                0x259C => 0b1101,
                0x259D => 0b0100,
                0x259E => 0b0110,
                _ => 0b0111, // 0x259F
            };
            let quads = [
                (0b1000, x0, y0, mx, my),
                (0b0100, mx, y0, x1, my),
                (0b0010, x0, my, mx, y1),
                (0b0001, mx, my, x1, y1),
            ];
            quads
                .iter()
                .filter(|q| bits & q.0 != 0)
                .map(|&(_, a, b, c, d)| rect(a, b, c, d))
                .collect()
        }
    }
}

fn powerline(code: u32, cell: &Cell) -> Vec<Shape> {
    let Cell { x0, x1, y0, y1 } = *cell;
    let my = y0 + cell.h() / 2.0;
    let p = |x: f32, y: f32| point(px(x), px(y));
    match code {
        0xE0B0 => vec![Shape::Polygon(vec![p(x0, y0), p(x1, my), p(x0, y1)])],
        0xE0B2 => vec![Shape::Polygon(vec![p(x1, y0), p(x0, my), p(x1, y1)])],
        0xE0B1 => vec![Shape::Polyline(vec![p(x0, y0), p(x1, my), p(x0, y1)], px(cell.light()))],
        _ => vec![Shape::Polyline(vec![p(x1, y0), p(x0, my), p(x1, y1)], px(cell.light()))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects(c: char) -> Vec<(f32, f32, f32, f32)> {
        shapes(c, 0.0, 8.0, 0.0, 18.0)
            .into_iter()
            .filter_map(|s| match s {
                Shape::Rect(b, _) => Some((
                    f32::from(b.origin.x),
                    f32::from(b.origin.y),
                    f32::from(b.origin.x + b.size.width),
                    f32::from(b.origin.y + b.size.height),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn straight_lines_reach_both_edges() {
        // A horizontal line must touch x=0 and x=8 so neighbours join.
        let r = rects('─');
        assert!(r.iter().any(|r| r.0 == 0.0) && r.iter().any(|r| r.2 == 8.0));
        let r = rects('│');
        assert!(r.iter().any(|r| r.1 == 0.0) && r.iter().any(|r| r.3 == 18.0));
    }

    #[test]
    fn full_block_fills_the_cell() {
        assert_eq!(rects('█'), vec![(0.0, 0.0, 8.0, 18.0)]);
        assert_eq!(rects('▀'), vec![(0.0, 0.0, 8.0, 9.0)]);
    }

    #[test]
    fn double_lines_are_two_strokes() {
        assert_eq!(rects('═').len(), 4); // two per arm
        assert_eq!(rects('╔').len(), 4);
    }

    /// Every filled pixel of `c` in an 8x18 cell, as a set.
    fn pixels(c: char) -> std::collections::BTreeSet<(i32, i32)> {
        rects(c)
            .into_iter()
            .flat_map(|(x0, y0, x1, y1)| {
                (y0 as i32..y1 as i32).flat_map(move |y| (x0 as i32..x1 as i32).map(move |x| (x, y)))
            })
            .collect()
    }

    #[test]
    fn corners_have_no_stubs() {
        // A corner is exactly the union of its two half-lines from the centre,
        // so nothing pokes out past the join.
        let up_left = pixels('┘');
        let (cx, cy) = (4, 9);
        assert!(up_left.iter().all(|&(x, y)| x <= cx && y <= cy), "{up_left:?}");
        let down_right = pixels('┌');
        assert!(down_right.iter().all(|&(x, y)| x >= cx - 1 && y >= cy - 1), "{down_right:?}");
        // And a corner plus its mirror covers a cross exactly.
        let cross: std::collections::BTreeSet<_> = pixels('┘').union(&pixels('┌')).copied().collect();
        assert_eq!(cross, pixels('┼'));
    }

    #[test]
    fn table_covers_the_range() {
        for c in 0x2500..=0x257F {
            let spec = BOX[c - 0x2500];
            assert!(!spec.is_empty(), "{c:x}");
        }
    }
}
