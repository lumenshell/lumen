//! Encoding mouse events for programs that turn on mouse reporting (vim, tmux,
//! htop, ...): X10/normal, UTF-8 (1005) and SGR (1006) formats.

use alacritty_terminal::term::TermMode;
use gpui::Modifiers;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held (any-event tracking, mode 1003).
    None,
}

impl Button {
    pub fn from_gpui(b: gpui::MouseButton) -> Option<Self> {
        match b {
            gpui::MouseButton::Left => Some(Self::Left),
            gpui::MouseButton::Middle => Some(Self::Middle),
            gpui::MouseButton::Right => Some(Self::Right),
            _ => None,
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
            Self::None => 3,
            Self::WheelUp => 64,
            Self::WheelDown => 65,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Press,
    Release,
    Motion,
}

/// Is the program asking for mouse events at all?
pub fn reporting(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

/// Should this motion be reported, given whether a button is held?
pub fn wants_motion(mode: TermMode, button_held: bool) -> bool {
    mode.contains(TermMode::MOUSE_MOTION) || (button_held && mode.contains(TermMode::MOUSE_DRAG))
}

/// Bytes to send for a mouse event at a 0-based viewport cell, or None if the
/// position can't be encoded in the active format.
pub fn encode(
    mode: TermMode,
    button: Button,
    action: Action,
    mods: &Modifiers,
    col: usize,
    row: usize,
) -> Option<Vec<u8>> {
    let sgr = mode.contains(TermMode::SGR_MOUSE);
    // Legacy formats can't say which button was released.
    let mut code = if action == Action::Release && !sgr { 3 } else { button.code() };
    if action == Action::Motion {
        code += 32;
    }
    code += 4 * mods.shift as u8 + 8 * mods.alt as u8 + 16 * mods.control as u8;

    if sgr {
        let end = if action == Action::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes());
    }

    let mut out = b"\x1b[M".to_vec();
    out.push(32 + code);
    if mode.contains(TermMode::UTF8_MOUSE) {
        // Coordinates are 32 + 1-based position, written as UTF-8 above 127.
        for v in [col, row] {
            let c = char::from_u32(32 + 1 + v as u32).filter(|_| v < 2015)?;
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    } else {
        // One byte each: positions past 223 can't be expressed.
        for v in [col, row] {
            out.push(u8::try_from(32 + 1 + v).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers {
        control: false,
        alt: false,
        shift: false,
        platform: false,
        function: false,
    };

    fn click() -> TermMode {
        TermMode::MOUSE_REPORT_CLICK
    }

    #[test]
    fn sgr_press_release_and_drag() {
        let m = click() | TermMode::SGR_MOUSE;
        assert_eq!(encode(m, Button::Left, Action::Press, &NONE, 4, 9).unwrap(), b"\x1b[<0;5;10M");
        assert_eq!(encode(m, Button::Right, Action::Release, &NONE, 0, 0).unwrap(), b"\x1b[<2;1;1m");
        assert_eq!(encode(m, Button::Left, Action::Motion, &NONE, 1, 1).unwrap(), b"\x1b[<32;2;2M");
        // SGR has no coordinate limit.
        assert_eq!(encode(m, Button::Left, Action::Press, &NONE, 300, 0).unwrap(), b"\x1b[<0;301;1M");
    }

    #[test]
    fn modifiers_add_to_the_button_code() {
        let m = click() | TermMode::SGR_MOUSE;
        let ctrl_shift = Modifiers { control: true, shift: true, ..NONE };
        assert_eq!(encode(m, Button::Left, Action::Press, &ctrl_shift, 0, 0).unwrap(), b"\x1b[<20;1;1M");
        let alt = Modifiers { alt: true, ..NONE };
        assert_eq!(encode(m, Button::WheelDown, Action::Press, &alt, 0, 0).unwrap(), b"\x1b[<73;1;1M");
    }

    #[test]
    fn normal_format() {
        assert_eq!(encode(click(), Button::Left, Action::Press, &NONE, 0, 0).unwrap(), b"\x1b[M !!");
        // Release is always button 3 in the legacy format.
        assert_eq!(encode(click(), Button::Middle, Action::Release, &NONE, 2, 3).unwrap(), b"\x1b[M##$");
        assert_eq!(encode(click(), Button::Left, Action::Press, &NONE, 222, 0).unwrap()[4], 255);
        assert!(encode(click(), Button::Left, Action::Press, &NONE, 223, 0).is_none());
    }

    #[test]
    fn utf8_format_extends_the_range() {
        let m = click() | TermMode::UTF8_MOUSE;
        let out = encode(m, Button::Left, Action::Press, &NONE, 300, 0).unwrap();
        assert_eq!(&out[..4], b"\x1b[M ");
        assert_eq!(std::str::from_utf8(&out[4..]).unwrap(), "\u{14d}!");
    }

    #[test]
    fn motion_filtering() {
        assert!(!wants_motion(click(), true));
        assert!(wants_motion(TermMode::MOUSE_DRAG, true));
        assert!(!wants_motion(TermMode::MOUSE_DRAG, false));
        assert!(wants_motion(TermMode::MOUSE_MOTION, false));
    }
}
