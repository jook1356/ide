//! Keyboard -> VT byte encoding (xterm conventions).
use iced::keyboard::{self, Key, key::Named};

/// Keyboard -> VT bytes (xterm conventions).
pub fn key_bytes(key: &Key, mods: keyboard::Modifiers, text: Option<&str>, app_cursor: bool) -> Option<Vec<u8>> {
    let m = 1 + mods.shift() as u8 + 2 * mods.alt() as u8 + 4 * mods.control() as u8;
    let csi = |c: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{c}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 { format!("\x1b[{n};{m}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    if let Key::Named(n) = key {
        return Some(match n {
            Named::Enter => b"\r".to_vec(),
            Named::Backspace => if mods.control() { b"\x08".to_vec() } else { b"\x7f".to_vec() },
            Named::Tab => if mods.shift() { b"\x1b[Z".to_vec() } else { b"\t".to_vec() },
            Named::Escape => b"\x1b".to_vec(),
            Named::Space => if mods.control() { vec![0] } else { b" ".to_vec() },
            Named::ArrowUp => csi('A'),
            Named::ArrowDown => csi('B'),
            Named::ArrowRight => csi('C'),
            Named::ArrowLeft => csi('D'),
            Named::Home => csi('H'),
            Named::End => csi('F'),
            Named::Insert => tilde(2),
            Named::Delete => tilde(3),
            Named::PageUp => tilde(5),
            Named::PageDown => tilde(6),
            Named::F1 => b"\x1bOP".to_vec(),
            Named::F2 => b"\x1bOQ".to_vec(),
            Named::F3 => b"\x1bOR".to_vec(),
            Named::F4 => b"\x1bOS".to_vec(),
            Named::F5 => tilde(15),
            Named::F6 => tilde(17),
            Named::F7 => tilde(18),
            Named::F8 => tilde(19),
            Named::F9 => tilde(20),
            Named::F10 => tilde(21),
            Named::F11 => tilde(23),
            Named::F12 => tilde(24),
            _ => return None,
        });
    }
    if let Key::Character(c) = key
        && mods.control()
    {
        let ch = c.chars().next()?.to_ascii_lowercase();
        let b = match ch {
            'a'..='z' => ch as u8 - b'a' + 1,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '7' => 0x1f,
            '@' | '2' => 0,
            _ => return None,
        };
        return Some(if mods.alt() { vec![0x1b, b] } else { vec![b] });
    }
    let t = text?;
    if t.is_empty() || t.chars().all(|c| c.is_control()) {
        return None;
    }
    let mut v = Vec::new();
    if mods.alt() {
        v.push(0x1b);
    }
    v.extend_from_slice(t.as_bytes());
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard::Modifiers as M;

    fn named(n: Named, m: M) -> Option<Vec<u8>> {
        key_bytes(&Key::Named(n), m, None, false)
    }

    #[test]
    fn named_keys() {
        assert_eq!(named(Named::Enter, M::empty()), Some(b"\r".to_vec()));
        assert_eq!(named(Named::ArrowUp, M::empty()), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes(&Key::Named(Named::ArrowUp), M::empty(), None, true), Some(b"\x1bOA".to_vec()));
        assert_eq!(named(Named::ArrowLeft, M::CTRL), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(named(Named::Home, M::empty()), Some(b"\x1b[H".to_vec()));
        assert_eq!(named(Named::PageDown, M::empty()), Some(b"\x1b[6~".to_vec()));
        assert_eq!(named(Named::F1, M::empty()), Some(b"\x1bOP".to_vec()));
        assert_eq!(named(Named::F5, M::SHIFT), Some(b"\x1b[15;2~".to_vec()));
        assert_eq!(named(Named::Tab, M::SHIFT), Some(b"\x1b[Z".to_vec()));
        assert_eq!(named(Named::Backspace, M::empty()), Some(vec![0x7f]));
    }

    #[test]
    fn ctrl_alt_and_text() {
        let c = |s: &str, m: M, t: Option<&str>| key_bytes(&Key::Character(s.into()), m, t, false);
        assert_eq!(c("c", M::CTRL, None), Some(vec![3]));
        assert_eq!(c("A", M::CTRL, None), Some(vec![1]));
        assert_eq!(c("x", M::CTRL | M::ALT, None), Some(vec![0x1b, 0x18]));
        assert_eq!(c("b", M::ALT, Some("b")), Some(b"\x1bb".to_vec()));
        assert_eq!(c("한", M::empty(), Some("한")), Some("한".as_bytes().to_vec()));
        assert_eq!(c("a", M::empty(), None), None);
    }
}
