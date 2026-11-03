//! `lumen info`: a flame in your theme's colours beside facts about the
//! system and this Lumen, and the 16 terminal colours. Like neofetch, but it
//! knows Lumen's settings too. Reads files and asks a couple of programs; never
//! needs a window, though it asks a web service for your public IP. In a
//! terminal the flame burns until you press a key.

use std::io::{IsTerminal, Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::mpsc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use gpui::{Hsla, Rgba};

use crate::settings::{Settings, config_path};
use crate::theme::Theme;

const USAGE: &str = "Usage: lumen info [--brief | --still] [--no-ip]\n\n\
    A flame, system facts and this Lumen's settings. In a terminal the flame\n\
    burns until you press a key (the key still reaches your shell).\n\n\
    \x20 --brief   burn for two seconds, then settle (for your shell's rc file)\n\
    \x20 --still   don't animate\n\
    \x20 --no-ip   don't look up your public IP (it asks api.ipify.org)\n\n\
    Piped, or with NO_COLOR=1, it's plain text.";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Motion {
    Still,
    Brief,
    Forever,
}

pub fn run(args: &[String]) -> i32 {
    let mut motion = Motion::Forever;
    let mut look_up_ip = true;
    for arg in args {
        match arg.as_str() {
            "--still" => motion = Motion::Still,
            "--brief" => motion = Motion::Brief,
            "--forever" => motion = Motion::Forever,
            "--no-ip" => look_up_ip = false,
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                return 0;
            }
            _ => {
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let stdout = std::io::stdout();
    let color = stdout.is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        && std::env::var("TERM").as_deref() != Ok("dumb");
    let settings = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| Settings::parse(&t))
        .unwrap_or_default();
    // Ask for the public IP while everything else is gathered.
    let ip = look_up_ip.then(spawn_ip_lookup);
    let mut facts = facts(&settings);
    let ip_index = ip.as_ref().map(|_| {
        let at = facts.iter().position(|(l, _)| *l == "Memory").map_or(facts.len(), |i| i + 1);
        facts.insert(at, (IP_LABEL, LOOKING_UP.into()));
        at
    });
    let text = render(&facts, &settings, color, STILL_T);
    // Animate only where the flame can be redrawn in place: in colour, and
    // with everything still on screen (lines that scrolled off can't be).
    let lines = text.matches('\n').count();
    let fits = window_size().is_some_and(|(_, rows)| (lines as u16) < rows);
    let animating = color && motion != Motion::Still && fits;
    let mut out = stdout.lock();
    if !animating {
        // Nothing to fill in later: wait (briefly) for the IP, then print.
        if let (Some(rx), Some(i)) = (&ip, ip_index) {
            facts[i].1 = ip_text(rx.recv_timeout(IP_WAIT).ok().flatten());
        }
        let _ = out.write_all(render(&facts, &settings, color, STILL_T).as_bytes());
        let _ = out.flush();
        return 0;
    }
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
    let width = facts.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0);
    let pending = ip.zip(ip_index).map(|(rx, i)| PendingIp {
        rx,
        // Output: a blank line, user@host, the rule, then the facts.
        line: 3 + i,
        label_width: width,
    });
    animate(&mut out, settings.theme(), lines, motion, pending)
}

// ---- public IP --------------------------------------------------------------

const IP_LABEL: &str = "Public IP";
const LOOKING_UP: &str = "looking up…";
/// How long to wait for the answer.
const IP_WAIT: Duration = Duration::from_millis(2500);

fn ip_text(ip: Option<IpAddr>) -> String {
    ip.map_or_else(|| "unavailable".to_string(), |ip| ip.to_string())
}

/// The public IP line, still to be filled in on screen.
struct PendingIp {
    rx: mpsc::Receiver<Option<IpAddr>>,
    /// Its line in the output (0 = the first).
    line: usize,
    label_width: usize,
}

/// Look up the public IP on another thread (DNS can't be given a timeout).
fn spawn_ip_lookup() -> mpsc::Receiver<Option<IpAddr>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(public_ip());
    });
    rx
}

/// Ask api.ipify.org, which answers with the address it sees, as plain text.
/// Plain HTTP keeps this free of a TLS library; the answer is only shown, and
/// only if it parses as an IP address.
fn public_ip() -> Option<IpAddr> {
    const HOST: &str = "api.ipify.org";
    let addr = (HOST, 80).to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok()?;
    // HTTP/1.0: a plain body, no chunked encoding to undo.
    write!(stream, "GET / HTTP/1.0\r\nHost: {HOST}\r\nUser-Agent: lumen-info\r\n\r\n").ok()?;
    let mut response = Vec::new();
    stream.take(4096).read_to_end(&mut response).ok()?;
    ip_from_response(&String::from_utf8_lossy(&response))
}

/// The address in a successful plain-text HTTP response, if that's all it is.
fn ip_from_response(response: &str) -> Option<IpAddr> {
    let (head, body) = response.split_once("\r\n\r\n")?;
    let status = head.lines().next()?;
    if !status.split_whitespace().nth(1).is_some_and(|code| code == "200") {
        return None;
    }
    body.trim().parse().ok()
}

// ---- the flame -------------------------------------------------------------

/// The flame is FLAME_W cells wide and FLAME_ROWS tall; each cell shows two
/// pixels stacked (with ▀ and ▄), so it's drawn on a FLAME_W x 2*FLAME_ROWS grid.
const FLAME_W: usize = 15;
const FLAME_ROWS: usize = 8;
const PX_H: usize = FLAME_ROWS * 2;
/// The moment the settled flame is drawn at.
const STILL_T: f32 = 0.0;

/// How hot each pixel is (0 = empty, 1 = white-hot) at time `t` seconds: a
/// teardrop, round at the bottom and drawn up to a tip, swaying more towards
/// the tip, with the tip's height flickering.
fn flame(t: f32) -> [[f32; FLAME_W]; PX_H] {
    let mut px = [[0.0f32; FLAME_W]; PX_H];
    // Flicker: a few unrelated waves, so it never visibly repeats.
    let tip = 1.0 + 0.10 * (t * 7.3).sin() + 0.06 * (t * 13.1 + 1.0).sin() + 0.04 * (t * 23.0).sin();
    let half = FLAME_W as f32 / 2.0;
    for (y, row) in px.iter_mut().enumerate() {
        // v: 0 at the bottom pixel row, 1 at the top.
        let v = (PX_H - 1 - y) as f32 / (PX_H - 1) as f32;
        let width = if v < 0.28 {
            (v / 0.28).sqrt().max(0.15) // rounded base
        } else {
            (1.0 - (v - 0.28) / (0.70 * tip)).max(0.0).powf(1.4) // narrowing to the tip
        };
        let hw = width * (half - 1.4);
        if hw <= 0.05 {
            continue;
        }
        let sway = (t * 2.3 + v * 2.8).sin() * v * 1.5 + (t * 5.9).sin() * v * v * 0.8;
        let cx = half + sway;
        for (x, p) in row.iter_mut().enumerate() {
            let d = ((x as f32 + 0.5) - cx).abs() / hw;
            if d < 1.0 {
                // Hottest low in the middle, cooling outwards and upwards.
                *p = ((1.0 - d).powf(0.6) * (1.0 - 0.45 * v)).clamp(0.0, 1.0);
            }
        }
    }
    px
}

/// The colour of a pixel this hot: a white-hot core, then yellow, orange and
/// red, fading into the background at the edges.
fn flame_color(heat: f32, theme: &Theme) -> Rgba {
    let (yellow, red) = (theme.ansi[11], theme.ansi[9]);
    let white = Hsla { h: 0.0, s: 0.0, l: 1.0, a: 1.0 };
    let base = if heat > 0.80 {
        mix(white, yellow, 0.55)
    } else if heat > 0.58 {
        Rgba::from(yellow)
    } else if heat > 0.36 {
        mix(yellow, red, 0.5)
    } else {
        Rgba::from(red)
    };
    let edge = (heat / 0.22).min(1.0);
    let pane = Rgba::from(theme.pane);
    let m = |a: f32, b: f32| a * edge + b * (1.0 - edge);
    Rgba { r: m(base.r, pane.r), g: m(base.g, pane.g), b: m(base.b, pane.b), a: 1.0 }
}

/// The flame at time `t` as FLAME_ROWS lines, each exactly FLAME_W cells.
fn flame_lines(t: f32, theme: &Theme, color: bool) -> Vec<String> {
    const EMPTY: f32 = 0.04;
    let px = flame(t);
    (0..FLAME_ROWS)
        .map(|row| {
            let (top, bottom) = (&px[row * 2], &px[row * 2 + 1]);
            let mut line = String::new();
            for x in 0..FLAME_W {
                let (a, b) = (top[x], bottom[x]);
                if !color {
                    // Plain text: one shade per cell.
                    let h = (a + b) / 2.0;
                    line.push(match h {
                        h if h < EMPTY => ' ',
                        h if h < 0.3 => '░',
                        h if h < 0.55 => '▒',
                        h if h < 0.8 => '▓',
                        _ => '█',
                    });
                    continue;
                }
                match (a > EMPTY, b > EMPTY) {
                    (false, false) => line.push_str("\x1b[0m "),
                    (true, false) => line.push_str(&format!("\x1b[0m{}▀", fg_rgb(flame_color(a, theme)))),
                    (false, true) => line.push_str(&format!("\x1b[0m{}▄", fg_rgb(flame_color(b, theme)))),
                    (true, true) => line.push_str(&format!(
                        "{}{}▀",
                        fg_rgb(flame_color(a, theme)),
                        bg_rgb(flame_color(b, theme))
                    )),
                }
            }
            if color {
                line.push_str("\x1b[0m");
            }
            line
        })
        .collect()
}

// ---- animation --------------------------------------------------------------

/// Set by a signal handler: the signal that asked us to stop, or 0.
static STOP: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(sig: libc::c_int) {
    STOP.store(sig, Ordering::SeqCst);
}

/// Redraw the flame in place, `lines` lines up from the cursor, until the
/// time's up, a key is pressed or a signal arrives. The cursor and terminal
/// settings are always put back. A key pressed is left for the shell.
fn animate(out: &mut impl Write, theme: &Theme, lines: usize, motion: Motion, mut ip: Option<PendingIp>) -> i32 {
    const FRAME: Duration = Duration::from_millis(80);
    const BRIEF: Duration = Duration::from_secs(2);
    let stdin_tty = std::io::stdin().is_terminal();
    // SAFETY: plain libc calls on our own process and its terminal; the
    // termios struct is fully written by tcgetattr before use.
    let saved = unsafe {
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as *const () as usize;
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
        let mut t: libc::termios = std::mem::zeroed();
        if stdin_tty && libc::tcgetattr(libc::STDIN_FILENO, &mut t) == 0 {
            // No echo, so a key pressed doesn't scribble on the art; not
            // canonical, so we see it without waiting for Enter. Ctrl+C still
            // signals.
            let mut quiet = t;
            quiet.c_lflag &= !(libc::ICANON | libc::ECHO);
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &quiet);
            Some(t)
        } else {
            None
        }
    };
    let _ = write!(out, "\x1b[?25l");
    let start = Instant::now();
    // From below the output up to the flame's first line (output starts with
    // one blank line).
    let up = lines - 1;
    let down = lines - 1 - FLAME_ROWS;
    let draw = |out: &mut dyn Write, t: f32| {
        let mut frame = format!("\x1b[{up}A");
        for line in flame_lines(t, theme, true) {
            frame.push_str("\r  ");
            frame.push_str(&line);
            frame.push('\n');
        }
        if down > 0 {
            frame.push_str(&format!("\x1b[{down}B"));
        }
        frame.push('\r');
        let _ = out.write_all(frame.as_bytes());
        let _ = out.flush();
    };
    // Fill in the public IP line when the answer comes (or give up on it).
    let fill_in = |out: &mut dyn Write, p: &PendingIp, value: &str| {
        let up = lines - p.line;
        let col = 2 + FLAME_W + 2;
        let fact = fact_line(IP_LABEL, value, p.label_width, theme.accent, true);
        let _ = write!(out, "\x1b[{up}A\r\x1b[{col}C{fact}\x1b[K\x1b[{up}B\r");
    };
    let mut settle_ip = |out: &mut dyn Write, wait: Duration| {
        if let Some(p) = &ip {
            let answer = if wait.is_zero() { p.rx.try_recv().ok() } else { p.rx.recv_timeout(wait).ok() };
            let timed_out = start.elapsed() >= IP_WAIT;
            if answer.is_some() || timed_out || !wait.is_zero() {
                fill_in(out, p, &ip_text(answer.flatten()));
                ip = None;
            }
        }
    };
    loop {
        let elapsed = start.elapsed();
        if STOP.load(Ordering::SeqCst) != 0 || (motion == Motion::Brief && elapsed >= BRIEF) {
            break;
        }
        settle_ip(out, Duration::ZERO);
        draw(out, elapsed.as_secs_f32() + 0.3);
        if stdin_tty {
            let mut fd = libc::pollfd { fd: libc::STDIN_FILENO, events: libc::POLLIN, revents: 0 };
            // SAFETY: one pollfd we own. Readable means a key: stop, and leave
            // it unread for whatever reads the terminal next.
            if unsafe { libc::poll(&mut fd, 1, FRAME.as_millis() as libc::c_int) } > 0 && fd.revents & libc::POLLIN != 0 {
                break;
            }
        } else {
            std::thread::sleep(FRAME);
        }
    }
    // Stopped before the answer came: give it a moment more.
    settle_ip(out, IP_WAIT.saturating_sub(start.elapsed()).min(Duration::from_millis(600)));
    draw(out, STILL_T);
    let _ = write!(out, "\x1b[?25h");
    let _ = out.flush();
    if let Some(t) = saved {
        // SAFETY: restoring the settings read above.
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t) };
    }
    match STOP.load(Ordering::SeqCst) {
        0 => 0,
        sig => 128 + sig,
    }
}

/// Label and value, in the order shown.
fn facts(settings: &Settings) -> Vec<(&'static str, String)> {
    let mut f: Vec<(&'static str, String)> = Vec::new();
    let mut add = |label: &'static str, value: Option<String>| {
        if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
            f.push((label, v));
        }
    };
    add("OS", os_name());
    add("Kernel", kernel());
    add("Uptime", uptime().map(|d| format_uptime(d.as_secs())));
    add("Shell", shell());
    add("CPU", cpu());
    add("GPU", gpu());
    add("Memory", memory());
    add("Terminal", Some(terminal()));
    add("Theme", Some(settings.theme().name.to_string()));
    add("Font", Some(format!("{} {}", settings.font_family, fmt_num(settings.font_size))));
    add("Integration", Some(if settings.shell_integration { "on" } else { "off" }.to_string()));
    add("Plugins", plugins(settings));
    f
}

fn render(facts: &[(&'static str, String)], settings: &Settings, color: bool, t: f32) -> String {
    let theme = settings.theme();
    let (accent, dim) = (theme.accent, theme.ansi[8]);
    let paint = |c: Hsla, s: &str| if color { format!("{}{s}\x1b[0m", fg(c)) } else { s.to_string() };

    let user = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_default();
    let host = hostname();
    let title = format!("{user}@{host}");
    let mut right: Vec<String> = vec![
        if color { format!("\x1b[1m{}\x1b[0m", paint(accent, &title)) } else { title.clone() },
        paint(dim, &"─".repeat(title.chars().count())),
    ];
    let width = facts.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0);
    for (label, value) in facts {
        right.push(fact_line(label, value, width, accent, color));
    }
    if color {
        right.push(String::new());
        right.extend(swatches(color));
    }

    let logo = flame_lines(t, theme, color);
    let logo_width = FLAME_W;

    let mut out = String::from("\n");
    for i in 0..logo.len().max(right.len()) {
        let left = logo.get(i).cloned().unwrap_or_else(|| " ".repeat(logo_width));
        let r = right.get(i).map(String::as_str).unwrap_or("");
        out.push_str(format!("  {left}  {r}").trim_end());
        out.push('\n');
    }
    out.push('\n');
    out
}

/// "Label    value", the label in the accent colour.
fn fact_line(label: &str, value: &str, width: usize, accent: Hsla, color: bool) -> String {
    let pad = " ".repeat(width.saturating_sub(label.chars().count()));
    let label = if color { format!("{}{label}\x1b[0m", fg(accent)) } else { label.to_string() };
    format!("{label}{pad}  {value}")
}

/// Two rows of eight blocks in the terminal's own 16 colours (so they show
/// whatever theme is drawing them).
fn swatches(color: bool) -> Vec<String> {
    if !color {
        return Vec::new();
    }
    let row = |base: u8| (0..8).map(|i| format!("\x1b[{}m   ", base + i)).collect::<String>() + "\x1b[0m";
    vec![row(40), row(100)]
}

fn fg(c: Hsla) -> String {
    fg_rgb(Rgba::from(c))
}

fn bg_rgb(rgb: Rgba) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("\x1b[48;2;{};{};{}m", byte(rgb.r), byte(rgb.g), byte(rgb.b))
}

fn fg_rgb(rgb: Rgba) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("\x1b[38;2;{};{};{}m", byte(rgb.r), byte(rgb.g), byte(rgb.b))
}

/// `a` over `b`, `t` of the way (1.0 is all `a`).
fn mix(a: Hsla, b: Hsla, t: f32) -> Rgba {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let m = |x: f32, y: f32| x * t + y * (1.0 - t);
    Rgba { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: 1.0 }
}

fn fmt_num(v: f32) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") }
}

// ---- facts ------------------------------------------------------------------

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Run a program and return its trimmed output, if it worked.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn os_name() -> Option<String> {
    if cfg!(target_os = "macos") {
        let name = output("sw_vers", &["-productName"])?;
        let version = output("sw_vers", &["-productVersion"]).unwrap_or_default();
        return Some(format!("{name} {version}").trim().to_string());
    }
    let text = read("/etc/os-release").or_else(|| read("/usr/lib/os-release"))?;
    os_release_name(&text)
}

/// PRETTY_NAME (else NAME) from an os-release file.
fn os_release_name(text: &str) -> Option<String> {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|v| !v.is_empty())
    };
    field("PRETTY_NAME").or_else(|| field("NAME"))
}

fn kernel() -> Option<String> {
    // SAFETY: uname fills the struct we own; on success its fields are NUL-terminated.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) != 0 {
            return None;
        }
        let s = |f: &[libc::c_char]| std::ffi::CStr::from_ptr(f.as_ptr()).to_string_lossy().into_owned();
        Some(format!("{} {} ({})", s(&u.sysname), s(&u.release), s(&u.machine)))
    }
}

fn hostname() -> String {
    let mut buf = [0 as libc::c_char; 256];
    // SAFETY: gethostname writes at most buf.len() bytes into our buffer.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr(), buf.len()) } == 0;
    if !ok {
        return "localhost".into();
    }
    buf[buf.len() - 1] = 0;
    // SAFETY: terminated above.
    let name = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
    name.split('.').next().unwrap_or(&name).to_string()
}

fn uptime() -> Option<Duration> {
    if let Some(text) = read("/proc/uptime") {
        let secs: f64 = text.split_whitespace().next()?.parse().ok()?;
        return Some(Duration::from_secs_f64(secs));
    }
    // macOS: kern.boottime = { sec = 1700000000, usec = 0 } ...
    let boot = output("sysctl", &["-n", "kern.boottime"])?;
    let sec: u64 = boot.split("sec =").nth(1)?.split(',').next()?.trim().parse().ok()?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(now.saturating_sub(sec)))
}

fn format_uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60);
    let plural = |n: u64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    let mut parts = Vec::new();
    if d > 0 {
        parts.push(plural(d, "day"));
    }
    if h > 0 {
        parts.push(plural(h, "hour"));
    }
    if m > 0 || parts.is_empty() {
        parts.push(plural(m, "min"));
    }
    parts.join(", ")
}

fn shell() -> Option<String> {
    let path = std::env::var("SHELL").ok()?;
    Some(Path::new(&path).file_name()?.to_string_lossy().into_owned())
}

fn cpu() -> Option<String> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).ok();
    let name = if cfg!(target_os = "macos") {
        output("sysctl", &["-n", "machdep.cpu.brand_string"])
    } else {
        read("/proc/cpuinfo").and_then(|t| cpu_model(&t))
    }?;
    Some(match threads {
        Some(n) => format!("{name} ({n} threads)"),
        None => name,
    })
}

/// The first "model name" (x86) or "Model"/"Hardware" (ARM) in /proc/cpuinfo,
/// with the trademark noise taken out.
fn cpu_model(text: &str) -> Option<String> {
    let value = |key: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_string()).filter(|v| !v.is_empty())
        })
    };
    let name = value("model name").or_else(|| value("Model")).or_else(|| value("Hardware"))?;
    let mut name = name.replace("(R)", "").replace("(TM)", "").replace(" CPU", "");
    if let Some(i) = name.find(" @ ") {
        name.truncate(i);
    }
    for suffix in [" Processor", " processor"] {
        if let Some(i) = name.find(suffix) {
            name.truncate(i);
        }
    }
    Some(name.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn gpu() -> Option<String> {
    if cfg!(target_os = "macos") {
        let text = output("system_profiler", &["SPDisplaysDataType"])?;
        return text.lines().find_map(|l| l.trim().strip_prefix("Chipset Model:").map(|v| v.trim().to_string()));
    }
    // NVIDIA's driver knows its cards' names even when lspci's list is
    // older than the card ("NVIDIA Device 2c05").
    let nvidia: Vec<String> = std::fs::read_dir("/proc/driver/nvidia/gpus")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("information")).ok())
        .filter_map(|t| nvidia_model(&t))
        .collect();
    let mut gpus: Vec<String> = output("lspci", &["-mm"]).map(|t| gpus_from_lspci(&t)).unwrap_or_default();
    if !nvidia.is_empty() {
        gpus.retain(|g| !g.starts_with("NVIDIA"));
        gpus.extend(nvidia);
    }
    (!gpus.is_empty()).then(|| gpus.join(", "))
}

/// "Model: NVIDIA GeForce RTX 5070 Ti" from the driver's information file.
fn nvidia_model(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("Model:")).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Display controllers from `lspci -mm`: "Vendor Device" for each.
fn gpus_from_lspci(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            // 01:00.0 "VGA compatible controller" "NVIDIA Corporation" "GA104 [GeForce RTX 3070]" ...
            let fields: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
            let class = fields.first()?;
            if !(class.contains("VGA") || class.contains("3D") || class.contains("Display")) {
                return None;
            }
            let vendor = fields.get(1)?.replace(" Corporation", "").replace(", Inc.", "").replace(" Inc.", "");
            let vendor = vendor.replace("Advanced Micro Devices, Inc. [AMD/ATI]", "AMD").replace("[AMD/ATI]", "");
            let device = fields.get(2)?;
            // Prefer the marketing name in brackets: "GA104 [GeForce RTX 3070]".
            let device = device.rsplit_once('[').and_then(|(_, b)| b.strip_suffix(']')).unwrap_or(device);
            Some(format!("{} {}", vendor.trim(), device.trim()))
        })
        .collect()
}

fn memory() -> Option<String> {
    if cfg!(target_os = "macos") {
        let total: u64 = output("sysctl", &["-n", "hw.memsize"])?.parse().ok()?;
        return Some(format!("{:.1} GiB", total as f64 / GIB));
    }
    let (used, total) = meminfo(&read("/proc/meminfo")?)?;
    Some(format!("{:.1} GiB / {:.1} GiB ({}%)", used as f64 / GIB, total as f64 / GIB, used * 100 / total.max(1)))
}

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Used and total bytes from /proc/meminfo (used = total - available).
fn meminfo(text: &str) -> Option<(u64, u64)> {
    let kb = |key: &str| -> Option<u64> {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    };
    let total = kb("MemTotal")? * 1024;
    let available = kb("MemAvailable")? * 1024;
    Some((total.saturating_sub(available), total))
}

fn terminal() -> String {
    let mut s = format!("Lumen {}", env!("CARGO_PKG_VERSION"));
    if std::env::var("TERM_PROGRAM").as_deref() != Ok("lumen") {
        s.push_str(" (not running in it)");
    }
    if let Some((cols, rows)) = window_size() {
        s.push_str(&format!(" · {cols}×{rows}"));
    }
    s
}

fn window_size() -> Option<(u16, u16)> {
    // SAFETY: TIOCGWINSZ fills the winsize we own.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        (libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0)
            .then_some((ws.ws_col, ws.ws_row))
    }
}

/// Built-in plugins that are on, then your own plugin folders.
fn plugins(settings: &Settings) -> Option<String> {
    let mut names: Vec<String> = settings.plugins.clone();
    if let Some(dir) = crate::plugins::plugins_dir()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        let mut own: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().join("plugin.toml").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !names.contains(n))
            .collect();
        own.sort();
        names.extend(own);
    }
    Some(if names.is_empty() { "none (Plugin Gallery in the palette)".into() } else { names.join(", ") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ip_only_from_a_clean_answer() {
        let ok = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n203.0.113.7";
        assert_eq!(ip_from_response(ok), Some("203.0.113.7".parse().unwrap()));
        assert_eq!(ip_from_response("HTTP/1.0 200 OK\r\n\r\n2001:db8::1\n"), Some("2001:db8::1".parse().unwrap()));
        assert_eq!(ip_from_response("HTTP/1.1 503 Busy\r\n\r\n203.0.113.7"), None);
        assert_eq!(ip_from_response("HTTP/1.1 200 OK\r\n\r\n\x1b]0;pwned\x07"), None);
        assert_eq!(ip_from_response("HTTP/1.1 200 OK\r\n\r\n<html>captive portal</html>"), None);
        assert_eq!(ip_from_response("garbage"), None);
    }

    #[test]
    fn os_release_prefers_the_pretty_name() {
        assert_eq!(os_release_name("NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n").unwrap(), "Ubuntu 24.04.1 LTS");
        assert_eq!(os_release_name("NAME=Arch\n").unwrap(), "Arch");
        assert_eq!(os_release_name("ID=x\n"), None);
    }

    #[test]
    fn cpu_names_are_tidied() {
        let intel = "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i7-9700K CPU @ 3.60GHz\n";
        assert_eq!(cpu_model(intel).unwrap(), "Intel Core i7-9700K");
        let amd = "model name\t: AMD Ryzen 9 7950X 16-Core Processor\n";
        assert_eq!(cpu_model(amd).unwrap(), "AMD Ryzen 9 7950X 16-Core");
        assert_eq!(cpu_model("Hardware\t: BCM2835\nModel\t: Raspberry Pi 4 Model B Rev 1.4\n").unwrap(), "Raspberry Pi 4 Model B Rev 1.4");
        assert_eq!(cpu_model("flags: fpu\n"), None);
    }

    #[test]
    fn memory_and_uptime() {
        let m = "MemTotal:       32768000 kB\nMemFree:  1000 kB\nMemAvailable:   16384000 kB\n";
        assert_eq!(meminfo(m), Some((16384000 * 1024, 32768000 * 1024)));
        assert_eq!(format_uptime(0), "0 mins");
        assert_eq!(format_uptime(61), "1 min");
        assert_eq!(format_uptime(90061), "1 day, 1 hour, 1 min");
        assert_eq!(format_uptime(7200), "2 hours");
    }

    #[test]
    fn gpus_come_from_display_controllers() {
        let lspci = "00:02.0 \"VGA compatible controller\" \"Intel Corporation\" \"Raptor Lake-S GT1 [UHD Graphics 770]\" -r04 \"\" \"\"\n\
                     01:00.0 \"3D controller\" \"NVIDIA Corporation\" \"AD107M [GeForce RTX 4060 Max-Q / Mobile]\" -ra1 \"\" \"\"\n\
                     00:1f.3 \"Audio device\" \"Intel Corporation\" \"Raptor Lake High Definition Audio Controller\"\n";
        assert_eq!(gpus_from_lspci(lspci), ["Intel UHD Graphics 770", "NVIDIA GeForce RTX 4060 Max-Q / Mobile"]);
        assert_eq!(nvidia_model("Model: \t\t NVIDIA GeForce RTX 5070 Ti\nIRQ: 112\n").unwrap(), "NVIDIA GeForce RTX 5070 Ti");
    }

    #[test]
    fn plain_output_has_no_escapes_and_lines_up() {
        let s = Settings::default();
        let facts = vec![("OS", "Test OS".to_string()), ("Memory", "1 GiB".to_string())];
        let out = render(&facts, &s, false, STILL_T);
        assert!(!out.contains('\x1b'), "{out}");
        assert!(out.contains("OS      Test OS"), "{out}");
        assert!(out.contains("Memory  1 GiB"), "{out}");
        let colored = render(&facts, &s, true, STILL_T);
        assert!(colored.contains("\x1b[38;2;") && colored.contains("\x1b[40m"));
    }

    #[test]
    fn every_flame_frame_is_the_same_size_and_shaped_like_a_flame() {
        let theme = Settings::default().theme().clone();
        for i in 0..40 {
            let t = i as f32 * 0.08;
            let plain = flame_lines(t, &theme, false);
            assert_eq!(plain.len(), FLAME_ROWS);
            assert!(plain.iter().all(|l| l.chars().count() == FLAME_W), "{plain:?}");
            // Coloured cells: one character each, whatever the escapes.
            let colored = flame_lines(t, &theme, true);
            for line in &colored {
                let visible = line.split('\x1b').map(|seg| seg.split_once('m').map_or(seg, |(_, r)| r)).collect::<String>();
                assert_eq!(visible.chars().count(), FLAME_W, "{line:?}");
            }
            let px = flame(t);
            let lit = |y: usize| px[y].iter().filter(|&&h| h > 0.04).count();
            // Wider low down than at the top, and nothing lit in the corners.
            assert!(lit(PX_H - 4) > lit(3), "frame {i}");
            assert!(px[PX_H - 1][0] < 0.04 && px[0][FLAME_W - 1] < 0.04);
        }
        // It moves.
        assert_ne!(flame_lines(0.0, &theme, false), flame_lines(0.5, &theme, false));
    }
}
