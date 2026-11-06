//! `lumen campfire`: a campfire that fills the pane. Flames rise from a bed
//! of coals under crossed logs, inside a ring of stones; sparks and smoke
//! drift up on the wind, past a treeline, under the stars. Runs on the
//! alternate screen, so your scrollback is untouched, until you press q, Esc
//! or Ctrl+C.
//!
//! It's drawn as pixel art, two pixels to a cell (with ▀), from a fixed
//! palette: flat colours, dark outlines, and dithering only where one band of
//! light meets the next. The flames are a few tongues, each a teardrop that
//! flickers, sways and leans with the wind, its edges broken up by noise that
//! runs upwards so they lick.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use gpui::{Hsla, Rgba};

use crate::settings::{Settings, config_path};

const USAGE: &str = "Usage: lumen campfire\n\n\
    A campfire that fills the pane, for cold coding sessions.\n\n\
    \x20 ↑ ↓  (or + -)   stoke the fire up or let it die down\n\
    \x20 ← →             change the wind\n\
    \x20 space           poke it, for a burst of sparks\n\
    \x20 q, Esc, Ctrl+C  put it out\n\n\
    Open it beside your work with: lumen msg split right --run 'lumen campfire'";

const FRAME: Duration = Duration::from_millis(33);
/// How long the key hint stays on the bottom row, and how long it fades.
const HINT_FOR: f32 = 5.0;
const HINT_FADE: f32 = 1.0;

type Rgb = [f32; 3];

// A fixed palette, as in pixel art: every colour drawn comes from here, or
// from the theme's background mixed towards one of these in a few steps.

/// The flames, coolest first, each with the heat it starts at.
const FIRE: [(f32, Rgb); 7] = [
    (0.10, [0.43, 0.10, 0.07]),
    (0.24, [0.70, 0.16, 0.08]),
    (0.40, [0.89, 0.33, 0.10]),
    (0.56, [0.97, 0.53, 0.14]),
    (0.70, [1.00, 0.71, 0.24]),
    (0.83, [1.00, 0.86, 0.45]),
    (0.93, [1.00, 0.96, 0.80]),
];
/// Coals and glowing wood, from dead to hottest.
const EMBERS: [Rgb; 4] = [[0.17, 0.07, 0.05], [0.52, 0.12, 0.06], [0.89, 0.33, 0.10], [1.0, 0.63, 0.20]];
const OUTLINE: Rgb = [0.09, 0.06, 0.05];
const BARK_LIGHT: Rgb = [0.47, 0.30, 0.17];
const BARK: Rgb = [0.34, 0.20, 0.11];
const BARK_DARK: Rgb = [0.22, 0.12, 0.07];
const CHARCOAL: Rgb = [0.15, 0.10, 0.08];
const WOOD_PALE: Rgb = [0.80, 0.64, 0.42];
const WOOD: Rgb = [0.62, 0.45, 0.27];
const STONE_LIGHT: Rgb = [0.56, 0.53, 0.50];
const STONE: Rgb = [0.40, 0.38, 0.36];
const STONE_DARK: Rgb = [0.26, 0.25, 0.24];
const STAR: Rgb = [0.92, 0.93, 1.0];
const WARM: Rgb = [1.0, 0.45, 0.12];

/// Set by a signal handler: the signal that asked us to stop, or 0.
static STOP: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(sig: libc::c_int) {
    STOP.store(sig, Ordering::SeqCst);
}

pub fn run(args: &[String]) -> i32 {
    if let Some(arg) = args.first() {
        if matches!(arg.as_str(), "-h" | "--help" | "help") {
            println!("{USAGE}");
            return 0;
        }
        eprintln!("{USAGE}");
        return 2;
    }
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        eprintln!("lumen campfire: it needs a terminal to burn in");
        return 1;
    }
    let settings = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| Settings::parse(&t))
        .unwrap_or_default();
    let theme = settings.theme();
    let look = Look { bg: rgb_of(theme.pane), hint: rgb_of(theme.subtext), dark: !theme.is_light };
    burn(&look)
}

/// The colours the fire is drawn among.
#[derive(Clone, Copy)]
struct Look {
    bg: Rgb,
    hint: Rgb,
    dark: bool,
}

fn burn(look: &Look) -> i32 {
    // SAFETY: plain libc calls on our own process and its terminal; the
    // termios struct is fully written by tcgetattr before use.
    let saved = unsafe {
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as *const () as usize;
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut t) == 0 {
            // Keys arrive at once and aren't echoed; Ctrl+C still signals.
            let mut quiet = t;
            quiet.c_lflag &= !(libc::ICANON | libc::ECHO);
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &quiet);
            Some(t)
        } else {
            None
        }
    };
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    // Alternate screen, no cursor, no autowrap (so the last cell can't scroll).
    let _ = write!(out, "\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[2J");

    let mut size = window_size().unwrap_or((80, 24));
    let mut fire = Fire::new(size.0 as usize, size.1 as usize * 2, seed(), *look);
    let mut prev: Option<Vec<Cell>> = None;
    let start = Instant::now();
    let mut last = start;
    'burning: loop {
        if STOP.load(Ordering::SeqCst) != 0 {
            break;
        }
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;
        if let Some(s) = window_size()
            && s != size
        {
            size = s;
            fire.resize(s.0 as usize, s.1 as usize * 2);
            prev = None;
            let _ = write!(out, "\x1b[0m\x1b[2J");
        }
        fire.step(dt);
        let (cols, rows) = (size.0.max(1) as usize, size.1.max(1) as usize);
        let mut frame = cells(&fire.pixels(), cols, rows);
        hint(&mut frame, cols, rows, start.elapsed().as_secs_f32(), look.hint);
        let text = encode(prev.as_deref(), &frame, cols);
        if out.write_all(text.as_bytes()).and_then(|_| out.flush()).is_err() {
            break;
        }
        prev = Some(frame);

        // Wait for the next frame, tending the fire as keys come in.
        let deadline = now + FRAME;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let mut fd = libc::pollfd { fd: libc::STDIN_FILENO, events: libc::POLLIN, revents: 0 };
            // SAFETY: one pollfd we own.
            let ready = unsafe { libc::poll(&mut fd, 1, left.as_millis().max(1) as libc::c_int) };
            if STOP.load(Ordering::SeqCst) != 0 {
                break 'burning;
            }
            if ready <= 0 || fd.revents & (libc::POLLIN | libc::POLLHUP) == 0 {
                continue;
            }
            let mut buf = [0u8; 64];
            // SAFETY: reading into a buffer we own, no further than its length.
            let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break 'burning; // the terminal went away
            }
            for key in parse_keys(&buf[..n as usize]) {
                match key {
                    Key::Quit => break 'burning,
                    Key::Up => fire.stoke(1.0),
                    Key::Down => fire.stoke(-1.0),
                    Key::Left => fire.blow(-1.0),
                    Key::Right => fire.blow(1.0),
                    Key::Poke => fire.poke(),
                }
            }
        }
    }

    let _ = write!(out, "\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l");
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

// ---- keys ------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Key {
    Quit,
    Up,
    Down,
    Left,
    Right,
    Poke,
}

fn parse_keys(bytes: &[u8]) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            if i + 2 < bytes.len() && matches!(bytes[i + 1], b'[' | b'O') {
                // An arrow key; anything else in this shape is ignored.
                keys.extend(match bytes[i + 2] {
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    b'C' => Some(Key::Right),
                    b'D' => Some(Key::Left),
                    _ => None,
                });
                i += 3;
                continue;
            }
            if i + 1 == bytes.len() {
                keys.push(Key::Quit); // Esc on its own
            }
            i += 1;
            continue;
        }
        keys.extend(match bytes[i] {
            b'q' | b'Q' | 0x03 => Some(Key::Quit),
            b'+' | b'=' | b'k' | b'w' => Some(Key::Up),
            b'-' | b'_' | b'j' | b's' => Some(Key::Down),
            b'h' | b'a' => Some(Key::Left),
            b'l' | b'd' => Some(Key::Right),
            b' ' => Some(Key::Poke),
            _ => None,
        });
        i += 1;
    }
    keys
}

// ---- the fire ----------------------------------------------------------------

/// A small, fast random number generator (xorshift64*); good enough for fire.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    (nanos ^ (std::process::id() as u64) << 32) | 1
}

struct Spark {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    age: f32,
    life: f32,
}

struct Star {
    x: usize,
    y: usize,
    /// How bright it is at its brightest: most are faint, a few aren't.
    shine: f32,
    phase: f32,
    speed: f32,
}

/// A puff of smoke: rises slowly, spreads and thins out.
struct Puff {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    /// Its radius at the end of its life.
    r: f32,
    age: f32,
    life: f32,
    /// Where in the noise its wisps come from, so no two look alike.
    seed: f32,
}

/// Where things go in a pane of a given size, in pixels (y grows down).
struct Layout {
    /// The fire's centre line.
    cx: f32,
    /// The pixel row the flames rise from; the ground is below it.
    base: usize,
    /// Half the width of the fire bed.
    hw: f32,
    /// How high the flames reach, on average.
    flame_h: f32,
    /// A log's radius.
    r: f32,
    /// Where the sky meets the ground, behind the fire.
    horizon: usize,
}

/// One tongue of flame: where along the bed it rises from (-1 to 1), how
/// tall it burns compared with the others, and where its flicker starts.
struct Tongue {
    at: f32,
    reach: f32,
    phase: f32,
}

/// A pine on the horizon, in silhouette.
struct Tree {
    x: f32,
    height: f32,
    /// Half its width at the bottom.
    spread: f32,
}

struct Fire {
    w: usize,
    h: usize,
    look: Look,
    heat: Vec<f32>,
    sparks: Vec<Spark>,
    smoke: Vec<Puff>,
    stars: Vec<Star>,
    trees: Vec<Tree>,
    /// Everything that keeps still (sky, glow, trees, ground), drawn once for
    /// the fire's size: each frame starts from a copy.
    scene: Vec<Rgb>,
    scene_for: f32,
    rng: Rng,
    /// How stoked the fire is: 1 is normal.
    size: f32,
    /// Wind you've added, on top of the breeze.
    wind: f32,
    /// A poke's flare-up, dying away to 0.
    surge: f32,
    tongues: Vec<Tongue>,
    t: f32,
}

impl Fire {
    fn new(w: usize, h: usize, seed: u64, look: Look) -> Self {
        let (w, h) = (w.max(1), h.max(2));
        let mut fire = Fire {
            w,
            h,
            look,
            heat: vec![0.0; w * h],
            sparks: Vec::new(),
            smoke: Vec::new(),
            stars: Vec::new(),
            trees: Vec::new(),
            scene: Vec::new(),
            scene_for: f32::NAN,
            rng: Rng(seed.max(1)),
            size: 1.0,
            wind: 0.0,
            surge: 0.0,
            tongues: Vec::new(),
            t: 0.0,
        };
        let horizon = fire.layout().horizon as f32;
        let rng = &mut fire.rng;
        if look.dark {
            // Thicker towards the top, thinning out towards the trees.
            fire.stars = (0..w * h / 700)
                .map(|_| Star {
                    x: (rng.f() * w as f32) as usize,
                    y: (rng.f().powf(1.5) * horizon * 0.75) as usize,
                    shine: rng.f().powi(3),
                    phase: rng.f() * std::f32::consts::TAU,
                    speed: 0.3 + rng.f() * 1.2,
                })
                .collect();
        }
        // Tall tongues in the middle of the bed, shorter ones to the sides.
        let n = 7;
        fire.tongues = (0..n)
            .map(|i| {
                let at = (i as f32 / (n - 1) as f32 * 2.0 - 1.0) * 0.8 + (rng.f() - 0.5) * 0.15;
                Tongue { at, reach: (1.0 - 0.6 * at.abs().powf(1.3)) * (0.85 + 0.25 * rng.f()), phase: rng.f() * 100.0 }
            })
            .collect();
        // A treeline, taller in some stretches than others.
        let tall = (h as f32 * 0.1).max(3.0);
        let mut x = -4.0;
        while x < w as f32 + 4.0 {
            let stand = 0.55 + 0.9 * noise(x * 0.03 + 5.0, 0.5);
            let height = tall * stand * (0.6 + 0.5 * rng.f());
            let spread = (height * (0.34 + 0.1 * rng.f())).max(1.0);
            fire.trees.push(Tree { x, height, spread });
            x += spread * (1.1 + 1.6 * rng.f());
        }
        fire
    }

    /// Start again at a new size, keeping how it's been tended.
    fn resize(&mut self, w: usize, h: usize) {
        let mut f = Fire::new(w, h, self.rng.next(), self.look);
        f.size = self.size;
        f.wind = self.wind;
        f.t = self.t;
        *self = f;
    }

    fn stoke(&mut self, by: f32) {
        self.size = (self.size + 0.15 * by).clamp(0.4, 1.6);
    }

    fn blow(&mut self, by: f32) {
        self.wind = (self.wind + 0.15 * by).clamp(-0.6, 0.6);
    }

    fn poke(&mut self) {
        self.surge = 1.0;
        let l = self.layout();
        for _ in 0..(l.hw as usize).clamp(8, 40) {
            self.spawn_spark(&l, 1.5);
        }
    }

    fn layout(&self) -> Layout {
        let (w, h) = (self.w as f32, self.h as f32);
        let ground = (h * 0.2).clamp(4.0, 50.0).min(h - 1.0);
        let scale = (w * 0.5).min(h * 0.8).max(4.0);
        let grow = self.size.sqrt();
        let hw = (scale * 0.2 * grow).max(2.0);
        // The logs and stones sit at the bottom of the pane: the front log
        // and the stones reach about 2.7 log radii below the bed. The radius
        // doesn't grow with stoking, so the fire stays put.
        let r = (scale * 0.2 * 0.17).clamp(1.0, 4.0);
        let base = (h - 2.0 - (r * 2.7 + 1.0).ceil()).max(1.0);
        let r = r.min((h - 1.0 - base) / 2.8).max(0.8);
        Layout {
            cx: w / 2.0,
            base: base as usize,
            hw,
            flame_h: (hw * 3.2 * grow).min(base * 0.6).max(2.0),
            r,
            horizon: (h - ground).max(1.0) as usize,
        }
    }

    /// The breeze: what you've set, plus a slow wander.
    fn wind_now(&self) -> f32 {
        self.wind + 0.22 * (self.t * 0.37).sin() + 0.12 * (self.t * 1.3 + 2.0).sin()
    }

    fn step(&mut self, dt: f32) {
        self.t += dt;
        self.surge *= (-dt * 1.5).exp();
        let l = self.layout();
        self.flames(&l);
        self.drift(&l, dt);
        self.smoulder(&l, dt);
        if self.scene_for != self.size {
            self.scene_for = self.size;
            self.scene = self.paint_scene(&l);
        }
    }

    /// Work out how hot each pixel is. Each tongue is a teardrop rising from
    /// the bed: it flickers taller and shorter, sways, leans with the wind,
    /// and its edges are broken up by noise that runs upwards, so they lick.
    /// Hottest low down the middle, cooling outwards and upwards; the
    /// palette turns that into a red rim around orange, yellow and white.
    fn flames(&mut self, l: &Layout) {
        let (w, t) = (self.w, self.t);
        let wind = self.wind_now();
        let flare = 1.0 + 0.3 * self.surge;
        self.heat.fill(0.0);
        let base = l.base as f32;
        for tongue in &self.tongues {
            let ph = tongue.phase;
            let height = l.flame_h * tongue.reach * flare * (0.75 + 0.35 * noise(t * 1.6 + ph, ph));
            let width = l.hw * (0.5 - 0.15 * tongue.at.abs());
            let x0 = l.cx + tongue.at * l.hw * 0.8;
            let top = (base - height).max(0.0) as usize;
            for y in top..=l.base {
                let v = (base - y as f32) / height; // 0 at the bed, 1 at the tip
                let sway = (t * 2.7 + ph + v * 3.0).sin() * v.powf(1.3) * width * 0.45;
                let centre = x0 + sway + wind * height * 0.5 * v * v;
                let half = width * (1.0 - v).powf(0.8) * (1.0 + 0.35 * (v * std::f32::consts::PI).sin());
                if half <= 0.05 {
                    continue;
                }
                let x_lo = (centre - half * 1.6).max(0.0) as usize;
                let x_hi = ((centre + half * 1.6 + 1.0).max(0.0) as usize).min(w);
                for x in x_lo..x_hi {
                    let lick = noise(x as f32 * 0.35 + ph * 10.0, y as f32 * 0.22 + t * 5.0) - 0.5;
                    let d = (x as f32 + 0.5 - centre).abs() / half + lick * (0.4 + 0.8 * v);
                    let heat = (1.0 - d).max(0.0).powf(0.7) * (1.2 - 0.85 * v) * flare;
                    let i = y * w + x;
                    if heat > self.heat[i] {
                        self.heat[i] = heat;
                    }
                }
            }
        }
    }

    fn spawn_spark(&mut self, l: &Layout, speed: f32) {
        let rng = &mut self.rng;
        let lift = (self.h as f32 / 60.0).sqrt().max(0.6);
        self.sparks.push(Spark {
            x: l.cx + (rng.f() * 2.0 - 1.0) * l.hw * 0.6,
            y: l.base as f32 - l.r * 1.5 - rng.f() * l.flame_h * 0.35,
            vx: (rng.f() - 0.5) * 0.5 * speed,
            vy: -(0.3 + 0.6 * rng.f()) * lift * speed,
            age: 0.0,
            life: 35.0 + 80.0 * rng.f(),
        });
    }

    /// Spawn sparks and carry them up on the wind.
    fn drift(&mut self, l: &Layout, dt: f32) {
        let frames = dt * 30.0;
        let mut chance = 0.12 * self.size * (l.hw / 12.0) * (1.0 + 4.0 * self.surge) * frames;
        while chance > 0.0 {
            if self.rng.f() < chance.min(1.0) {
                self.spawn_spark(l, 1.0);
            }
            chance -= 1.0;
        }
        let wind = self.wind_now();
        let Fire { sparks, rng, w, .. } = self;
        for s in sparks.iter_mut() {
            s.vx = (s.vx + ((rng.f() - 0.5) * 0.12 + wind * 0.015) * frames) * 0.97;
            s.x += s.vx * frames;
            s.y += s.vy * frames;
            s.vy *= 0.995;
            s.age += frames;
        }
        let w = *w as f32;
        sparks.retain(|s| s.age < s.life && s.y >= 0.0 && s.x >= 0.0 && s.x < w);
        sparks.truncate(400);
    }

    /// Puff smoke off the tops of the flames and let it drift away.
    fn smoulder(&mut self, l: &Layout, dt: f32) {
        let frames = dt * 30.0;
        let lift = (self.h as f32 / 60.0).sqrt().max(0.6);
        if self.rng.f() < 0.12 * self.size.sqrt() * frames {
            let rng = &mut self.rng;
            self.smoke.push(Puff {
                x: l.cx + (rng.f() - 0.5) * l.hw * 0.6,
                y: l.base as f32 - l.flame_h * (0.75 + 0.25 * rng.f()),
                vx: (rng.f() - 0.5) * 0.1,
                vy: -(0.12 + 0.12 * rng.f()) * lift,
                r: l.hw * (0.25 + 0.2 * rng.f()),
                age: 0.0,
                life: 180.0 + 120.0 * rng.f(),
                seed: rng.f() * 100.0,
            });
        }
        let wind = self.wind_now();
        for p in &mut self.smoke {
            p.vx = (p.vx + wind * 0.004 * frames) * 0.99;
            p.x += p.vx * frames;
            p.y += p.vy * frames;
            p.age += frames;
        }
        self.smoke.retain(|p| p.age < p.life && p.y + p.r > 0.0);
        self.smoke.truncate(60);
    }

    /// The still part of the picture: sky with the fire's glow in it, the
    /// treeline, and the ground with a pool of firelight on it.
    fn paint_scene(&self, l: &Layout) -> Vec<Rgb> {
        let (w, h, look) = (self.w, self.h, self.look);
        let bg = look.bg;
        let (sky_warm, tree, ground, ground_lit) = if look.dark {
            (
                mix(bg, [0.45, 0.17, 0.08], 0.2),
                mix(bg, [0.02, 0.02, 0.03], 0.6),
                mix(bg, [0.09, 0.065, 0.055], 0.8),
                [0.32, 0.17, 0.09],
            )
        } else {
            (
                bg, // in daylight the sky doesn't glow
                mix(bg, [0.2, 0.27, 0.24], 0.7),
                mix(bg, [0.55, 0.47, 0.38], 0.6),
                [0.85, 0.58, 0.36],
            )
        };
        let (gx, gy) = (l.cx, l.base as f32 - l.flame_h * 0.4);
        let glow_r2 = (l.flame_h * 1.3 + l.hw).powi(2);
        let (pool_rx, pool_ry) = (l.hw * 3.4, ((h - l.horizon) as f32 * 0.7).max(1.0));
        let pool_y = l.base as f32 + l.r;
        let mut px = vec![bg; w * h];
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                px[y * w + x] = if y < l.horizon {
                    let v = (-((fx - gx).powi(2) + (fy - gy).powi(2)) / glow_r2).exp();
                    let lit = banded(v * 0.95, 3.0, x, y);
                    if self.tree_at(fx, fy, l.horizon as f32) {
                        mix(tree, sky_warm, lit * 0.25)
                    } else {
                        mix(bg, sky_warm, lit)
                    }
                } else {
                    let e = ((fx - l.cx) / pool_rx).powi(2) + ((fy - pool_y) / pool_ry).powi(2);
                    mix(ground, ground_lit, banded((1.0 - e).max(0.0), 3.0, x, y) * 0.6)
                };
            }
        }
        px
    }

    /// Whether (x, y) is inside a tree on the horizon, or the scrub between.
    fn tree_at(&self, x: f32, y: f32, horizon: f32) -> bool {
        if y >= horizon - 1.0 - 1.5 * noise(x * 0.05, 3.0) {
            return true;
        }
        self.trees.iter().any(|t| {
            let top = horizon - t.height;
            if y < top {
                return false;
            }
            // Down from the tip, widening in three tiers.
            let d = (y - top) / t.height;
            let tier = (d * 3.0).fract();
            (x - t.x).abs() <= t.spread * d * (0.65 + 0.35 * tier) + 0.3
        })
    }

    /// Every pixel's colour, row by row.
    fn pixels(&self) -> Vec<Rgb> {
        let (w, h, t, look) = (self.w, self.h, self.t, self.look);
        let l = self.layout();
        let mut px = if self.scene.len() == w * h { self.scene.clone() } else { self.paint_scene(&l) };

        for s in &self.stars {
            let i = s.y * w + s.x;
            if s.y >= l.horizon || i >= px.len() {
                continue;
            }
            let shine = s.shine * (0.8 + 0.2 * (t * s.speed + s.phase).sin());
            px[i] = mix(look.bg, STAR, if shine > 0.55 { 0.95 } else { 0.4 });
            // The brightest few sparkle, with four short points.
            if shine > 0.75 && s.x > 0 && s.x + 1 < w && s.y > 0 && s.y + 1 < l.horizon {
                for j in [i - 1, i + 1, i - w, i + w] {
                    px[j] = mix(look.bg, STAR, 0.4);
                }
            }
        }

        // Smoke: faint puffs, flat, with dithered edges.
        let smoke: Rgb = if look.dark { [0.62, 0.60, 0.62] } else { [0.3, 0.28, 0.27] };
        for p in &self.smoke {
            let f = p.age / p.life;
            let r = p.r * (0.35 + 0.65 * f.sqrt());
            let alpha = (std::f32::consts::PI * f).sin();
            let x0 = (p.x - r).max(0.0) as usize;
            let x1 = ((p.x + r + 1.0).max(0.0) as usize).min(w);
            let y0 = (p.y - r).max(0.0) as usize;
            let y1 = ((p.y + r + 1.0).max(0.0) as usize).min(l.base);
            for y in y0..y1 {
                for x in x0..x1 {
                    let (dx, dy) = (x as f32 + 0.5 - p.x, y as f32 + 0.5 - p.y);
                    let d2 = (dx * dx + dy * dy) / (r * r);
                    if d2 >= 1.0 {
                        continue;
                    }
                    let wisp = noise(x as f32 * 0.16 + p.seed, y as f32 * 0.1 - t * 0.3);
                    let a = alpha * (1.0 - d2) * (0.4 + 1.0 * wisp);
                    if banded(a, 1.0, x, y) > 0.0 {
                        px[y * w + x] = mix(px[y * w + x], smoke, 0.09);
                    }
                }
            }
        }

        // The flames.
        for y in 0..=l.base.min(h - 1) {
            for x in 0..w {
                let i = y * w + x;
                let up = if y > 0 { self.heat[i - w] } else { 0.0 };
                let down = if y + 1 < h { self.heat[i + w] } else { 0.0 };
                let heat = 0.25 * up + 0.5 * self.heat[i] + 0.25 * down;
                if let Some(c) = fire_color(heat, x, y) {
                    px[i] = c;
                }
            }
        }

        // The coal bed, then two logs crossed at the back, stones either side
        // and one log across the front.
        let flicker = 0.85 + 0.1 * (t * 5.3).sin() + 0.05 * (t * 11.7).sin() + 0.2 * self.surge;
        let yb = l.base as f32;
        let r = l.r;
        self.draw_coals(&mut px, &l, flicker);
        let rise = l.hw * 0.35 + r;
        let back = [
            ([l.cx - l.hw * 1.15, yb + r * 0.9], [l.cx + l.hw * 0.8, yb - rise]),
            ([l.cx + l.hw * 1.15, yb + r * 0.9], [l.cx - l.hw * 0.8, yb - rise]),
        ];
        for (k, (a, b)) in back.into_iter().enumerate() {
            self.draw_log(&mut px, &l, a, b, k as f32);
        }
        let (rx, ry) = (r * 1.5 + 1.0, r * 0.95 + 0.6);
        let sy = (yb + 1.0 + r * 1.6).min(h as f32 - 0.5);
        let off = l.hw * 1.15 + r + rx * 0.9;
        for side in [-1.0f32, 1.0] {
            for (k, along) in [(1.0, 0.0), (0.85, 1.9)] {
                let sx = l.cx + side * (off + rx * along);
                let c = [sx, sy + 0.3 * along];
                self.draw_shadow(&mut px, [c[0] + side * rx * 0.3, c[1] + ry * 0.7], [rx * k * 1.15, ry * k * 0.45]);
                self.draw_stone(&mut px, &l, c, [rx * k, ry * k]);
            }
        }
        let front = ([l.cx - l.hw * 0.9, yb + r * 1.6], [l.cx + l.hw * 0.95, yb + r * 1.3]);
        self.draw_log(&mut px, &l, front.0, front.1, 2.0);

        for s in &self.sparks {
            let (x, y) = (s.x as usize, s.y as usize);
            if x < w && y < h {
                let f = (s.age / s.life).clamp(0.0, 0.999);
                px[y * w + x] = FIRE[6 - (f * 6.0) as usize].1;
            }
        }
        px
    }

    /// Glowing coals heaped under the logs, brightening and dimming in patches.
    fn draw_coals(&self, px: &mut [Rgb], l: &Layout, flicker: f32) {
        let (cy, rx, ry) = (l.base as f32 + l.r * 0.6, l.hw * 0.95, l.r * 1.4 + 0.5);
        let x0 = (l.cx - rx).max(0.0) as usize;
        let x1 = ((l.cx + rx + 1.0) as usize).min(self.w);
        let y0 = (cy - ry).max(0.0) as usize;
        let y1 = ((cy + ry + 1.0) as usize).min(self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = ((x as f32 + 0.5 - l.cx) / rx, (y as f32 + 0.5 - cy) / ry);
                let e = dx * dx + dy * dy;
                if e > 1.0 {
                    continue;
                }
                let glow = noise(x as f32 * 0.45 + 7.0, y as f32 * 0.6 - self.t * 0.5) * (1.0 - 0.5 * e) * flicker * 1.3;
                px[y * self.w + x] = EMBERS[(dither(glow.min(1.0), 3.0, x, y) * 3.0).round() as usize];
            }
        }
    }

    /// A log from `a` to `b`, outlined: bark in three tones lit from above,
    /// with ridges along it, charred where it's in the fire and glowing along
    /// its top there, and end grain with rings at its ends.
    fn draw_log(&self, px: &mut [Rgb], l: &Layout, a: [f32; 2], b: [f32; 2], which: f32) {
        let r = l.r;
        let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
        let len2 = (abx * abx + aby * aby).max(1e-6);
        let len = len2.sqrt();
        // The normal on the log's upper side.
        let (nx, ny) = if abx >= 0.0 { (aby / len, -abx / len) } else { (-aby / len, abx / len) };
        let x0 = (a[0].min(b[0]) - r - 1.0).max(0.0) as usize;
        let x1 = ((a[0].max(b[0]) + r + 1.0).max(0.0) as usize).min(self.w);
        let y0 = (a[1].min(b[1]) - r - 1.0).max(0.0) as usize;
        let y1 = ((a[1].max(b[1]) + r + 1.0).max(0.0) as usize).min(self.h);
        let outlined = r >= 1.6;
        for y in y0..y1 {
            for x in x0..x1 {
                let (p0, p1) = (x as f32 + 0.5, y as f32 + 0.5);
                let u = ((p0 - a[0]) * abx + (p1 - a[1]) * aby) / len2;
                let uc = u.clamp(0.0, 1.0);
                let (dx, dy) = (p0 - (a[0] + abx * uc), p1 - (a[1] + aby * uc));
                let dist = (dx * dx + dy * dy).sqrt();
                if dist > r {
                    continue;
                }
                let c = if outlined && dist > r - 0.85 {
                    OUTLINE
                } else if !(0.0..=1.0).contains(&u) {
                    if ((dist * 1.4) as i32) % 2 == 0 { WOOD_PALE } else { WOOD }
                } else {
                    let side = ((dx * nx + dy * ny) / r).clamp(-1.0, 1.0); // 1 on top
                    let along = u * len;
                    let band = (side * r).round() as i32;
                    let ridge = hash(band, (along / 3.0) as i32 + which as i32 * 1000) < 0.3;
                    let inside = (1.0 - (p0 - l.cx).abs() / l.hw).clamp(0.0, 1.0);
                    if inside > 0.35 && side > 0.45 {
                        // A glowing seam where the flames lick it.
                        let hot = noise(along * 0.25 + which * 13.0, self.t * 0.6) * (0.5 + inside) * (1.0 + 0.5 * self.surge);
                        EMBERS[((hot * 3.0).round() as usize).clamp(1, 3)]
                    } else if inside > 0.35 {
                        CHARCOAL
                    } else if side > 0.4 {
                        if ridge { BARK } else { BARK_LIGHT }
                    } else if side > -0.35 {
                        if ridge { BARK_DARK } else { BARK }
                    } else {
                        BARK_DARK
                    }
                };
                px[y * self.w + x] = c;
            }
        }
    }

    /// A shadow on the ground, centred at `c` with radii `rad`.
    fn draw_shadow(&self, px: &mut [Rgb], c: [f32; 2], rad: [f32; 2]) {
        let x0 = (c[0] - rad[0]).max(0.0) as usize;
        let x1 = ((c[0] + rad[0] + 1.0).max(0.0) as usize).min(self.w);
        let y0 = (c[1] - rad[1]).max(0.0) as usize;
        let y1 = ((c[1] + rad[1] + 1.0).max(0.0) as usize).min(self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = (x as f32 + 0.5 - c[0]) / rad[0];
                let dy = (y as f32 + 0.5 - c[1]) / rad[1];
                if dx * dx + dy * dy < 1.0 {
                    let i = y * self.w + x;
                    px[i] = scale(px[i], 0.6);
                }
            }
        }
    }

    /// An outlined stone centred at `c` with radii `rad`: lit on top and on
    /// the side facing the fire.
    fn draw_stone(&self, px: &mut [Rgb], l: &Layout, c: [f32; 2], rad: [f32; 2]) {
        let x0 = (c[0] - rad[0] - 1.0).max(0.0) as usize;
        let x1 = ((c[0] + rad[0] + 1.0).max(0.0) as usize).min(self.w);
        let y0 = (c[1] - rad[1] - 1.0).max(0.0) as usize;
        let y1 = ((c[1] + rad[1] + 1.0).max(0.0) as usize).min(self.h);
        let inside = |x: f32, y: f32| ((x + 0.5 - c[0]) / rad[0]).powi(2) + ((y + 0.5 - c[1]) / rad[1]).powi(2) <= 1.0;
        let outlined = rad[0].min(rad[1]) >= 1.5;
        for y in y0..y1 {
            for x in x0..x1 {
                let (fx, fy) = (x as f32, y as f32);
                if !inside(fx, fy) {
                    continue;
                }
                let edge = !(inside(fx - 1.0, fy) && inside(fx + 1.0, fy) && inside(fx, fy - 1.0) && inside(fx, fy + 1.0));
                let dx = (fx + 0.5 - c[0]) / rad[0];
                let dy = (fy + 0.5 - c[1]) / rad[1];
                let toward = if c[0] < l.cx { dx } else { -dx }; // 1 on the fire's side
                let lit = -dy * 0.6 + toward * 0.5;
                let tone = if lit > 0.3 { STONE_LIGHT } else if lit > -0.25 { STONE } else { STONE_DARK };
                px[y * self.w + x] = if outlined && edge {
                    OUTLINE
                } else if toward > 0.35 && lit > -0.25 {
                    mix(tone, WARM, 0.28)
                } else {
                    tone
                };
            }
        }
    }
}

/// The palette colour for this much heat, dithered between neighbours, or
/// None where there's no flame.
fn fire_color(heat: f32, x: usize, y: usize) -> Option<Rgb> {
    let heat = heat + (bayer(x, y) - 0.5) * 0.05;
    FIRE.iter().rev().find(|(from, _)| heat >= *from).map(|(_, c)| *c)
}

const BAYER: [[f32; 4]; 4] = [[0.0, 8.0, 2.0, 10.0], [12.0, 4.0, 14.0, 6.0], [3.0, 11.0, 1.0, 9.0], [15.0, 7.0, 13.0, 5.0]];

/// An ordered-dither threshold for a pixel, in (0, 1).
fn bayer(x: usize, y: usize) -> f32 {
    (BAYER[y % 4][x % 4] + 0.5) / 16.0
}

/// `v` (0 to 1) as one of `levels` + 1 steps, dithered between them.
fn dither(v: f32, levels: f32, x: usize, y: usize) -> f32 {
    ((v.clamp(0.0, 1.0) * levels + bayer(x, y)).floor() / levels).min(1.0)
}

/// `v` (0 to 1) as one of `levels` + 1 flat bands, dithered only in a narrow
/// strip where two bands meet, as pixel art draws light falling off.
fn banded(v: f32, levels: f32, x: usize, y: usize) -> f32 {
    let s = v.clamp(0.0, 1.0) * levels;
    let edge = ((s.fract() - 0.5) * 3.0 + 0.5).clamp(0.0, 1.0);
    ((s.floor() + if edge > bayer(x, y) { 1.0 } else { 0.0 }) / levels).min(1.0)
}

/// A repeatable random number in [0, 1) for a pair of integers.
fn hash(i: i32, j: i32) -> f32 {
    let mut n = (i as u32).wrapping_mul(0x9e37_79b1) ^ (j as u32).wrapping_mul(0x85eb_ca77);
    n = (n ^ (n >> 16)).wrapping_mul(0x7feb_352d);
    n ^= n >> 15;
    (n & 0xffff) as f32 / 65536.0
}

/// Smooth value noise in [0, 1): random at whole coordinates, eased between.
fn noise(x: f32, y: f32) -> f32 {
    let hash = |i: i32, j: i32| {
        let mut n = (i as u32).wrapping_mul(0x27d4_eb2d) ^ (j as u32).wrapping_mul(0x1656_67b1);
        n = (n ^ (n >> 15)).wrapping_mul(0x85eb_ca6b);
        n ^= n >> 13;
        (n & 0xffff) as f32 / 65536.0
    };
    let (x0, y0) = (x.floor(), y.floor());
    let (i, j) = (x0 as i32, y0 as i32);
    let ease = |t: f32| t * t * (3.0 - 2.0 * t);
    let (fx, fy) = (ease(x - x0), ease(y - y0));
    let top = hash(i, j) + (hash(i + 1, j) - hash(i, j)) * fx;
    let bottom = hash(i, j + 1) + (hash(i + 1, j + 1) - hash(i, j + 1)) * fx;
    top + (bottom - top) * fy
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn scale(c: Rgb, k: f32) -> Rgb {
    [c[0] * k, c[1] * k, c[2] * k]
}

fn rgb_of(c: Hsla) -> Rgb {
    let c = Rgba::from(c);
    [c.r, c.g, c.b]
}

// ---- drawing to the terminal ---------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
struct Cell {
    ch: char,
    fg: [u8; 3],
    bg: [u8; 3],
}

fn byte(c: Rgb) -> [u8; 3] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Two pixels to a cell: the top one in ▀, the bottom one behind it.
fn cells(px: &[Rgb], cols: usize, rows: usize) -> Vec<Cell> {
    let mut out = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for x in 0..cols {
            let top = byte(px.get(row * 2 * cols + x).copied().unwrap_or_default());
            let bottom = byte(px.get((row * 2 + 1) * cols + x).copied().unwrap_or_default());
            out.push(if top == bottom {
                Cell { ch: ' ', fg: top, bg: top }
            } else {
                Cell { ch: '▀', fg: top, bg: bottom }
            });
        }
    }
    out
}

/// The keys, on the bottom row for the first few seconds, then fading away.
fn hint(frame: &mut [Cell], cols: usize, rows: usize, elapsed: f32, color: Rgb) {
    let alpha = ((HINT_FOR - elapsed) / HINT_FADE).clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let text = [" q quit · ↑↓ stoke · ←→ wind · space poke ", " q quit · ↑↓ stoke ", " q quit "]
        .into_iter()
        .find(|t| t.chars().count() <= cols);
    let Some(text) = text else { return };
    let start = (rows - 1) * cols + (cols - text.chars().count()) / 2;
    for (cell, ch) in frame[start..].iter_mut().zip(text.chars()) {
        let bg = cell.bg;
        let behind = bg.map(|v| v as f32 / 255.0);
        *cell = Cell { ch, fg: byte(mix(behind, color, alpha)), bg };
    }
}

/// The escape codes that turn `prev` (or a blank screen) into `next`,
/// writing only the cells that changed.
fn encode(prev: Option<&[Cell]>, next: &[Cell], cols: usize) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let (mut fg, mut bg) = (None, None);
    let mut at = None;
    for (i, c) in next.iter().enumerate() {
        if prev.is_some_and(|p| p.get(i) == Some(c)) {
            continue;
        }
        let (row, col) = (i / cols, i % cols);
        if at != Some((row, col)) {
            let _ = write!(out, "\x1b[{};{}H", row + 1, col + 1);
        }
        if c.ch != ' ' && fg != Some(c.fg) {
            let [r, g, b] = c.fg;
            let _ = write!(out, "\x1b[38;2;{r};{g};{b}m");
            fg = Some(c.fg);
        }
        if bg != Some(c.bg) {
            let [r, g, b] = c.bg;
            let _ = write!(out, "\x1b[48;2;{r};{g};{b}m");
            bg = Some(c.bg);
        }
        out.push(c.ch);
        // At the last column the cursor stays put (autowrap is off).
        at = (col + 1 < cols).then_some((row, col + 1));
    }
    out
}

fn window_size() -> Option<(u16, u16)> {
    // SAFETY: TIOCGWINSZ fills the winsize we own.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        (libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 && ws.ws_row > 0)
            .then_some((ws.ws_col, ws.ws_row))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOK: Look = Look { bg: [0.1, 0.1, 0.12], hint: [0.6, 0.6, 0.6], dark: true };

    #[test]
    fn keys() {
        assert_eq!(parse_keys(b"q"), vec![Key::Quit]);
        assert_eq!(parse_keys(b"\x1b"), vec![Key::Quit]);
        assert_eq!(parse_keys(b"\x1b[A\x1b[B\x1bOC\x1b[D"), vec![Key::Up, Key::Down, Key::Right, Key::Left]);
        assert_eq!(parse_keys(b"+- x"), vec![Key::Up, Key::Down, Key::Poke]);
        // Alt+key isn't Esc.
        assert_eq!(parse_keys(b"\x1bx"), vec![]);
    }

    #[test]
    fn burns_at_any_size() {
        for (w, h) in [(1, 2), (3, 2), (10, 6), (80, 48), (240, 140), (30, 120)] {
            let mut fire = Fire::new(w, h, 7, LOOK);
            fire.poke();
            for _ in 0..60 {
                fire.step(0.033);
            }
            let px = fire.pixels();
            assert_eq!(px.len(), w * h);
            assert!(px.iter().flatten().all(|v| v.is_finite()));
            let mut frame = cells(&px, w, h / 2);
            hint(&mut frame, w, h / 2, 0.0, LOOK.hint);
            encode(None, &frame, w);
        }
    }

    #[test]
    fn flames_rise_from_the_bed() {
        let mut fire = Fire::new(80, 48, 3, LOOK);
        for _ in 0..90 {
            fire.step(0.033);
        }
        let l = fire.layout();
        let row = |y: usize| fire.heat[y * 80..(y + 1) * 80].iter().sum::<f32>();
        assert!(row(l.base - 4) > 1.0, "flames near the bed");
        assert!(row(0) < row(l.base - 4) * 0.05, "and little at the top of the pane");
    }

    #[test]
    fn colors_get_hotter() {
        let lum = |c: Rgb| c[0] + c[1] + c[2];
        assert!(FIRE.windows(2).all(|p| p[1].0 > p[0].0 && lum(p[1].1) > lum(p[0].1)));
        assert!(EMBERS.windows(2).all(|p| lum(p[1]) > lum(p[0])));
        assert_eq!(fire_color(0.0, 0, 0), None);
        assert_eq!(fire_color(1.0, 0, 0), Some(FIRE[6].1));
    }

    #[test]
    fn dither_spreads_between_steps() {
        let mean = |v: f32| (0..16).map(|i| dither(v, 3.0, i % 4, i / 4)).sum::<f32>() / 16.0;
        assert!((mean(0.5) - 0.5).abs() < 0.05);
        assert_eq!(mean(0.0), 0.0);
        assert_eq!(mean(1.0), 1.0);
    }

    #[test]
    fn only_changes_are_written() {
        let a = vec![Cell { ch: ' ', fg: [0; 3], bg: [0; 3] }; 6];
        assert_eq!(encode(Some(&a), &a, 3), "");
        let mut b = a.clone();
        b[4] = Cell { ch: '▀', fg: [255, 0, 0], bg: [0; 3] };
        assert_eq!(encode(Some(&a), &b, 3), "\x1b[2;2H\x1b[38;2;255;0;0m\x1b[48;2;0;0;0m▀");
    }
}
