//! Keyboard select mode: a vi-style cursor for moving around the screen and
//! scrollback, selecting and copying without the mouse.
//!
//! The cursor, its motions (words, paragraphs, brackets…) and keeping the
//! selection in step live in `alacritty_terminal`; this module maps keys to
//! them and adds counts, `gg`/`G`, page moves, selections, and jumps between
//! commands. Anything that needs the UI (copying, the find bar) comes back as
//! an [`Outcome`] for the view to carry out.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vi_mode::ViMotion;
use gpui::Keystroke;

use crate::marks;

/// Keys typed so far that need another to mean something.
#[derive(Default, Debug)]
pub struct ViState {
    /// A count typed before a motion, as in `5j`.
    pub count: Option<usize>,
    /// `g` was typed; `gg` goes to the top.
    pub pending_g: bool,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The key did something (or nothing) inside the mode; redraw.
    Handled,
    /// Copy this text and leave the mode.
    Copy(String),
    /// Leave the mode.
    Exit,
    /// Open the find bar; `backward` for `?`.
    Search { backward: bool },
    /// Go to the next (`reverse` false) match of the last search.
    SearchAgain { reverse: bool },
}

pub fn active<T>(term: &Term<T>) -> bool {
    term.mode().contains(TermMode::VI)
}

/// Turn the mode on or off. Leaving clears the selection and returns to the
/// bottom, where typing goes.
pub fn toggle<T: EventListener>(term: &mut Term<T>, state: &mut ViState) {
    *state = ViState::default();
    if active(term) {
        term.toggle_vi_mode();
        term.selection = None;
        term.scroll_display(Scroll::Bottom);
    } else {
        term.toggle_vi_mode();
    }
}

/// Handle one keystroke in the mode.
pub fn key<T: EventListener>(term: &mut Term<T>, state: &mut ViState, ks: &Keystroke) -> Outcome {
    let m = ks.modifiers;
    // Printable keys come as the character typed, so `$` and `G` work on any
    // layout; ctrl and alt combinations by key name.
    let ch = if m.control || m.alt || m.platform {
        None
    } else {
        ks.key_char.as_deref().and_then(|s| {
            let mut chars = s.chars();
            let c = chars.next()?;
            // Shift+v is `V` even if the platform reported the plain letter.
            let c = if m.shift { c.to_ascii_uppercase() } else { c };
            chars.next().is_none().then_some(c)
        })
    };
    let typed_count = state.count.take();
    let count = typed_count.unwrap_or(1);
    let pending_g = std::mem::take(&mut state.pending_g);
    let motion = |term: &mut Term<T>, motion: ViMotion| {
        for _ in 0..count {
            term.vi_motion(motion);
        }
        Outcome::Handled
    };

    if m.control && !m.alt && !m.platform {
        return match ks.key.as_str() {
            "u" => page(term, count, 0.5),
            "d" => page(term, count, -0.5),
            "b" => page(term, count, 1.0),
            "f" => page(term, count, -1.0),
            "v" => select(term, SelectionType::Block),
            "c" => Outcome::Exit,
            _ => Outcome::Handled,
        };
    }

    match (ch, ks.key.as_str()) {
        (Some(d @ '1'..='9'), _) => digit(state, typed_count, d, pending_g),
        (Some('0'), _) if typed_count.is_some() => digit(state, typed_count, '0', pending_g),
        (Some('h'), _) | (_, "left") => motion(term, ViMotion::Left),
        (Some('j'), _) | (_, "down") => motion(term, ViMotion::Down),
        (Some('k'), _) | (_, "up") => motion(term, ViMotion::Up),
        (Some('l'), _) | (_, "right") => motion(term, ViMotion::Right),
        (Some('w'), _) => motion(term, ViMotion::SemanticRight),
        (Some('b'), _) => motion(term, ViMotion::SemanticLeft),
        (Some('e'), _) => motion(term, ViMotion::SemanticRightEnd),
        (Some('W'), _) => motion(term, ViMotion::WordRight),
        (Some('B'), _) => motion(term, ViMotion::WordLeft),
        (Some('E'), _) => motion(term, ViMotion::WordRightEnd),
        (Some('0'), _) | (_, "home") => motion(term, ViMotion::First),
        (Some('^'), _) => motion(term, ViMotion::FirstOccupied),
        (Some('$'), _) | (_, "end") => motion(term, ViMotion::Last),
        (Some('H'), _) => motion(term, ViMotion::High),
        (Some('M'), _) => motion(term, ViMotion::Middle),
        (Some('L'), _) => motion(term, ViMotion::Low),
        (Some('{'), _) => motion(term, ViMotion::ParagraphUp),
        (Some('}'), _) => motion(term, ViMotion::ParagraphDown),
        (Some('%'), _) => motion(term, ViMotion::Bracket),
        (Some('g'), _) if pending_g => {
            term.vi_goto_point(Point::new(term.topmost_line(), Column(0)));
            Outcome::Handled
        }
        (Some('g'), _) => {
            state.pending_g = true;
            Outcome::Handled
        }
        (Some('G'), _) => {
            let line = last_text_line(term);
            term.vi_goto_point(Point::new(line, Column(0)));
            Outcome::Handled
        }
        (_, "pageup") => page(term, count, 1.0),
        (_, "pagedown") => page(term, count, -1.0),
        (Some('['), _) => prompt(term, count, true),
        (Some(']'), _) => prompt(term, count, false),
        (Some('v'), _) => select(term, SelectionType::Simple),
        (Some('V'), _) => select(term, SelectionType::Lines),
        (Some('y'), _) | (_, "enter") => match term.selection_to_string().filter(|t| !t.is_empty()) {
            Some(text) => Outcome::Copy(text),
            None => Outcome::Handled,
        },
        (Some('Y'), _) => {
            // The whole line under the cursor.
            let point = term.vi_mode_cursor.point;
            let mut line = Selection::new(SelectionType::Lines, point, Side::Left);
            line.include_all();
            term.selection = Some(line);
            match term.selection_to_string() {
                Some(text) => Outcome::Copy(text),
                None => Outcome::Handled,
            }
        }
        (Some('/'), _) => Outcome::Search { backward: false },
        (Some('?'), _) => Outcome::Search { backward: true },
        (Some('n'), _) => Outcome::SearchAgain { reverse: false },
        (Some('N'), _) => Outcome::SearchAgain { reverse: true },
        (_, "escape") if term.selection.is_some() => {
            term.selection = None;
            Outcome::Handled
        }
        (_, "escape") | (Some('q' | 'i'), _) => Outcome::Exit,
        // Anything else is swallowed: typing must not reach the shell here.
        _ => Outcome::Handled,
    }
}

fn digit(state: &mut ViState, so_far: Option<usize>, d: char, pending_g: bool) -> Outcome {
    state.count = Some((so_far.unwrap_or(0) * 10 + d.to_digit(10).unwrap() as usize).min(10_000));
    state.pending_g = pending_g;
    Outcome::Handled
}

/// Start, change or end a selection at the cursor. The same kind again ends it.
fn select<T: EventListener>(term: &mut Term<T>, ty: SelectionType) -> Outcome {
    match term.selection.as_mut() {
        Some(sel) if sel.ty == ty => term.selection = None,
        Some(sel) => sel.ty = ty,
        None => {
            let mut sel = Selection::new(ty, term.vi_mode_cursor.point, Side::Left);
            sel.include_all();
            term.selection = Some(sel);
        }
    }
    Outcome::Handled
}

/// Move by `pages` screens (positive is up), scrolling the view along.
fn page<T: EventListener>(term: &mut Term<T>, count: usize, pages: f32) -> Outcome {
    let lines = (term.screen_lines() as f32 * pages).round() as i32 * count as i32;
    let target = term.vi_mode_cursor.scroll(term, lines).point;
    term.scroll_display(Scroll::Delta(lines));
    term.vi_goto_point(target);
    Outcome::Handled
}

/// Jump to the `count`th prompt above (`up`) or below the cursor. Needs shell
/// integration; without it nothing moves.
fn prompt<T: EventListener>(term: &mut Term<T>, count: usize, up: bool) -> Outcome {
    let here = term.vi_mode_cursor.point.line;
    let prompts = marks::prompts(term);
    let target = if up {
        prompts.iter().rev().filter(|p| p.line < here).nth(count - 1)
    } else {
        prompts.iter().filter(|p| p.line > here).nth(count - 1)
    };
    if let Some(p) = target {
        term.vi_goto_point(Point::new(p.line, Column(0)));
    }
    Outcome::Handled
}

/// The last line with text on it, so `G` doesn't land in the blank space
/// below the prompt.
fn last_text_line<T>(term: &Term<T>) -> Line {
    let grid = term.grid();
    (term.topmost_line().0..=term.bottommost_line().0)
        .rev()
        .map(Line)
        .find(|&l| grid[l].into_iter().any(|c| c.c != ' ' && c.c != '\0'))
        .unwrap_or(Line(0))
}

/// The hint shown while the mode is on.
pub fn status<T>(term: &Term<T>, state: &ViState) -> String {
    let what = match term.selection.as_ref().map(|s| s.ty) {
        Some(SelectionType::Lines) => "Selecting lines · y copy · Esc cancel",
        Some(SelectionType::Block) => "Selecting a block · y copy · Esc cancel",
        Some(_) => "Selecting · y copy · Esc cancel",
        None => "Select mode · v select · V lines · / find · Esc exit",
    };
    match state.count {
        Some(n) => format!("{n} · {what}"),
        None => what.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    struct Size;
    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            8
        }
        fn screen_lines(&self) -> usize {
            8
        }
        fn columns(&self) -> usize {
            30
        }
    }

    fn term(text: &str) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &Size, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, text.replace('\n', "\r\n").as_bytes());
        term
    }

    /// Press each key of `keys` (space-separated keystrokes, GPUI syntax).
    fn press(term: &mut Term<VoidListener>, state: &mut ViState, keys: &str) -> Outcome {
        let mut last = Outcome::Handled;
        for k in keys.split(' ') {
            let mut ks = Keystroke::parse(k).unwrap();
            if !ks.modifiers.control && ks.key.chars().count() == 1 {
                let c = ks.key.chars().next().unwrap();
                let c = if ks.modifiers.shift { c.to_ascii_uppercase() } else { c };
                ks.key_char = Some(c.to_string());
            }
            last = key(term, state, &ks);
        }
        last
    }

    #[test]
    fn shift_with_a_plain_letter_is_the_capital() {
        let mut t = term("one\ntwo\n");
        let mut s = ViState::default();
        toggle(&mut t, &mut s);
        let ks = Keystroke {
            modifiers: gpui::Modifiers { shift: true, ..Default::default() },
            key: "v".into(),
            key_char: Some("v".into()),
        };
        key(&mut t, &mut s, &ks);
        assert_eq!(t.selection.as_ref().map(|s| s.ty), Some(SelectionType::Lines));
    }

    fn char_key(c: char) -> Keystroke {
        Keystroke { key: c.to_string(), key_char: Some(c.to_string()), ..Default::default() }
    }

    #[test]
    fn select_words_and_copy() {
        let mut t = term("$ echo hello world\nhello world\n$ ");
        let mut s = ViState::default();
        toggle(&mut t, &mut s);
        assert!(active(&t));
        // To the top, down a line, select two words.
        let out = press(&mut t, &mut s, "g g j v e w e y");
        assert_eq!(out, Outcome::Copy("hello world".into()));
    }

    #[test]
    fn counts_lines_and_escape() {
        let mut t = term("one\ntwo\nthree\nfour\n");
        let mut s = ViState::default();
        toggle(&mut t, &mut s);
        press(&mut t, &mut s, "g g");
        press(&mut t, &mut s, "2 j");
        assert_eq!(t.vi_mode_cursor.point.line, Line(2));
        // `1 0 k` is ten lines up (clamped at the top), not column 0 then up.
        press(&mut t, &mut s, "1 0 k");
        assert_eq!(t.vi_mode_cursor.point.line, Line(0));
        press(&mut t, &mut s, "2 j");
        assert_eq!(press(&mut t, &mut s, "shift-v j y"), Outcome::Copy("three\nfour\n".into()));
        // Esc drops a selection first, then leaves. (The view leaves the
        // mode after a copy; here the copied selection is still there.)
        t.selection = None;
        press(&mut t, &mut s, "shift-v");
        assert_eq!(press(&mut t, &mut s, "escape"), Outcome::Handled);
        assert!(t.selection.is_none());
        assert_eq!(press(&mut t, &mut s, "escape"), Outcome::Exit);
    }

    #[test]
    fn symbols_go_by_the_character_typed() {
        let mut t = term("  indented line\n");
        let mut s = ViState::default();
        toggle(&mut t, &mut s);
        press(&mut t, &mut s, "g g");
        key(&mut t, &mut s, &char_key('$'));
        assert_eq!(t.vi_mode_cursor.point.column, Column(14));
        key(&mut t, &mut s, &char_key('^'));
        assert_eq!(t.vi_mode_cursor.point.column, Column(2));
        assert_eq!(key(&mut t, &mut s, &char_key('/')), Outcome::Search { backward: false });
        assert_eq!(key(&mut t, &mut s, &char_key('N')), Outcome::SearchAgain { reverse: true });
    }

    #[test]
    fn typing_is_swallowed_and_leaving_cleans_up() {
        let mut t = term("$ ls\n");
        let mut s = ViState::default();
        toggle(&mut t, &mut s);
        assert_eq!(press(&mut t, &mut s, "x z"), Outcome::Handled);
        press(&mut t, &mut s, "v l");
        toggle(&mut t, &mut s);
        assert!(!active(&t));
        assert!(t.selection.is_none());
    }

    #[test]
    fn jumps_between_prompts() {
        use crate::shell_integration::{Rewriter, Shared};
        use std::sync::{Arc, Mutex};
        let mut t = Term::new(Config::default(), &Size, VoidListener);
        let mut bytes = Vec::new();
        let mut r = Rewriter::new(Arc::new(Mutex::new(Shared::default())), None, None, false);
        let marks = |s: &str| s.replace('A', "\x1b]133;A\x07").replace('B', "\x1b]133;B\x07").replace('C', "\x1b]133;C\x07");
        r.feed(marks("A$ Bls\r\nCa\r\nb\r\nA$ Becho\r\nCx\r\nA$ B").as_bytes(), &mut bytes);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut t, &bytes);
        let mut s = ViState::default();
        toggle(&mut t, &mut s);
        press(&mut t, &mut s, "[");
        assert_eq!(t.vi_mode_cursor.point.line, Line(3));
        press(&mut t, &mut s, "[");
        assert_eq!(t.vi_mode_cursor.point.line, Line(0));
        press(&mut t, &mut s, "2 ]");
        assert_eq!(t.vi_mode_cursor.point.line, Line(5));
    }
}
