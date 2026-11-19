//! Owns a shell process and its terminal state (alacritty_terminal), and bridges
//! terminal events onto an async channel the UI can await.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use crate::shell_integration::{self, MarkedPty, Shared};

#[derive(Clone)]
pub struct Listener(UnboundedSender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.unbounded_send(event);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl GridSize {
    /// Until the pane is laid out.
    pub const DEFAULT: GridSize = GridSize { cols: 80, rows: 24, cell_width: 8, cell_height: 16 };

    fn window_size(self) -> WindowSize {
        WindowSize {
            num_cols: self.cols,
            num_lines: self.rows,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

pub type SharedTerm = Arc<FairMutex<Term<Listener>>>;

/// Who draws a plugin pane: the connection that opened it, which gets the
/// pane's input, size and closing (as `pane` messages) and alone may write
/// to it.
#[derive(Clone)]
pub struct Owner {
    /// The connection, to recognise its requests and notice it closing.
    pub conn: u64,
    pub out: std::sync::mpsc::Sender<String>,
    /// The plugin's name ("script" for a socket client), the pane's first title.
    pub name: String,
}

impl Owner {
    /// Tell the owner something about one of its panes.
    pub fn send(&self, pane: u64, event: &str, mut params: serde_json::Value) {
        params["event"] = event.into();
        params["pane"] = pane.into();
        let msg = serde_json::json!({"jsonrpc": "2.0", "method": "pane", "params": params});
        let _ = self.out.send(msg.to_string());
    }
}

/// What's on the other side of the terminal: a shell (or other program) in
/// a pseudoterminal, a plugin drawing the pane itself, or nothing at all
/// (a pane that only says why no shell could be started).
enum Backend {
    Process {
        notifier: Notifier,
        child_pid: u32,
    },
    Message {
        parser: Box<alacritty_terminal::vte::ansi::Processor>,
        events: UnboundedSender<Event>,
    },
    Plugin {
        owner: Owner,
        pane: u64,
        parser: Box<alacritty_terminal::vte::ansi::Processor>,
        events: UnboundedSender<Event>,
    },
}

pub struct Pty {
    pub term: SharedTerm,
    backend: Backend,
    pub size: GridSize,
    /// What the shell has told us through shell integration.
    pub shell_state: Arc<Mutex<Shared>>,
}

impl Pty {
    pub fn spawn(
        cwd: Option<PathBuf>,
        shell: Option<&[String]>,
        integrate: bool,
        pane: u64,
        config: Config,
        size: GridSize,
    ) -> anyhow::Result<(Self, UnboundedReceiver<Event>)> {
        let (tx, rx) = unbounded();
        let term = Arc::new(FairMutex::new(Term::new(config, &size, Listener(tx.clone()))));

        let mut env = HashMap::new();
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "lumen".into());
        // For `lumen msg` and plugins run from this pane.
        if let Some(socket) = crate::api::socket_path() {
            env.insert("LUMEN_SOCKET".into(), socket.display().to_string());
            env.insert("LUMEN_PANE".into(), pane.to_string());
        }

        // Our scripts put this on every mark, so output can't fake them.
        let key = shell_integration::new_key();
        env.insert("LUMEN_MARK_KEY".into(), key.clone());

        let shell = shell.map(<[String]>::to_vec).unwrap_or_else(shell_integration::default_shell);
        let (shell, expect_key) = if integrate {
            let (cmd, extra) = shell_integration::inject(&shell);
            let injected = !extra.is_empty();
            env.extend(extra);
            (cmd, injected)
        } else {
            (shell, false)
        };
        let options = tty::Options {
            shell: shell.split_first().map(|(program, args)| tty::Shell::new(program.clone(), args.to_vec())),
            working_directory: cwd.filter(|p| p.is_dir()).or_else(fallback_cwd),
            drain_on_exit: false,
            env,
            #[cfg(target_os = "windows")]
            escape_args: false,
        };
        let pty = tty::new(&options, size.window_size(), 0)?;
        let child_pid = pty.child().id();
        let shell_state = Arc::new(Mutex::new(Shared::default()));
        let pty = MarkedPty::new(pty, shell_state.clone(), pane, key, expect_key)?;
        let event_loop = EventLoop::new(term.clone(), Listener(tx), pty, false, false)?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();

        Ok((Self { term, backend: Backend::Process { notifier, child_pid }, size, shell_state }, rx))
    }

    /// A pane's shell, whatever happens: yours (`shell`, or `$SHELL`); if
    /// that can't be started, `/bin/sh` saying why; and if nothing can be,
    /// a pane that only says so. A wrong `shell` setting mustn't take Lumen
    /// down with it.
    pub fn shell(
        cwd: Option<PathBuf>,
        shell: Option<&[String]>,
        integrate: bool,
        pane: u64,
        config: Config,
        size: GridSize,
    ) -> (Self, UnboundedReceiver<Event>) {
        let first = match Self::spawn(cwd.clone(), shell, integrate, pane, config.clone(), size) {
            Ok(spawned) => return spawned,
            Err(e) => e,
        };
        let wanted = shell.map(<[String]>::to_vec).unwrap_or_else(shell_integration::default_shell).join(" ");
        let why = format!("Lumen couldn't start your shell, {wanted}: {first}");
        let fix = if shell.is_some() { "Check `shell` in config.toml." } else { "Check $SHELL." };
        // The message goes as arguments, so nothing in it is read as shell.
        let fallback = [
            "/bin/sh",
            "-c",
            r#"printf '\033[31m%s\033[0m\n%s This is /bin/sh instead.\n\n' "$1" "$2"; exec /bin/sh"#,
            "sh",
            why.as_str(),
            fix,
        ]
        .map(String::from);
        match Self::spawn(cwd, Some(&fallback), false, pane, config.clone(), size) {
            Ok(spawned) => spawned,
            Err(e) => Self::message(
                &format!("\x1b[31m{why}\x1b[0m\r\n/bin/sh couldn't be started either: {e}\r\n{fix}\r\n"),
                config,
            ),
        }
    }

    /// A pane with no program at all, showing `text`.
    fn message(text: &str, config: Config) -> (Self, UnboundedReceiver<Event>) {
        let (tx, rx) = unbounded();
        let size = GridSize::DEFAULT;
        let term = Arc::new(FairMutex::new(Term::new(config, &size, Listener(tx.clone()))));
        let backend = Backend::Message { parser: Box::default(), events: tx };
        let mut this = Self { term, backend, size, shell_state: Arc::new(Mutex::new(Shared::default())) };
        this.feed(text.as_bytes());
        (this, rx)
    }

    /// A pane with no program: `owner` draws it with [`Pty::feed`], and gets
    /// what you type in it.
    pub fn plugin(owner: Owner, pane: u64, config: Config) -> (Self, UnboundedReceiver<Event>) {
        let (tx, rx) = unbounded();
        let size = GridSize { cols: 80, rows: 24, cell_width: 8, cell_height: 16 };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, Listener(tx.clone()))));
        let backend = Backend::Plugin { owner, pane, parser: Box::default(), events: tx };
        (Self { term, backend, size, shell_state: Arc::new(Mutex::new(Shared::default())) }, rx)
    }

    /// The plugin drawing this pane, if it's a plugin pane.
    pub fn owner(&self) -> Option<&Owner> {
        match &self.backend {
            Backend::Plugin { owner, .. } => Some(owner),
            Backend::Process { .. } | Backend::Message { .. } => None,
        }
    }

    /// Show `bytes` in a pane with no program (a plugin's, or a message),
    /// as if a program had printed them.
    pub fn feed(&mut self, bytes: &[u8]) {
        if let Backend::Plugin { parser, events, .. } | Backend::Message { parser, events } = &mut self.backend {
            parser.advance(&mut *self.term.lock(), bytes);
            let _ = events.unbounded_send(Event::Wakeup);
        }
    }

    /// Input for the program: typed keys, pastes, and replies to its
    /// queries. A plugin pane's input goes to its plugin.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        match &self.backend {
            Backend::Process { notifier, .. } => notifier.notify(bytes),
            Backend::Message { .. } => {}
            Backend::Plugin { owner, pane, .. } => {
                let bytes = bytes.into();
                owner.send(*pane, "input", serde_json::json!({ "text": String::from_utf8_lossy(&bytes) }));
            }
        }
    }

    pub fn resize(&mut self, size: GridSize) {
        if size == self.size || size.cols == 0 || size.rows == 0 {
            return;
        }
        self.size = size;
        match &mut self.backend {
            Backend::Process { notifier, .. } => notifier.on_resize(size.window_size()),
            Backend::Message { .. } => {}
            Backend::Plugin { owner, pane, .. } => {
                owner.send(*pane, "resize", serde_json::json!({ "cols": size.cols, "rows": size.rows }))
            }
        }
        self.term.lock().resize(size);
    }

    pub fn window_size(&self) -> WindowSize {
        self.size.window_size()
    }

    /// Best-effort working directory of the shell, used so new panes open where you are.
    /// Prefers what the shell reported (OSC 7), which also works on macOS.
    pub fn cwd(&self) -> Option<PathBuf> {
        if let Some(cwd) = self.shell_state.lock().unwrap().cwd.clone().filter(|p| p.is_dir()) {
            return Some(cwd);
        }
        let Backend::Process { child_pid, .. } = &self.backend else { return None };
        #[cfg(target_os = "linux")]
        {
            std::fs::read_link(format!("/proc/{child_pid}/cwd")).ok()
        }
        #[cfg(target_os = "macos")]
        {
            macos_cwd(*child_pid)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = child_pid;
            None
        }
    }
}

/// A process's working directory, from the kernel (macOS has no /proc).
#[cfg(target_os = "macos")]
fn macos_cwd(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: the buffer is a zeroed proc_vnodepathinfo of exactly `size` bytes.
    let written = unsafe {
        libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDVNODEPATHINFO, 0, info.as_mut_ptr().cast(), size)
    };
    if written != size {
        return None;
    }
    // SAFETY: proc_pidinfo filled the whole struct.
    let info = unsafe { info.assume_init() };
    // vip_path is a NUL-terminated C string stored as [[c_char; 32]; 32].
    let raw: Vec<u8> = info.pvi_cdir.vip_path.iter().flatten().map(|&c| c as u8).take_while(|&b| b != 0).collect();
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(&raw));
    path.is_dir().then_some(path)
}

/// Where a pane starts when it isn't given a directory. Normally the shell
/// inherits Lumen's own directory, so `lumen` run from a project opens there.
/// But apps launched from Finder, the Dock or a desktop menu start in `/`,
/// and a terminal there is useless, so those open in your home folder.
fn fallback_cwd() -> Option<PathBuf> {
    let own = std::env::current_dir().ok();
    if own.as_deref().is_some_and(|p| p != Path::new("/")) {
        return None;
    }
    std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_dir())
}

impl Drop for Pty {
    fn drop(&mut self) {
        match &self.backend {
            Backend::Process { notifier, .. } => {
                let _ = notifier.0.send(Msg::Shutdown);
            }
            Backend::Plugin { owner, pane, .. } => owner.send(*pane, "closed", serde_json::json!({})),
            Backend::Message { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line, Point};
    use std::time::{Duration, Instant};

    fn screen(pty: &Pty) -> String {
        let term = pty.term.lock();
        let bottom = Line(term.screen_lines() as i32 - 1);
        term.bounds_to_string(Point::new(Line(0), Column(0)), Point::new(bottom, term.last_column()))
    }

    #[test]
    fn a_shell_that_isnt_there_gets_bin_sh_and_says_so() {
        let missing = ["/nonexistent/lumen-test-shell".to_string()];
        // What used to be fatal: starting it fails.
        assert!(Pty::spawn(None, Some(&missing), true, 1, Config::default(), GridSize::DEFAULT).is_err());

        let (pty, _rx) = Pty::shell(None, Some(&missing), true, 1, Config::default(), GridSize::DEFAULT);
        assert!(matches!(pty.backend, Backend::Process { .. }), "/bin/sh runs in its place");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !screen(&pty).contains("This is /bin/sh instead.") {
            assert!(Instant::now() < deadline, "no message: {:?}", screen(&pty));
            std::thread::sleep(Duration::from_millis(20));
        }
        let text = screen(&pty).replace('\n', "");
        assert!(text.contains("Lumen couldn't start your shell, /nonexistent/lumen-test-shell"), "{text}");
        assert!(text.contains("Check `shell` in config.toml."), "{text}");
    }

    #[test]
    fn a_message_pane_shows_its_text_and_takes_no_input() {
        let (mut pty, _rx) = Pty::message("\x1b[31mno shell\x1b[0m\r\nsecond line\r\n", Config::default());
        assert_eq!(screen(&pty).trim_end(), "no shell\nsecond line");
        assert!(pty.owner().is_none() && pty.cwd().is_none_or(|_| true));
        pty.write(&b"ls\r"[..]);
        pty.resize(GridSize { cols: 40, rows: 10, ..GridSize::DEFAULT });
        assert!(screen(&pty).contains("no shell"));
    }
}
