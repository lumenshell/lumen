//! A key Lumen catches from any app (X11), for the quick terminal: parsing
//! `quick-terminal-key` and grabbing it on the root window.

/// X11 modifier masks.
const SHIFT: u16 = 1;
const LOCK: u16 = 2;
const CONTROL: u16 = 4;
const ALT: u16 = 8;
const NUM_LOCK: u16 = 16;
const SUPER: u16 = 64;

/// A key: its X11 modifier mask and keysym.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hotkey {
    pub modifiers: u16,
    pub keysym: u32,
}

/// `quick-terminal-key` as a key: modifiers (ctrl, alt, shift, super) and a
/// key, joined by `-`, like "f12", "ctrl-grave" or "super-space". Empty is
/// none. The error says what it should be.
pub fn parse(text: &str) -> Result<Option<Hotkey>, String> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return Ok(None);
    }
    let should = || format!("a key like \"f12\", \"ctrl-grave\" or \"super-space\" (not \"{text}\")");
    // The key is last; "ctrl--" means Ctrl and minus.
    let (mods, key) = match text.strip_suffix("--") {
        Some(rest) => (rest, "-"),
        None => text.rsplit_once('-').map_or(("", text.as_str()), |(m, k)| (m, k)),
    };
    let mut modifiers = 0;
    for m in mods.split('-').filter(|m| !m.is_empty()) {
        modifiers |= match m {
            "ctrl" | "control" => CONTROL,
            "alt" => ALT,
            "shift" => SHIFT,
            "super" | "cmd" | "win" | "meta" => SUPER,
            _ => return Err(should()),
        };
    }
    let keysym = keysym(key).ok_or_else(should)?;
    Ok(Some(Hotkey { modifiers, keysym }))
}

/// The X11 keysym for a key name.
fn keysym(key: &str) -> Option<u32> {
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && (c.is_ascii_graphic())
    {
        return Some(c.to_ascii_lowercase() as u32);
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=35).contains(&n).then_some(0xffbe + n - 1);
    }
    Some(match key {
        "space" => 0x20,
        "grave" | "backtick" => 0x60,
        "minus" => 0x2d,
        "equal" => 0x3d,
        "tab" => 0xff09,
        "enter" | "return" => 0xff0d,
        "escape" | "esc" => 0xff1b,
        "backspace" => 0xff08,
        "insert" => 0xff63,
        "delete" => 0xffff,
        "home" => 0xff50,
        "end" => 0xff57,
        "pageup" => 0xff55,
        "pagedown" => 0xff56,
        "up" => 0xff52,
        "down" => 0xff54,
        "left" => 0xff51,
        "right" => 0xff53,
        "pause" => 0xff13,
        "scrolllock" => 0xff14,
        "printscreen" | "print" => 0xff61,
        "menu" => 0xff67,
        _ => return None,
    })
}

/// Grab `key` from every app until the returned guard is dropped, calling
/// `pressed` (on another thread) each time it's pressed. Errors say why, such
/// as another app holding the key.
#[cfg(target_os = "linux")]
pub fn grab(key: Hotkey, pressed: impl Fn() + Send + 'static) -> Result<Grab, String> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{ConnectionExt, GrabMode, ModMask};

    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return Err("Lumen can't catch keys from other apps on Wayland; bind `lumen msg quick-terminal` to a key in your desktop's settings".into());
    }
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("can't reach the X server: {e}"))?;
    let root = conn.setup().roots[screen].root;
    let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
    let map = conn
        .get_keyboard_mapping(min, max - min + 1)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let per = map.keysyms_per_keycode as usize;
    let keycode = map
        .keysyms
        .chunks(per.max(1))
        .position(|syms| syms.contains(&key.keysym))
        .map(|i| min + i as u8)
        .ok_or("that key isn't on this keyboard")?;
    // Grab it with Caps Lock and Num Lock on or off, so they don't matter.
    for extra in [0, LOCK, NUM_LOCK, LOCK | NUM_LOCK] {
        let mods = ModMask::from(key.modifiers | extra);
        conn.grab_key(false, root, mods, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|_| "another app already uses that key; pick another".to_string())?;
    }
    conn.flush().map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::Builder::new()
        .name("lumen-hotkey".into())
        .spawn(move || {
            // Polled, so dropping the guard ends it promptly (and closing the
            // connection releases the grab).
            while !stopped.load(Ordering::Relaxed) {
                match conn.poll_for_event() {
                    Ok(Some(Event::KeyPress(ev))) if ev.detail == keycode => pressed(),
                    Ok(Some(_)) => continue,
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(15)),
                    Err(_) => break,
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Grab(stop))
}

#[cfg(not(target_os = "linux"))]
pub fn grab(_: Hotkey, _: impl Fn() + Send + 'static) -> Result<Grab, String> {
    Err("Lumen can only catch keys from other apps on Linux (X11) so far; bind `lumen msg quick-terminal` to a key in your system's settings".into())
}

/// A grabbed key; dropping it releases the key.
pub struct Grab(#[cfg(target_os = "linux")] std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for Grab {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_parse_like_the_keys_table() {
        assert_eq!(parse(""), Ok(None));
        assert_eq!(parse("F12"), Ok(Some(Hotkey { modifiers: 0, keysym: 0xffc9 })));
        assert_eq!(parse("ctrl-grave"), Ok(Some(Hotkey { modifiers: CONTROL, keysym: 0x60 })));
        assert_eq!(parse("ctrl-`"), Ok(Some(Hotkey { modifiers: CONTROL, keysym: 0x60 })));
        assert_eq!(parse("super-space"), Ok(Some(Hotkey { modifiers: SUPER, keysym: 0x20 })));
        assert_eq!(parse("ctrl-alt-t"), Ok(Some(Hotkey { modifiers: CONTROL | ALT, keysym: 0x74 })));
        assert_eq!(parse("ctrl--"), Ok(Some(Hotkey { modifiers: CONTROL, keysym: 0x2d })));
        assert!(parse("hyper-x").is_err());
        assert!(parse("f99").is_err());
        assert!(parse("ctrl-nosuchkey").unwrap_err().contains("ctrl-nosuchkey"));
    }
}
