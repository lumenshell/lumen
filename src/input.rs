//! Translate GPUI keystrokes into the byte sequences a shell expects.

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

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
