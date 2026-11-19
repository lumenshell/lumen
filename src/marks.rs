//! Finding prompts and command output in the grid, from the hidden links
//! shell integration puts on prompt and input text.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::Term;

use crate::shell_integration::{INPUT, PROMPT};

#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    /// First line of the prompt.
    pub line: Line,
    /// Last line of the prompt and the command typed at it.
    pub input_end: Line,
    /// Link id, unique per prompt.
    pub id: String,
    /// Exit status of the command run at the *previous* prompt.
    pub exit_before: Option<i32>,
}

fn exit_of(uri: &str) -> Option<i32> {
    uri.strip_prefix(PROMPT)?.strip_prefix("?exit=")?.parse().ok()
}

/// Every prompt in the screen and scrollback, oldest first. This reads all
/// of the scrollback; for the newest few, see [`newest_first`].
pub fn prompts<T>(term: &Term<T>) -> Vec<Prompt> {
    let mut found: Vec<Prompt> = Vec::new();
    newest_first(term, None, |p| {
        found.push(p);
        true
    });
    found.reverse();
    found
}

/// Give `each` every prompt, newest first, until it returns false. The grid
/// is read from the bottom up and no further than that, so asking for the
/// newest prompt or two costs the same however long the scrollback is.
///
/// A prompt is a run of cells linked with one prompt id; typed input after
/// it (and before the next prompt) extends its `input_end`.
///
/// Where a prompt starts is only certain once the one above it is reached,
/// which may be a long way up. With `gap`, a prompt is taken as whole once
/// that many lines above it have none of it: a shell draws its prompt in
/// one go, so a screenful is plenty.
pub fn newest_first<T>(term: &Term<T>, gap: Option<usize>, mut each: impl FnMut(Prompt) -> bool) {
    let grid = term.grid();
    let mut current: Option<Prompt> = None;
    // The last line of input met since the newer prompt began: it belongs
    // to the prompt above it, which hasn't been reached yet.
    let mut input: Option<Line> = None;
    for line in (term.topmost_line().0..=term.bottommost_line().0).rev().map(Line) {
        if let Some(gap) = gap
            && current.as_ref().is_some_and(|p| (p.line.0 - line.0) as usize > gap)
            && let Some(done) = current.take()
            && !each(done)
        {
            return;
        }
        let row = &grid[line];
        for col in (0..term.columns()).rev() {
            let Some(link) = row[Column(col)].hyperlink() else { continue };
            let uri = link.uri();
            if uri.starts_with(PROMPT) {
                match &mut current {
                    Some(p) if p.id == link.id() => {
                        p.line = line;
                        p.exit_before = exit_of(uri);
                        input = None;
                    }
                    _ => {
                        if let Some(done) = current.take()
                            && !each(done)
                        {
                            return;
                        }
                        current = Some(Prompt {
                            line,
                            input_end: input.take().map_or(line, |i| i.max(line)),
                            id: link.id().to_string(),
                            exit_before: exit_of(uri),
                        });
                    }
                }
            } else if uri == INPUT && input.is_none() {
                input = Some(line);
            }
        }
    }
    if let Some(p) = current {
        each(p);
    }
}

/// The newest prompt, if there is one.
pub fn newest_prompt<T>(term: &Term<T>) -> Option<Prompt> {
    let mut newest = None;
    newest_first(term, Some(term.screen_lines()), |p| {
        newest = Some(p);
        false
    });
    newest
}

/// [`recent_commands`] of every prompt in the terminal, reading back only
/// as far as the commands asked for.
pub fn newest_commands<T>(term: &Term<T>, n: usize, skip: usize) -> Vec<Option<String>> {
    let mut found: Vec<Option<String>> = Vec::new();
    let mut skipped = 0;
    if n > 0 {
        newest_first(term, Some(term.screen_lines()), |p| {
            match command_at(term, &p) {
                Some(_) if skipped < skip => skipped += 1,
                Some(command) => found.push(Some(command)),
                None => {}
            }
            found.len() < n
        });
    }
    found.resize(n, None);
    found.reverse();
    found
}

/// Exit status of each prompt's command, by prompt id (None while a command
/// is still running or when the shell didn't report one).
pub fn exits(prompts: &[Prompt]) -> std::collections::HashMap<String, Option<i32>> {
    prompts
        .windows(2)
        .map(|w| (w[0].id.clone(), w[1].exit_before))
        .chain(prompts.last().map(|p| (p.id.clone(), None)))
        .collect()
}

/// The command typed at the newest prompt that has one, as shown on screen
/// (wrapped lines joined). Tests check [`newest_commands`] against this.
#[cfg(test)]
pub fn last_command<T>(term: &Term<T>, prompts: &[Prompt]) -> Option<String> {
    recent_commands(term, prompts, 1, 0).pop().flatten()
}

/// The last `n` commands typed, oldest first, after skipping the newest
/// `skip` (say, one still running). Padded at the front with `None` when
/// fewer are on screen. Tests check [`newest_commands`] against this.
#[cfg(test)]
pub fn recent_commands<T>(term: &Term<T>, prompts: &[Prompt], n: usize, skip: usize) -> Vec<Option<String>> {
    let mut found: Vec<Option<String>> =
        prompts.iter().rev().filter_map(|p| command_at(term, p)).skip(skip).take(n).map(Some).collect();
    found.resize(n, None);
    found.reverse();
    found
}

/// The command typed at prompt `p`, if any.
fn command_at<T>(term: &Term<T>, p: &Prompt) -> Option<String> {
    let grid = term.grid();
    {
        let mut text = String::new();
        for line in (p.line.0..=p.input_end.0).map(Line) {
            for col in 0..term.columns() {
                let cell = &grid[line][Column(col)];
                if cell.hyperlink().is_some_and(|l| l.uri() == INPUT)
                    && !cell.flags.contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER)
                {
                    text.push(cell.c);
                }
            }
        }
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    }
}

/// Lines of output from the most recent finished command, if it printed any.
pub fn last_output<T>(term: &Term<T>, prompts: &[Prompt]) -> Option<(Point, Point)> {
    // The newest prompt is the one waiting for input; its predecessor is the
    // last command that finished.
    let [.., done, next] = prompts else { return None };
    let first = done.input_end + 1;
    let last = next.line - 1;
    (first <= last).then(|| (Point::new(first, Column(0)), Point::new(last, term.last_column())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_integration::{Rewriter, Shared};
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;
    use std::sync::{Arc, Mutex};

    struct Size;
    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            10
        }
        fn screen_lines(&self) -> usize {
            10
        }
        fn columns(&self) -> usize {
            40
        }
    }

    /// A terminal fed shell output (with OSC 133 marks) through the rewriter.
    fn term(output: &str) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &Size, VoidListener);
        let mut rewriter = Rewriter::new(Arc::new(Mutex::new(Shared::default())), None, None, false);
        let mut bytes = Vec::new();
        let output = output.replace('A', "\x1b]133;A\x07").replace('B', "\x1b]133;B\x07");
        let output =
            output.replace('C', "\x1b]133;C\x07").replace("D0", "\x1b]133;D;0\x07").replace("D1", "\x1b]133;D;1\x07");
        rewriter.feed(output.replace('\n', "\r\n").as_bytes(), &mut bytes);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, &bytes);
        term
    }

    #[test]
    fn a_command_edited_after_wrapping_is_read_correctly() {
        // bash reprints the prompt when a wrapped line is edited (here with
        // Ctrl+U); the reprint mustn't be read as part of the command.
        let long = "echo a very long command that wraps past forty";
        let t = term(&format!(
            // (ESC M moves up and ESC [8G to column 8: the helper turns capital A-D into marks.)
            "A$ Bls\nCx\nD0A~/src$ B{long}\x1bM\r~/src$ B\x1b[K\r\n\x1b[K\x1bM\x1b[8Gls -l"
        ));
        let p = prompts(&t);
        assert_eq!(p.len(), 2, "{p:?}");
        assert_eq!(recent_commands(&t, &p, 2, 0), vec![Some("ls".into()), Some("ls -l".into())]);
    }

    #[test]
    fn finds_prompts_output_and_exit_statuses() {
        // Placeholders: A prompt, B input, C output, D0/D1 exit status.
        let t = term("A$ Bls\nCa.txt\nb.txt\nD0A$ Bfalse\nCD1A$ B");
        let p = prompts(&t);
        assert_eq!(p.iter().map(|p| p.line.0).collect::<Vec<_>>(), vec![0, 3, 4]);
        let exits = exits(&p);
        assert_eq!(exits[&p[0].id], Some(0));
        assert_eq!(exits[&p[1].id], Some(1));
        assert_eq!(exits[&p[2].id], None);

        assert_eq!(last_command(&t, &p).as_deref(), Some("false"));
        assert_eq!(last_command(&t, &p[..1]).as_deref(), Some("ls"));
        assert_eq!(recent_commands(&t, &p, 2, 0), vec![Some("ls".into()), Some("false".into())]);
        assert_eq!(recent_commands(&t, &p, 1, 1), vec![Some("ls".into())]);
        assert_eq!(recent_commands(&t, &p, 3, 0)[0], None);
        // Read from the bottom up, the answers are the same.
        for (n, skip) in [(0, 0), (1, 0), (2, 0), (1, 1), (3, 0), (2, 5)] {
            assert_eq!(newest_commands(&t, n, skip), recent_commands(&t, &p, n, skip), "{n} {skip}");
        }
        assert_eq!(newest_prompt(&t).as_ref(), p.last());
        // `false` printed nothing; the last output is empty.
        assert_eq!(last_output(&t, &p), None);
        // Before that, `ls` printed two lines.
        let (start, end) = last_output(&t, &p[..2]).unwrap();
        assert_eq!(t.bounds_to_string(start, end).trim_end(), "a.txt\nb.txt");
    }
}
