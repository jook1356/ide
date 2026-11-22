//! xterm mouse reporting: what a TUI that turned it on (DECSET 1000 / 1002 / 1003, encoding
//! 1006 SGR or 1005 UTF-8) receives for clicks, drags, motion and the wheel. This is the
//! standard protocol every terminal speaks (Windows Terminal, iTerm2, VS Code's xterm.js);
//! Claude Code's and codex's clickable UIs use it.

use alacritty_terminal::term::TermMode;
use iced::keyboard::Modifiers;

/// The button of a report (xterm's low bits; the wheel is 64 + direction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    /// Motion without a button held (any-motion mode, 1003).
    None,
    WheelUp,
    WheelDown,
}

impl Button {
    fn code(self) -> u32 {
        match self {
            Button::Left => 0,
            Button::Middle => 1,
            Button::Right => 2,
            Button::None => 3,
            Button::WheelUp => 64,
            Button::WheelDown => 65,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    Motion,
}

/// Whether the program asked for mouse reports. Shift held bypasses them (local selection),
/// as in xterm and Windows Terminal.
pub fn reporting(mode: TermMode, mods: Modifiers) -> bool {
    mode.intersects(TermMode::MOUSE_MODE) && !mods.shift()
}

/// Whether `action` is reported in `mode` (clicks always; motion only with a button held in
/// drag mode, any motion in motion mode).
pub fn wanted(mode: TermMode, action: Action, button: Button) -> bool {
    match action {
        Action::Press | Action::Release => mode.intersects(TermMode::MOUSE_MODE),
        Action::Motion => {
            mode.contains(TermMode::MOUSE_MOTION)
                || (mode.contains(TermMode::MOUSE_DRAG) && button != Button::None)
        }
    }
}

/// The bytes of one report at the 0-based cell (`col`, `row`) of the viewport, or `None` when
/// the legacy encoding cannot express the cell.
pub fn encode(
    mode: TermMode,
    action: Action,
    button: Button,
    col: usize,
    row: usize,
    mods: Modifiers,
) -> Option<Vec<u8>> {
    let mut code = button.code();
    if action == Action::Motion {
        code += 32;
    }
    if mods.shift() {
        code += 4;
    }
    if mods.alt() {
        code += 8;
    }
    if mods.control() {
        code += 16;
    }
    let (x, y) = (col + 1, row + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if action == Action::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }
    // Legacy encodings report a release as button 3 (which one is not said).
    if action == Action::Release && !matches!(button, Button::WheelUp | Button::WheelDown) {
        code = (code & !3) | 3;
    }
    let mut out = b"\x1b[M".to_vec();
    if mode.contains(TermMode::UTF8_MOUSE) {
        for v in [32 + code, 32 + x as u32, 32 + y as u32] {
            let c = char::from_u32(v).filter(|_| v < 2048)?;
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    } else {
        for v in [32 + code, 32 + x as u32, 32 + y as u32] {
            out.push(u8::try_from(v).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLICK: TermMode = TermMode::MOUSE_REPORT_CLICK;

    #[test]
    fn sgr_press_release_and_modifiers() {
        let m = CLICK | TermMode::SGR_MOUSE;
        let none = Modifiers::empty();
        assert_eq!(encode(m, Action::Press, Button::Left, 4, 2, none).unwrap(), b"\x1b[<0;5;3M");
        assert_eq!(encode(m, Action::Release, Button::Left, 4, 2, none).unwrap(), b"\x1b[<0;5;3m");
        assert_eq!(encode(m, Action::Press, Button::Right, 0, 0, Modifiers::CTRL).unwrap(), b"\x1b[<18;1;1M");
        assert_eq!(encode(m, Action::Press, Button::WheelDown, 9, 9, none).unwrap(), b"\x1b[<65;10;10M");
        assert_eq!(encode(m, Action::Motion, Button::Left, 1, 1, none).unwrap(), b"\x1b[<32;2;2M");
    }

    #[test]
    fn legacy_encoding() {
        let none = Modifiers::empty();
        assert_eq!(encode(CLICK, Action::Press, Button::Left, 0, 0, none).unwrap(), b"\x1b[M !!");
        assert_eq!(encode(CLICK, Action::Release, Button::Right, 0, 0, none).unwrap(), b"\x1b[M#!!");
        assert!(encode(CLICK, Action::Press, Button::Left, 300, 0, none).is_none());
        let utf8 = CLICK | TermMode::UTF8_MOUSE;
        assert_eq!(
            encode(utf8, Action::Press, Button::Left, 300, 0, none).unwrap(),
            "\x1b[M \u{14d}!".as_bytes()
        );
    }

    #[test]
    fn what_is_reported() {
        let drag = CLICK | TermMode::MOUSE_DRAG;
        assert!(wanted(drag, Action::Motion, Button::Left));
        assert!(!wanted(drag, Action::Motion, Button::None));
        assert!(wanted(TermMode::MOUSE_MOTION, Action::Motion, Button::None));
        assert!(!wanted(TermMode::empty(), Action::Press, Button::Left));
        assert!(reporting(CLICK, Modifiers::empty()));
        assert!(!reporting(CLICK, Modifiers::SHIFT));
    }
}
