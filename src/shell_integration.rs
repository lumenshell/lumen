//! Shell integration: knowing where prompts, commands and their output are.
//!
//! Shells loaded with Lumen's snippets (`shell-integration/`) mark each
//! prompt and command with OSC 133 and report their directory with OSC 7.
//! The terminal library ignores both, so the PTY output is rewritten on its
//! way in: the prompt and the typed command are wrapped in hidden OSC 8 links
//! (`lumen:prompt`, `lumen:input`). Those links are stored on the cells
//! themselves, so the marks move with the text as it scrolls or reflows and
//! disappear with it when history is trimmed.

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite, Pty};
use polling::{Event, PollMode, Poller};

/// Link scheme for the hidden marks. Never opened or shown as a link.
pub const SCHEME: &str = "lumen:";
pub const PROMPT: &str = "lumen:prompt";
pub const INPUT: &str = "lumen:input";

const BASH: &str = include_str!("../shell-integration/lumen.bash");
const ZSH: &str = include_str!("../shell-integration/lumen.zsh");
const ZSHENV: &str = include_str!("../shell-integration/zshenv");
const FISH: &str = include_str!("../shell-integration/lumen.fish");

/// Where the scripts are written for shells to load.
fn install_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let base = if cfg!(target_os = "macos") {
            home?.join("Library/Application Support")
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| home.map(|h| h.join(".local/share")))?
        };
        let dir = base.join("lumen").join("shell-integration");
        let files = [
            ("bash/lumen.bash", BASH),
            ("zsh/lumen.zsh", ZSH),
            ("zsh/.zshenv", ZSHENV),
            ("fish/fish/vendor_conf.d/lumen.fish", FISH),
        ];
        for (name, text) in files {
            let path = dir.join(name);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text) {
                std::fs::create_dir_all(path.parent()?).ok()?;
                std::fs::write(&path, text).ok()?;
            }
        }
        Some(dir)
    })
    .as_deref()
}

/// How to start `shell` (program and arguments) so it loads the integration:
/// the arguments to use and environment variables to add. Unknown shells,
/// and bash started as a login shell, run unchanged.
pub fn inject(shell: &[String]) -> (Vec<String>, HashMap<String, String>) {
    let mut env = HashMap::new();
    let unchanged = (shell.to_vec(), HashMap::new());
    let Some(dir) = install_dir() else { return unchanged };
    let Some((program, args)) = shell.split_first() else { return unchanged };
    let name = Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or("");
    match name {
        "bash" => {
            // --rcfile replaces ~/.bashrc and only applies to non-login
            // interactive shells; the script sources the usual files itself.
            if args.iter().any(|a| a == "-l" || a == "--login" || a == "-c" || a.starts_with("--rcfile")) {
                return unchanged;
            }
            env.insert("LUMEN_BASH_RCFILE".into(), "1".into());
            let mut cmd = vec![program.clone(), "--rcfile".into(), dir.join("bash/lumen.bash").display().to_string()];
            cmd.extend(args.iter().cloned());
            (cmd, env)
        }
        "zsh" => {
            if let Some(orig) = std::env::var_os("ZDOTDIR") {
                env.insert("LUMEN_ZDOTDIR".into(), orig.to_string_lossy().into_owned());
            }
            env.insert("ZDOTDIR".into(), dir.join("zsh").display().to_string());
            env.insert("LUMEN_SHELL_INTEGRATION".into(), dir.join("zsh").display().to_string());
            (shell.to_vec(), env)
        }
        "fish" => {
            let data = dir.join("fish").display().to_string();
            let rest = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
            env.insert("XDG_DATA_DIRS".into(), format!("{data}:{rest}"));
            env.insert("LUMEN_FISH_DATA".into(), data);
            (shell.to_vec(), env)
        }
        _ => unchanged,
    }
}

/// The shell to run when none is configured: `$SHELL`, then `/bin/sh`.
pub fn default_shell() -> Vec<String> {
    vec![std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into())]
}

/// State the rewriter shares with the UI.
#[derive(Default)]
pub struct Shared {
    /// Directory last reported by the shell with OSC 7.
    pub cwd: Option<PathBuf>,
    /// The shell has sent at least one mark, so integration is working.
    pub active: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Normal,
    /// Saw ESC; waiting to see if it starts an OSC.
    Esc,
    /// Inside an OSC we want (133 or 7), buffering until the terminator.
    Capture,
    /// Inside some other OSC: passed through untouched.
    Pass,
    /// ESC seen inside a passed-through OSC (maybe the ST terminator).
    PassEsc,
    /// ESC seen inside a captured OSC (maybe the ST terminator).
    CaptureEsc,
}

/// Longest OSC we buffer; anything longer can't be one of ours.
const MAX_CAPTURE: usize = 4096;

/// Rewrites PTY output: OSC 133 marks become hidden OSC 8 links and OSC 7 is
/// recorded. Handles sequences split across reads.
pub struct Rewriter {
    state: State,
    /// Bytes of an OSC being inspected or captured, after `ESC ]`.
    body: Vec<u8>,
    /// A command ran since the last prompt (OSC 133;C seen).
    ran: bool,
    /// Exit status of that command (OSC 133;D).
    exit: Option<i32>,
    /// The shell has sent a prompt mark (OSC 133;A).
    prompted: bool,
    /// A command is running: between OSC 133;C and the next D or A. Its
    /// output, not the shell, is writing to the terminal.
    running: bool,
    shared: Arc<Mutex<Shared>>,
    host: String,
}

fn local_hostname() -> &'static str {
    static HOST: OnceLock<String> = OnceLock::new();
    HOST.get_or_init(|| {
        std::fs::read_to_string("/proc/sys/kernel/hostname")
            .ok()
            .or_else(|| {
                std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
            })
            .map(|h| h.trim().to_string())
            .unwrap_or_default()
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Rewriter {
    pub fn new(shared: Arc<Mutex<Shared>>) -> Self {
        Self::with_host(shared, local_hostname().to_string())
    }

    fn with_host(shared: Arc<Mutex<Shared>>, host: String) -> Self {
        Self { state: State::Normal, body: Vec::new(), ran: false, exit: None, prompted: false, running: false, shared, host }
    }

    /// Could `body` (bytes after `ESC ]`) still become an OSC we capture?
    fn wanted_prefix(body: &[u8]) -> bool {
        b"133;".starts_with(body) || body.starts_with(b"133;") || b"7;".starts_with(body) || body.starts_with(b"7;")
    }

    pub fn feed(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &b in input {
            match self.state {
                State::Normal => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    } else {
                        out.push(b);
                    }
                }
                State::Esc => {
                    if b == b']' {
                        self.body.clear();
                        self.state = State::Capture;
                    } else {
                        out.push(0x1b);
                        if b == 0x1b {
                            continue; // stay in Esc for the new ESC
                        }
                        out.push(b);
                        self.state = State::Normal;
                    }
                }
                State::Capture => match b {
                    0x07 => self.finish(out),
                    0x1b => self.state = State::CaptureEsc,
                    _ => {
                        self.body.push(b);
                        if !Self::wanted_prefix(&self.body) || self.body.len() > MAX_CAPTURE {
                            // Not ours: emit what was held and pass the rest through.
                            out.extend_from_slice(b"\x1b]");
                            out.extend_from_slice(&self.body);
                            self.body.clear();
                            self.state = State::Pass;
                        }
                    }
                },
                State::CaptureEsc => {
                    if b == b'\\' {
                        self.finish(out);
                    } else {
                        // ESC cancelled the OSC: drop it, start a new sequence.
                        self.body.clear();
                        self.state = State::Esc;
                        self.feed(&[b], out);
                    }
                }
                State::Pass => {
                    out.push(b);
                    match b {
                        0x07 => self.state = State::Normal,
                        0x1b => self.state = State::PassEsc,
                        _ => {}
                    }
                }
                State::PassEsc => {
                    if b == b'\\' {
                        out.push(b);
                        self.state = State::Normal;
                    } else {
                        // The ESC (already emitted) cancelled the OSC and starts
                        // a new sequence; replay this byte as following it.
                        out.pop();
                        self.state = State::Esc;
                        self.feed(&[b], out);
                    }
                }
            }
        }
    }

    fn link(out: &mut Vec<u8>, uri: &str) {
        out.extend_from_slice(b"\x1b]8;;");
        out.extend_from_slice(uri.as_bytes());
        out.extend_from_slice(b"\x1b\\");
    }

    /// Handle a complete captured OSC.
    fn finish(&mut self, out: &mut Vec<u8>) {
        self.state = State::Normal;
        let body = String::from_utf8_lossy(&self.body).into_owned();
        self.body.clear();
        if let Some(mark) = body.strip_prefix("133;") {
            self.shared.lock().unwrap().active = true;
            let mut parts = mark.split(';');
            match parts.next() {
                Some("A") => {
                    // The link on the prompt carries the previous command's exit status.
                    let uri = match (self.ran, self.exit) {
                        (true, Some(code)) => format!("{PROMPT}?exit={code}"),
                        _ => PROMPT.to_string(),
                    };
                    Self::link(out, &uri);
                    self.ran = false;
                    self.exit = None;
                    self.prompted = true;
                    self.running = false;
                }
                Some("B") => Self::link(out, INPUT),
                Some("C") => {
                    Self::link(out, "");
                    self.ran = true;
                    self.running = true;
                }
                Some("D") => {
                    self.running = false;
                    if self.ran {
                        self.exit = parts.next().and_then(|c| c.trim().parse().ok());
                    }
                }
                _ => {}
            }
        } else if let Some(url) = body.strip_prefix("7;") {
            // Only believe the integrated shell, at its prompt. Anything else
            // (a program's output, `cat` of a file, a remote host) could
            // otherwise choose where the next pane opens.
            if !self.prompted || self.running {
                return;
            }
            if let Some(rest) = url.strip_prefix("file://") {
                let (host, path) = rest.find('/').map(|i| rest.split_at(i)).unwrap_or((rest, ""));
                let local = host.is_empty() || host == "localhost" || host.eq_ignore_ascii_case(&self.host);
                if local && !path.is_empty() {
                    self.shared.lock().unwrap().cwd = Some(PathBuf::from(percent_decode(path)));
                }
            }
        }
    }
}

/// Reads the PTY through a [`Rewriter`].
pub struct MarkedReader {
    file: std::fs::File,
    rewriter: Rewriter,
    out: Vec<u8>,
    pos: usize,
    scratch: Vec<u8>,
}

impl Read for MarkedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.pos < self.out.len() {
                let n = buf.len().min(self.out.len() - self.pos);
                buf[..n].copy_from_slice(&self.out[self.pos..self.pos + n]);
                self.pos += n;
                if self.pos == self.out.len() {
                    self.out.clear();
                    self.pos = 0;
                }
                return Ok(n);
            }
            let got = self.file.read(&mut self.scratch)?;
            if got == 0 {
                return Ok(0);
            }
            let (scratch, out) = (&self.scratch[..got], &mut self.out);
            self.rewriter.feed(scratch, out);
            // Everything read may be an unfinished sequence; read more.
        }
    }
}

/// The PTY with its output read through a [`MarkedReader`].
pub struct MarkedPty {
    pty: Pty,
    reader: MarkedReader,
}

impl MarkedPty {
    pub fn new(pty: Pty, shared: Arc<Mutex<Shared>>) -> io::Result<Self> {
        // A second handle on the same open file: polling still watches the
        // original, and reads through either see the same data.
        let file = pty.file().try_clone()?;
        let reader = MarkedReader {
            file,
            rewriter: Rewriter::new(shared),
            out: Vec::new(),
            pos: 0,
            scratch: vec![0; 0x1_0000],
        };
        Ok(Self { pty, reader })
    }
}

impl EventedReadWrite for MarkedPty {
    type Reader = MarkedReader;
    type Writer = std::fs::File;

    unsafe fn register(&mut self, poll: &Arc<Poller>, interest: Event, mode: PollMode) -> io::Result<()> {
        unsafe { self.pty.register(poll, interest, mode) }
    }

    fn reregister(&mut self, poll: &Arc<Poller>, interest: Event, mode: PollMode) -> io::Result<()> {
        self.pty.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut MarkedReader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut std::fs::File {
        self.pty.writer()
    }
}

impl EventedPty for MarkedPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

impl OnResize for MarkedPty {
    fn on_resize(&mut self, size: WindowSize) {
        self.pty.on_resize(size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(chunks: &[&[u8]]) -> (Vec<u8>, Arc<Mutex<Shared>>) {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut r = Rewriter::with_host(shared.clone(), "myhost".into());
        let mut out = Vec::new();
        for c in chunks {
            r.feed(c, &mut out);
        }
        (out, shared)
    }

    fn text(out: &[u8]) -> String {
        String::from_utf8_lossy(out).replace('\x1b', "⎋").replace('\x07', "🔔")
    }

    #[test]
    fn marks_become_links_with_the_previous_exit_status() {
        let (out, shared) = rewrite(&[b"\x1b]133;A\x07$ \x1b]133;B\x07false\r\n\x1b]133;C\x07\x1b]133;D;1\x07\x1b]133;A\x07$ "]);
        assert_eq!(
            text(&out),
            "⎋]8;;lumen:prompt⎋\\$ ⎋]8;;lumen:input⎋\\false\r\n⎋]8;;⎋\\⎋]8;;lumen:prompt?exit=1⎋\\$ "
        );
        assert!(shared.lock().unwrap().active);
    }

    #[test]
    fn exit_status_only_counts_when_a_command_ran() {
        // An empty Enter: D without C (bash repeats the old status).
        let (out, _) = rewrite(&[b"\x1b]133;D;1\x07\x1b]133;A\x07"]);
        assert_eq!(text(&out), "⎋]8;;lumen:prompt⎋\\");
    }

    #[test]
    fn sequences_split_across_reads() {
        let (out, _) = rewrite(&[b"ab\x1b", b"]13", b"3;C\x1b", b"\\cd"]);
        assert_eq!(text(&out), "ab⎋]8;;⎋\\cd");
    }

    #[test]
    fn other_sequences_pass_through_untouched() {
        let input: &[u8] = b"\x1b[31mred\x1b]0;title\x07\x1b]8;;https://x.y\x1b\\link\x1b]52;c;aGk=\x07\x1b\x1b[0m";
        let (out, _) = rewrite(&[input]);
        assert_eq!(out, input);
        // Byte at a time too.
        let bytes: Vec<&[u8]> = input.chunks(1).collect();
        assert_eq!(rewrite(&bytes).0, input);
    }

    #[test]
    fn cwd_from_osc7_for_this_host_only() {
        let (out, shared) = rewrite(&[b"\x1b]133;A\x07\x1b]7;file://myhost/home/me/My%20Docs\x07"]);
        assert_eq!(text(&out), "⎋]8;;lumen:prompt⎋\\");
        assert_eq!(shared.lock().unwrap().cwd, Some(PathBuf::from("/home/me/My Docs")));
        let (_, shared) = rewrite(&[b"\x1b]133;A\x07\x1b]7;file://elsewhere/srv\x1b\\"]);
        assert_eq!(shared.lock().unwrap().cwd, None);
    }

    #[test]
    fn osc7_ignored_unless_the_shell_is_at_its_prompt() {
        let cwd = |chunks: &[&[u8]]| rewrite(chunks).1.lock().unwrap().cwd.clone();
        // No integration: nothing vouches for it.
        assert_eq!(cwd(&[b"\x1b]7;file://localhost/tmp\x07"]), None);
        // Printed by a running command.
        assert_eq!(cwd(&[b"\x1b]133;A\x07\x1b]133;C\x07\x1b]7;file://localhost/tmp\x07"]), None);
        // Back at the prompt after the command finished.
        assert_eq!(
            cwd(&[b"\x1b]133;A\x07\x1b]133;C\x07\x1b]133;D;0\x07\x1b]7;file://localhost/tmp\x07"]),
            Some(PathBuf::from("/tmp"))
        );
    }

    #[test]
    fn injection_per_shell() {
        let (cmd, env) = inject(&["/bin/bash".into()]);
        assert_eq!(cmd[1], "--rcfile");
        assert!(env.contains_key("LUMEN_BASH_RCFILE"));
        let (cmd, env) = inject(&["/bin/bash".into(), "--login".into()]);
        assert_eq!((cmd.len(), env.len()), (2, 0));
        let (_, env) = inject(&["/usr/bin/zsh".into()]);
        assert!(env["ZDOTDIR"].ends_with("shell-integration/zsh"));
        let (_, env) = inject(&["fish".into()]);
        assert!(env["XDG_DATA_DIRS"].contains("shell-integration/fish"));
        let (cmd, env) = inject(&["/usr/bin/nu".into()]);
        assert_eq!((cmd, env.len()), (vec!["/usr/bin/nu".to_string()], 0));
    }
}
