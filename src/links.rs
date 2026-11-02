//! Finding links in the terminal grid: OSC 8 hyperlinks set by programs, and
//! plain-text URLs matched with a regex.

use std::ops::RangeInclusive;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Boundary, Direction, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::search::{RegexIter, RegexSearch};

/// URL pattern, as in Alacritty's default hint.
const URL_REGEX: &str = "(ipfs:|ipns:|magnet:|mailto:|gemini://|gopher://|https://|http://|news:|file:|git://|ssh:|ftp://)\
                         [^\u{0000}-\u{001F}\u{007F}-\u{009F}<>\"\\s{-}\\^⟨⟩`\\\\]+";

/// Schemes we'll hand to the system opener for URLs written out in the text,
/// where what you see is what opens.
const ALLOWED_SCHEMES: &[&str] = &[
    "http", "https", "mailto", "ftp", "file", "gemini", "gopher", "ssh", "git", "news", "magnet",
    "ipfs", "ipns",
];

/// OSC 8 links hide their target behind arbitrary text, so a program could
/// dress up `file:///…/evil.desktop` as "docs". Only web and mail targets open.
const EXPLICIT_SCHEMES: &[&str] = &["http", "https", "mailto"];

#[derive(Clone, PartialEq, Debug)]
pub struct Link {
    /// Cells covered by the link, in grid coordinates (history lines negative).
    pub range: RangeInclusive<Point>,
    pub uri: String,
    /// True for OSC 8 links, whose visible text may differ from the target.
    pub explicit: bool,
}

pub struct LinkFinder {
    regex: Option<RegexSearch>,
}

impl LinkFinder {
    pub fn new() -> Self {
        Self { regex: None }
    }

    /// The link under `point`, if any.
    pub fn link_at<T>(&mut self, term: &Term<T>, point: Point) -> Option<Link> {
        if !in_grid(term, point) {
            return None;
        }
        hyperlink_at(term, point).or_else(|| self.url_at(term, point))
    }

    fn url_at<T>(&mut self, term: &Term<T>, point: Point) -> Option<Link> {
        let regex = match &mut self.regex {
            Some(r) => r,
            slot => slot.insert(RegexSearch::new(URL_REGEX).ok()?),
        };
        // Search the whole logical line, following soft wraps.
        let start = term.line_search_left(point);
        let end = term.line_search_right(point);
        let found = RegexIter::new(start, end, Direction::Right, term, regex)
            .find(|m| m.contains(&point))?;
        let (start, end) = (*found.start(), *found.end());
        let text = term.bounds_to_string(start, end);
        let trimmed = trim_url(&text);
        let cut = text.chars().count() - trimmed.chars().count();
        let end = end.sub(term, Boundary::Grid, cut);
        if end < point {
            return None;
        }
        Some(Link { range: start..=end, uri: trimmed.to_string(), explicit: false })
    }
}

/// Is this link something we're willing to open?
pub fn openable(link: &Link) -> bool {
    let allowed = if link.explicit { EXPLICIT_SCHEMES } else { ALLOWED_SCHEMES };
    link.uri
        .split_once(':')
        .is_some_and(|(scheme, _)| allowed.contains(&scheme.to_ascii_lowercase().as_str()))
}

fn in_grid<T>(term: &Term<T>, point: Point) -> bool {
    point.line >= term.topmost_line()
        && point.line <= term.bottommost_line()
        && point.column.0 < term.columns()
}

/// Extend an OSC 8 hyperlink in both directions over cells sharing it.
fn hyperlink_at<T>(term: &Term<T>, point: Point) -> Option<Link> {
    let grid = term.grid();
    let link = grid[point].hyperlink()?;
    let same = |p: Point| grid[p].hyperlink().as_ref() == Some(&link);
    // Cap the walk so a pathological screen full of one link stays cheap.
    let limit = term.columns() * 64;

    let mut start = point;
    for _ in 0..limit {
        let prev = start.sub(term, Boundary::Grid, 1);
        if prev == start || !same(prev) {
            break;
        }
        start = prev;
    }
    let mut end = point;
    for _ in 0..limit {
        let next = end.add(term, Boundary::Grid, 1);
        if next == end || !same(next) {
            break;
        }
        end = next;
    }
    Some(Link { range: start..=end, uri: link.uri().to_string(), explicit: true })
}

/// Drop trailing punctuation that is almost never part of a URL in prose, and a
/// closing bracket with no opening partner ("(see https://x.y/z)").
fn trim_url(url: &str) -> &str {
    let mut s = url;
    loop {
        let Some(last) = s.chars().last() else { return s };
        let drop = match last {
            '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '"' => true,
            ')' => s.matches('(').count() < s.matches(')').count(),
            ']' => s.matches('[').count() < s.matches(']').count(),
            '}' => s.matches('{').count() < s.matches('}').count(),
            _ => false,
        };
        if !drop {
            return s;
        }
        s = &s[..s.len() - last.len_utf8()];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_prose_punctuation() {
        assert_eq!(trim_url("https://a.b/c."), "https://a.b/c");
        assert_eq!(trim_url("https://a.b/c),"), "https://a.b/c");
        assert_eq!(trim_url("https://en.wikipedia.org/wiki/Rust_(language)"), "https://en.wikipedia.org/wiki/Rust_(language)");
        assert_eq!(trim_url("https://a.b/?q=1"), "https://a.b/?q=1");
    }

    fn link(uri: &str, explicit: bool) -> Link {
        let p = Point::new(alacritty_terminal::index::Line(0), alacritty_terminal::index::Column(0));
        Link { range: p..=p, uri: uri.into(), explicit }
    }

    #[test]
    fn scheme_allowlist() {
        assert!(openable(&link("https://example.com", false)));
        assert!(openable(&link("HTTP://example.com", false)));
        assert!(openable(&link("file:///etc/hosts", false)));
        assert!(!openable(&link("vscode://open?file=x", false)));
        assert!(!openable(&link("no-scheme", false)));
    }

    #[test]
    fn osc8_links_are_web_only() {
        assert!(openable(&link("https://example.com", true)));
        assert!(openable(&link("mailto:a@b.c", true)));
        assert!(!openable(&link("file:///home/me/evil.desktop", true)));
        assert!(!openable(&link("ssh://host", true)));
    }
}
