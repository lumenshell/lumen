//! The scrollbar shown on a pane's right edge while you scroll: where its
//! thumb and command marks go, and which scroll position a click or drag
//! means. Drawing and mouse handling are in `terminal_view`.

use std::time::{Duration, Instant};

use gpui::{Bounds, Pixels, point, px, size};

/// Width of the track and thumb; wider while the pointer is over it.
pub const WIDTH: f32 = 6.0;
pub const HOVER_WIDTH: f32 = 8.0;
/// The thumb never gets shorter than this, however long the history.
const MIN_THUMB: f32 = 24.0;
/// How long the scrollbar stays after the last scroll, then fades out.
pub const SHOW_FOR: Duration = Duration::from_millis(1000);
pub const FADE_FOR: Duration = Duration::from_millis(250);
/// How near a command's mark a click must be to jump to that command.
pub const TICK_REACH: f32 = 4.0;

/// The scroll position, as the terminal has it: `history` lines above the
/// screen, `rows` on it, and the view `offset` lines up from the bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub history: usize,
    pub rows: usize,
    pub offset: usize,
}

impl Position {
    fn total(&self) -> f32 {
        (self.history + self.rows).max(1) as f32
    }

    /// Index of the top line in view, counting from the oldest line.
    fn top(&self) -> usize {
        self.history.saturating_sub(self.offset)
    }
}

/// Where the track goes beside a pane's text: centred in the padding to its
/// right when there's room, else just inside the text's right edge.
pub fn track(text: Bounds<Pixels>, padding: f32, width: f32) -> Bounds<Pixels> {
    let x = if padding >= width + 4.0 {
        f32::from(text.origin.x + text.size.width) + (padding - width) / 2.0
    } else {
        f32::from(text.origin.x + text.size.width) - width - 2.0
    };
    Bounds::new(point(px(x), text.origin.y), size(px(width), text.size.height))
}

/// The thumb: as tall as the screen is a share of everything (at least
/// MIN_THUMB), placed by how far up you've scrolled.
pub fn thumb(track: Bounds<Pixels>, pos: Position) -> Bounds<Pixels> {
    let height = f32::from(track.size.height);
    let thumb = (height * pos.rows as f32 / pos.total()).clamp(MIN_THUMB.min(height), height);
    let travel = height - thumb;
    let y = if pos.history == 0 { 0.0 } else { travel * pos.top() as f32 / pos.history as f32 };
    Bounds::new(point(track.origin.x, track.origin.y + px(y)), size(track.size.width, px(thumb)))
}

/// Height in the track of the line `line` lines from the oldest, matching
/// where the thumb sits when that line is at the top of the view.
pub fn line_y(track: Bounds<Pixels>, pos: Position, line: usize) -> f32 {
    let at_top = Position { offset: pos.history.saturating_sub(line), ..pos };
    f32::from(thumb(track, at_top).origin.y)
}

/// The view offset that puts the thumb's top at `thumb_top` (a height in the
/// window), as when dragging it.
pub fn offset_for(track: Bounds<Pixels>, pos: Position, thumb_top: f32) -> usize {
    let height = f32::from(track.size.height);
    let thumb = f32::from(thumb(track, pos).size.height);
    let travel = (height - thumb).max(1.0);
    let share = ((thumb_top - f32::from(track.origin.y)) / travel).clamp(0.0, 1.0);
    let top = (share * pos.history as f32).round() as usize;
    pos.history - top.min(pos.history)
}

/// The view offset that puts line `line` (counting from the oldest) at the
/// top of the view, as when clicking a command's mark.
pub fn offset_to_show(pos: Position, line: usize) -> usize {
    pos.history.saturating_sub(line)
}

/// How opaque the scrollbar is: fully while you're scrolling, pointing at it
/// or dragging it, fading out once SHOW_FOR has passed since the last scroll.
pub fn opacity(shown_at: Option<Instant>, now: Instant, held: bool) -> f32 {
    if held {
        return 1.0;
    }
    let Some(at) = shown_at else { return 0.0 };
    let since = now.saturating_duration_since(at);
    if since <= SHOW_FOR {
        1.0
    } else {
        1.0 - ((since - SHOW_FOR).as_secs_f32() / FADE_FOR.as_secs_f32()).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track_of(height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(500.0), px(100.0)), size(px(WIDTH), px(height)))
    }

    #[test]
    fn the_track_sits_in_the_padding_when_it_fits() {
        let text = Bounds::new(point(px(10.0), px(20.0)), size(px(400.0), px(300.0)));
        assert_eq!(track(text, 12.0, WIDTH).origin.x, px(413.0));
        // Too little padding: just inside the text instead.
        assert_eq!(track(text, 4.0, WIDTH).origin.x, px(402.0));
        assert_eq!(track(text, 12.0, WIDTH).size.height, px(300.0));
    }

    #[test]
    fn the_thumb_shows_how_much_is_in_view_and_where() {
        let t = track_of(400.0);
        // 100 lines of history and 100 on screen: half the track.
        let bottom = thumb(t, Position { history: 100, rows: 100, offset: 0 });
        assert_eq!((bottom.origin.y, bottom.size.height), (px(300.0), px(200.0)));
        let top = thumb(t, Position { history: 100, rows: 100, offset: 100 });
        assert_eq!(top.origin.y, px(100.0));
        // Long history: never shorter than MIN_THUMB.
        let long = thumb(t, Position { history: 100_000, rows: 40, offset: 0 });
        assert_eq!(long.size.height, px(MIN_THUMB));
        assert_eq!(long.origin.y + long.size.height, px(500.0));
    }

    #[test]
    fn dragging_and_marks_agree_with_the_thumb() {
        let t = track_of(400.0);
        let pos = Position { history: 1000, rows: 50, offset: 0 };
        // Dragging the thumb to where a line's mark is shows that line at the top.
        for line in [0, 1, 250, 999, 1000] {
            let y = line_y(t, pos, line);
            assert_eq!(offset_for(t, pos, y), offset_to_show(pos, line), "line {line}");
        }
        // Past the ends clamps.
        assert_eq!(offset_for(t, pos, 0.0), 1000);
        assert_eq!(offset_for(t, pos, 9999.0), 0);
        // The top of the history is at the top of the track.
        assert_eq!(line_y(t, pos, 0), 100.0);
    }

    #[test]
    fn it_fades_after_scrolling_stops_unless_held() {
        let at = Instant::now();
        assert_eq!(opacity(None, at, false), 0.0);
        assert_eq!(opacity(Some(at), at + SHOW_FOR, false), 1.0);
        let half = opacity(Some(at), at + SHOW_FOR + FADE_FOR / 2, false);
        assert!((half - 0.5).abs() < 0.01, "{half}");
        assert_eq!(opacity(Some(at), at + SHOW_FOR + FADE_FOR * 2, false), 0.0);
        assert_eq!(opacity(None, at, true), 1.0);
    }
}
