//! Find in the visible screen and scrollback.
//!
//! Matches are stored by absolute line (counted from the oldest line of
//! history) so they stay attached to their text while new output scrolls the
//! grid; the view converts them back to grid points when drawing.

use std::ops::RangeInclusive;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::search::{RegexIter, RegexSearch};

/// Enough to be useful in a huge log without making each scan slow.
const MAX_MATCHES: usize = 10_000;

/// (absolute line, column)
type Abs = (i64, usize);

#[derive(Default)]
pub struct Search {
    pub query: String,
    /// Treat the query as a regular expression rather than plain text.
    pub regex: bool,
    /// Match case exactly. When off, case is ignored unless the query has a
    /// capital letter.
    pub case_sensitive: bool,
    compiled: Option<RegexSearch>,
    /// The query isn't a valid regular expression.
    pub invalid: bool,
    matches: Vec<(Abs, Abs)>,
    pub current: Option<usize>,
    /// More matches exist than were collected.
    pub truncated: bool,
}

fn history<T>(term: &Term<T>) -> i64 {
    term.grid().history_size() as i64
}

fn to_abs<T>(term: &Term<T>, p: Point) -> Abs {
    (p.line.0 as i64 + history(term), p.column.0)
}

fn to_point<T>(term: &Term<T>, (line, col): Abs) -> Point {
    Point::new(Line((line - history(term)) as i32), Column(col))
}

/// Escape regex metacharacters so a plain-text query matches literally.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Search {
    pub fn pattern(&self) -> String {
        let body = if self.regex { self.query.clone() } else { escape(&self.query) };
        if self.case_sensitive { format!("(?-i){body}") } else { body }
    }

    /// Recompile after the query or options change, rescan, and select the
    /// match nearest the bottom of the view.
    pub fn update<T: EventListener>(&mut self, term: &mut Term<T>) {
        self.compiled = None;
        self.invalid = false;
        if !self.query.is_empty() {
            match RegexSearch::new(&self.pattern()) {
                Ok(r) => self.compiled = Some(r),
                Err(_) => self.invalid = true,
            }
        }
        self.scan(term);
        self.current = None;
        self.select_nearest(term);
    }

    /// Find every match again (after new output), keeping the current one
    /// selected if it still exists.
    pub fn rescan<T>(&mut self, term: &Term<T>) {
        let current = self.current.map(|i| self.matches[i]);
        self.scan(term);
        self.current = current.and_then(|m| self.matches.iter().position(|x| *x == m));
    }

    fn scan<T>(&mut self, term: &Term<T>) {
        self.matches.clear();
        self.truncated = false;
        let Some(regex) = self.compiled.as_mut() else { return };
        let start = Point::new(term.topmost_line(), Column(0));
        let end = Point::new(term.bottommost_line(), term.last_column());
        for m in RegexIter::new(start, end, Direction::Right, term, regex) {
            if self.matches.len() == MAX_MATCHES {
                self.truncated = true;
                break;
            }
            self.matches.push((to_abs(term, *m.start()), to_abs(term, *m.end())));
        }
    }

    /// Select the last match at or above the bottom of the view.
    fn select_nearest<T: EventListener>(&mut self, term: &mut Term<T>) {
        if self.matches.is_empty() {
            return;
        }
        let offset = term.grid().display_offset() as i32;
        let bottom = to_abs(term, Point::new(Line(term.screen_lines() as i32 - 1 - offset), Column(0)));
        let i = self.matches.iter().rposition(|m| m.0.0 <= bottom.0).unwrap_or(0);
        self.select(term, i);
    }

    /// Move to the next older (`older`) or newer match, wrapping around.
    pub fn step<T: EventListener>(&mut self, term: &mut Term<T>, older: bool) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        let i = match self.current {
            None => {
                self.select_nearest(term);
                return;
            }
            Some(i) if older => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
        };
        self.select(term, i);
    }

    fn select<T: EventListener>(&mut self, term: &mut Term<T>, i: usize) {
        self.current = Some(i);
        let (start, end) = self.matches[i];
        // Bring the whole match into view (end first, so the start wins).
        term.scroll_to_point(to_point(term, end));
        term.scroll_to_point(to_point(term, start));
    }

    pub fn count(&self) -> usize {
        self.matches.len()
    }

    /// Matches overlapping the visible screen, as grid points, with whether
    /// each is the current one.
    pub fn visible<T>(&self, term: &Term<T>) -> Vec<(RangeInclusive<Point>, bool)> {
        let offset = term.grid().display_offset() as i32;
        let top = to_abs(term, Point::new(Line(-offset), Column(0))).0;
        let bottom = top + term.screen_lines() as i64 - 1;
        // Matches are sorted, so skip straight to the first that can be visible.
        let first = self.matches.partition_point(|m| m.1.0 < top);
        self.matches[first..]
            .iter()
            .enumerate()
            .take_while(|(_, m)| m.0.0 <= bottom)
            .map(|(k, &(s, e))| (to_point(term, s)..=to_point(term, e), self.current == Some(first + k)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    struct Size(usize, usize);
    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            self.1
        }
        fn screen_lines(&self) -> usize {
            self.1
        }
        fn columns(&self) -> usize {
            self.0
        }
    }

    /// A 40x5 terminal with 100 lines of history, fed `text`.
    fn term(text: &str) -> Term<VoidListener> {
        let config = Config { scrolling_history: 100, ..Default::default() };
        let mut term = Term::new(config, &Size(40, 5), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, text.replace('\n', "\r\n").as_bytes());
        term
    }

    fn search(query: &str) -> Search {
        Search { query: query.into(), ..Default::default() }
    }

    #[test]
    fn finds_matches_in_scrollback_and_selects_the_newest() {
        let mut t = term(&(0..20).map(|i| format!("line {i} needle\n")).collect::<String>());
        let mut s = search("needle");
        s.update(&mut t);
        assert_eq!(s.count(), 20);
        assert_eq!(s.current, Some(19));
        // Older goes up, and wraps around from the oldest to the newest.
        s.step(&mut t, true);
        assert_eq!(s.current, Some(18));
        s.current = Some(0);
        s.step(&mut t, true);
        assert_eq!(s.current, Some(19));
    }

    #[test]
    fn selecting_an_old_match_scrolls_it_into_view() {
        let mut t = term(&(0..20).map(|i| format!("row {i}\n")).collect::<String>());
        let mut s = search("row 2\n".trim());
        s.update(&mut t);
        // "row 2" matches only row 2 (rows 20+ don't exist), far up the history.
        assert_eq!(s.count(), 1);
        assert!(t.grid().display_offset() > 0);
        let visible = s.visible(&t);
        assert_eq!(visible.len(), 1);
        assert!(visible[0].1);
    }

    #[test]
    fn plain_text_is_literal_and_case_is_smart() {
        let mut t = term("a.b axb A.B\n");
        let mut s = search("a.b");
        s.update(&mut t);
        assert_eq!(s.count(), 2); // "a.b" and "A.B", not "axb"

        let mut s = search("A.B");
        s.update(&mut t);
        assert_eq!(s.count(), 1); // a capital letter makes it case-sensitive

        let mut s = Search { case_sensitive: true, ..search("a.b") };
        s.update(&mut t);
        assert_eq!(s.count(), 1);

        let mut s = Search { regex: true, ..search("a.b") };
        s.update(&mut t);
        assert_eq!(s.count(), 3);
    }

    #[test]
    fn invalid_regex_is_reported_not_fatal() {
        let mut t = term("hello\n");
        let mut s = Search { regex: true, ..search("(unclosed") };
        s.update(&mut t);
        assert!(s.invalid);
        assert_eq!(s.count(), 0);
    }

    #[test]
    fn matches_follow_their_text_as_output_scrolls() {
        // Fill the 5-row screen, so further lines push "target" into history.
        let mut t = term("target\nA\nB\nC\n");
        let mut s = search("target");
        s.update(&mut t);
        let line = |s: &Search, t: &Term<VoidListener>| to_point(t, s.matches[0].0).line;
        assert_eq!(line(&s, &t), Line(0));

        let mut parser: Processor = Processor::new();
        parser.advance(&mut t, b"D\r\nE\r\n");
        // Without rescanning, the stored match already points two lines up.
        assert_eq!(line(&s, &t), Line(-2));
        s.rescan(&t);
        assert_eq!((s.count(), s.current), (1, Some(0)));
        assert_eq!(line(&s, &t), Line(-2));
    }
}
