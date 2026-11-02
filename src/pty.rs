//! Owns a shell process and its terminal state (alacritty_terminal), and bridges
//! terminal events onto an async channel the UI can await.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
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

pub struct Pty {
    pub term: SharedTerm,
    notifier: Notifier,
    pub size: GridSize,
    child_pid: u32,
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
    ) -> anyhow::Result<(Self, UnboundedReceiver<Event>)> {
        let (tx, rx) = unbounded();
        let size = GridSize { cols: 80, rows: 24, cell_width: 8, cell_height: 16 };
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

        let shell = shell.map(<[String]>::to_vec).unwrap_or_else(shell_integration::default_shell);
        let shell = if integrate {
            let (cmd, extra) = shell_integration::inject(&shell);
            env.extend(extra);
            cmd
        } else {
            shell
        };
        let options = tty::Options {
            shell: shell
                .split_first()
                .map(|(program, args)| tty::Shell::new(program.clone(), args.to_vec())),
            working_directory: cwd.filter(|p| p.is_dir()),
            drain_on_exit: false,
            env,
            #[cfg(target_os = "windows")]
            escape_args: false,
        };
        let pty = tty::new(&options, size.window_size(), 0)?;
        let child_pid = pty.child().id();
        let shell_state = Arc::new(Mutex::new(Shared::default()));
        let pty = MarkedPty::new(pty, shell_state.clone(), pane)?;
        let event_loop = EventLoop::new(term.clone(), Listener(tx), pty, false, false)?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();

        Ok((Self { term, notifier, size, child_pid, shell_state }, rx))
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    pub fn resize(&mut self, size: GridSize) {
        if size == self.size || size.cols == 0 || size.rows == 0 {
            return;
        }
        self.size = size;
        self.notifier.on_resize(size.window_size());
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
        #[cfg(target_os = "linux")]
        {
            std::fs::read_link(format!("/proc/{}/cwd", self.child_pid)).ok()
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = self.child_pid;
            None
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}
