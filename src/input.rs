//! Translate GPUI keystrokes into the byte sequences a shell expects:
//! legacy xterm sequences, or the kitty keyboard protocol
//! (<https://sw.kovidgoyal.net/kitty/keyboard-protocol/>) when a program
//! turns it on.

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

/// Whether a key went down, auto-repeated or came up.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum KeyEvent {
    Press,
    Repeat,
    Release,
}

/// Bytes to send for a key event, or `None` to send nothing.
pub fn encode(ks: &Keystroke, mode: TermMode, event: KeyEvent) -> Option<Vec<u8>> {
    if mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL) {
        return kitty(ks, mode, event);
    }
    match event {
        KeyEvent::Release => None,
        _ => to_esc(ks, mode),
    }
}

/// How the kitty protocol names a key: a functional key with its own number
/// and final byte, or a key that types text, by its unshifted character.
enum KittyKey {
    Functional { number: u32, last: char },
    Text(char),
}

fn kitty_key(key: &str) -> Option<KittyKey> {
    use KittyKey::*;
    let f = |number, last| Some(Functional { number, last });
    match key {
        "escape" => f(27, 'u'),
        "enter" => f(13, 'u'),
        "tab" => f(9, 'u'),
        "backspace" => f(127, 'u'),
        "insert" => f(2, '~'),
        "delete" => f(3, '~'),
        "pageup" => f(5, '~'),
        "pagedown" => f(6, '~'),
        "up" => f(1, 'A'),
        "down" => f(1, 'B'),
        "right" => f(1, 'C'),
        "left" => f(1, 'D'),
        "home" => f(1, 'H'),
        "end" => f(1, 'F'),
        "f1" => f(1, 'P'),
        "f2" => f(1, 'Q'),
        "f3" => f(13, '~'),
        "f4" => f(1, 'S'),
        "f5" => f(15, '~'),
        "f6" => f(17, '~'),
        "f7" => f(18, '~'),
        "f8" => f(19, '~'),
        "f9" => f(20, '~'),
        "f10" => f(21, '~'),
        "f11" => f(23, '~'),
        "f12" => f(24, '~'),
        "space" => Some(Text(' ')),
        k => {
            let mut chars = k.chars();
            let c = chars.next()?;
            chars.next().is_none().then(|| Text(c.to_lowercase().next().unwrap_or(c)))
        }
    }
}

/// The kitty keyboard protocol, for whichever of its flags the program
/// pushed: disambiguate, report event types, report alternate keys, report
/// all keys as escape codes, report associated text. Keys that only modify
/// (Shift on its own and so on) aren't reported.
fn kitty(ks: &Keystroke, mode: TermMode, event: KeyEvent) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    if m.platform {
        return None;
    }
    let all_keys = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let event_types = mode.contains(TermMode::REPORT_EVENT_TYPES);
    let key = kitty_key(&ks.key)?;
    let mods = 1 + m.shift as u32 + 2 * m.alt as u32 + 4 * m.control as u32;
    let text = ks.key_char.as_deref().filter(|t| !t.is_empty() && !t.chars().any(char::is_control));
    let legacy_safe = matches!(key, KittyKey::Functional { number: 13 | 9 | 127, last: 'u' });

    if event == KeyEvent::Release {
        // Releases are only reported when asked for; Enter, Tab and Backspace
        // only with every key as an escape code, so a shell left in this mode
        // still works.
        if !event_types || (legacy_safe && !all_keys) {
            return None;
        }
    } else if !all_keys {
        // Without "all keys as escape codes", keys that type text (bare or
        // with Shift) send that text, and Enter, Tab and Backspace without
        // modifiers stay legacy bytes.
        let plain = !m.control && !m.alt;
        match key {
            KittyKey::Text(_) if plain => return text.map(|t| t.as_bytes().to_vec()),
            KittyKey::Functional { .. } if legacy_safe && mods == 1 => return to_esc(ks, mode),
            // Unmodified cursor and function keys keep their legacy form
            // (which respects application cursor mode) unless events are reported.
            KittyKey::Functional { last, .. } if last != 'u' && mods == 1 && !event_types => {
                return to_esc(ks, mode);
            }
            _ => {}
        }
    }

    let event_suffix = match (event_types, event) {
        (true, KeyEvent::Repeat) => ":2",
        (true, KeyEvent::Release) => ":3",
        _ => "",
    };
    let modifiers = if mods == 1 && event_suffix.is_empty() {
        String::new()
    } else {
        format!("{mods}{event_suffix}")
    };
    let out = match key {
        KittyKey::Functional { number: _, last } if last != 'u' && last != '~' => {
            // Letter-terminated keys: CSI 1 ; mods X, or CSI X when plain.
            if modifiers.is_empty() { format!("\x1b[{last}") } else { format!("\x1b[1;{modifiers}{last}") }
        }
        KittyKey::Functional { number, last } => {
            if modifiers.is_empty() { format!("\x1b[{number}{last}") } else { format!("\x1b[{number};{modifiers}{last}") }
        }
        KittyKey::Text(base) => {
            let mut code = (base as u32).to_string();
            if mode.contains(TermMode::REPORT_ALTERNATE_KEYS) && m.shift {
                let shifted = text.and_then(|t| {
                    let mut c = t.chars();
                    let first = c.next()?;
                    (c.next().is_none() && first != base).then_some(first)
                });
                if let Some(shifted) = shifted {
                    code.push_str(&format!(":{}", shifted as u32));
                }
            }
            let associated = (all_keys
                && mode.contains(TermMode::REPORT_ASSOCIATED_TEXT)
                && event != KeyEvent::Release
                && !m.control)
                .then_some(text)
                .flatten()
                .map(|t| t.chars().map(|c| (c as u32).to_string()).collect::<Vec<_>>().join(":"));
            match (associated, modifiers.is_empty()) {
                (Some(t), true) => format!("\x1b[{code};;{t}u"),
                (Some(t), false) => format!("\x1b[{code};{modifiers};{t}u"),
                (None, true) => format!("\x1b[{code}u"),
                (None, false) => format!("\x1b[{code};{modifiers}u"),
            }
        }
    };
    Some(out.into_bytes())
}

pub fn to_esc(ks: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    // Platform key (cmd on macOS, super on Linux) is reserved for app shortcuts.
    if m.platform {
        return None;
    }
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl
    let modp = 1 + (m.shift as u8) + 2 * (m.alt as u8) + 4 * (m.control as u8);

    let csi_letter = |letter: char| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[1;{modp}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let csi_tilde = |n: u8| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[{n};{modp}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let ss3_or_csi = |letter: char| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[1;{modp}{letter}").into_bytes()
        } else {
            format!("\x1bO{letter}").into_bytes()
        }
    };

    let bytes = match ks.key.as_str() {
        "up" => csi_letter('A'),
        "down" => csi_letter('B'),
        "right" => csi_letter('C'),
        "left" => csi_letter('D'),
        "home" => csi_letter('H'),
        "end" => csi_letter('F'),
        "pageup" => csi_tilde(5),
        "pagedown" => csi_tilde(6),
        "insert" => csi_tilde(2),
        "delete" => csi_tilde(3),
        "f1" => ss3_or_csi('P'),
        "f2" => ss3_or_csi('Q'),
        "f3" => ss3_or_csi('R'),
        "f4" => ss3_or_csi('S'),
        "f5" => csi_tilde(15),
        "f6" => csi_tilde(17),
        "f7" => csi_tilde(18),
        "f8" => csi_tilde(19),
        "f9" => csi_tilde(20),
        "f10" => csi_tilde(21),
        "f11" => csi_tilde(23),
        "f12" => csi_tilde(24),
        "enter" => {
            if m.alt {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        "tab" => {
            if m.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        "backspace" => {
            if m.control {
                vec![0x08]
            } else if m.alt {
                b"\x1b\x7f".to_vec()
            } else {
                vec![0x7f]
            }
        }
        "escape" => vec![0x1b],
        "space" if m.control => vec![0],
        key => {
            if m.control {
                let ch = key.chars().next()?;
                if key.chars().count() != 1 {
                    return None;
                }
                let code = match ch.to_ascii_lowercase() {
                    c @ 'a'..='z' => c as u8 - b'a' + 1,
                    '@' | '2' => 0,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '-' | '7' => 0x1f,
                    '8' | '?' => 0x7f,
                    _ => return None,
                };
                let mut v = Vec::new();
                if m.alt {
                    v.push(0x1b);
                }
                v.push(code);
                v
            } else {
                let mut text = ks.key_char.clone().or_else(|| {
                    (key == "space").then(|| " ".to_string())
                })?;
                // Guard against a stale keyboard state reporting an unshifted letter.
                if m.shift && text.len() == 1 && text.chars().all(|c| c.is_ascii_lowercase()) {
                    text = text.to_ascii_uppercase();
                }
                if text.is_empty() {
                    return None;
                }
                let mut v = Vec::new();
                if m.alt && !cfg!(target_os = "macos") {
                    v.push(0x1b);
                    // On Linux key_char may already be the plain char; use the key.
                    if key.chars().count() == 1 {
                        let k = if m.shift { key.to_uppercase() } else { key.to_string() };
                        v.extend_from_slice(k.as_bytes());
                        return Some(v);
                    }
                }
                v.extend_from_slice(text.as_bytes());
                v
            }
        }
    };
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::TermMode as T;

    fn key(spec: &str, key_char: Option<&str>) -> Keystroke {
        let mut ks = Keystroke::parse(spec).unwrap();
        ks.key_char = key_char.map(String::from);
        ks
    }

    fn send(spec: &str, key_char: Option<&str>, mode: TermMode, event: KeyEvent) -> Option<String> {
        encode(&key(spec, key_char), mode, event).map(|b| String::from_utf8(b).unwrap())
    }

    fn press(spec: &str, key_char: Option<&str>, mode: TermMode) -> Option<String> {
        send(spec, key_char, mode, KeyEvent::Press)
    }

    #[test]
    fn legacy_until_a_program_asks() {
        assert_eq!(press("ctrl-i", None, T::empty()).as_deref(), Some("\t"));
        assert_eq!(press("escape", None, T::empty()).as_deref(), Some("\x1b"));
        assert_eq!(send("a", Some("a"), T::empty(), KeyEvent::Release), None);
    }

    #[test]
    fn disambiguate_separates_lookalike_keys() {
        let d = T::DISAMBIGUATE_ESC_CODES;
        assert_eq!(press("ctrl-i", None, d).as_deref(), Some("\x1b[105;5u"));
        assert_eq!(press("tab", None, d).as_deref(), Some("\t"));
        assert_eq!(press("escape", None, d).as_deref(), Some("\x1b[27u"));
        assert_eq!(press("ctrl-[", None, d).as_deref(), Some("\x1b[91;5u"));
        assert_eq!(press("shift-enter", None, d).as_deref(), Some("\x1b[13;2u"));
        assert_eq!(press("enter", None, d).as_deref(), Some("\r"));
        assert_eq!(press("alt-x", Some("x"), d).as_deref(), Some("\x1b[120;3u"));
        assert_eq!(press("shift-a", Some("A"), d).as_deref(), Some("A"));
        assert_eq!(press("a", Some("a"), d).as_deref(), Some("a"));
        assert_eq!(press("up", None, d).as_deref(), Some("\x1b[A"));
        assert_eq!(press("ctrl-up", None, d).as_deref(), Some("\x1b[1;5A"));
        assert_eq!(press("shift-delete", None, d).as_deref(), Some("\x1b[3;2~"));
        assert_eq!(press("ctrl-f3", None, d).as_deref(), Some("\x1b[13;5~"));
        assert_eq!(press("ctrl-space", None, d).as_deref(), Some("\x1b[32;5u"));
    }

    #[test]
    fn event_types_report_repeats_and_releases() {
        let m = T::DISAMBIGUATE_ESC_CODES | T::REPORT_EVENT_TYPES;
        assert_eq!(send("ctrl-a", None, m, KeyEvent::Repeat).as_deref(), Some("\x1b[97;5:2u"));
        assert_eq!(send("ctrl-a", None, m, KeyEvent::Release).as_deref(), Some("\x1b[97;5:3u"));
        assert_eq!(send("a", Some("a"), m, KeyEvent::Release).as_deref(), Some("\x1b[97;1:3u"));
        assert_eq!(send("up", None, m, KeyEvent::Press).as_deref(), Some("\x1b[A"));
        assert_eq!(send("up", None, m, KeyEvent::Release).as_deref(), Some("\x1b[1;1:3A"));
        // Enter's release only with every key as an escape code.
        assert_eq!(send("enter", None, m, KeyEvent::Release), None);
        assert_eq!(send("enter", None, m | T::REPORT_ALL_KEYS_AS_ESC, KeyEvent::Release).as_deref(), Some("\x1b[13;1:3u"));
    }

    #[test]
    fn all_keys_alternates_and_text() {
        let all = T::DISAMBIGUATE_ESC_CODES | T::REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(press("a", Some("a"), all).as_deref(), Some("\x1b[97u"));
        assert_eq!(press("enter", None, all).as_deref(), Some("\x1b[13u"));
        assert_eq!(press("shift-a", Some("A"), all).as_deref(), Some("\x1b[97;2u"));
        let alt = all | T::REPORT_ALTERNATE_KEYS;
        assert_eq!(press("shift-a", Some("A"), alt).as_deref(), Some("\x1b[97:65;2u"));
        let text = all | T::REPORT_ASSOCIATED_TEXT;
        assert_eq!(press("shift-a", Some("A"), text).as_deref(), Some("\x1b[97;2;65u"));
        assert_eq!(press("a", Some("a"), text).as_deref(), Some("\x1b[97;;97u"));
    }
}
