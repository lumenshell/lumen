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

/// Every prompt in the screen and scrollback, oldest first.
pub fn prompts<T>(term: &Term<T>) -> Vec<Prompt> {
    let grid = term.grid();
    let mut found: Vec<Prompt> = Vec::new();
    for line in (term.topmost_line().0..=term.bottommost_line().0).map(Line) {
        let row = &grid[line];
        for col in 0..term.columns() {
            let Some(link) = row[Column(col)].hyperlink() else { continue };
            let uri = link.uri();
            if uri.starts_with(PROMPT) {
                match found.last_mut() {
                    Some(p) if p.id == link.id() => p.input_end = line,
                    _ => found.push(Prompt {
                        line,
                        input_end: line,
                        id: link.id().to_string(),
                        exit_before: exit_of(uri),
                    }),
                }
            } else if uri == INPUT
                && let Some(p) = found.last_mut()
            {
                p.input_end = p.input_end.max(line);
            }
        }
    }
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
        let mut rewriter = Rewriter::new(Arc::new(Mutex::new(Shared::default())), None);
        let mut bytes = Vec::new();
        let output = output.replace('A', "\x1b]133;A\x07").replace('B', "\x1b]133;B\x07");
        let output = output.replace('C', "\x1b]133;C\x07").replace("D0", "\x1b]133;D;0\x07").replace("D1", "\x1b]133;D;1\x07");
        rewriter.feed(output.replace('\n', "\r\n").as_bytes(), &mut bytes);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, &bytes);
        term
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

        // `false` printed nothing; the last output is empty.
        assert_eq!(last_output(&t, &p), None);
        // Before that, `ls` printed two lines.
        let (start, end) = last_output(&t, &p[..2]).unwrap();
        assert_eq!(t.bounds_to_string(start, end).trim_end(), "a.txt\nb.txt");
    }
}
