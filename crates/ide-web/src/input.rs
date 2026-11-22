//! iced keyboard/mouse values -> CEF input structs.

use cef::{BrowserHost, ImplBrowserHost as _, KeyEvent, KeyEventType};
use iced::keyboard::key::{Code, Named, Physical};
use iced::keyboard::{self, Key, Modifiers};

// cef_event_flags_t
pub(crate) const EF_SHIFT: u32 = 1 << 1;
pub(crate) const EF_CONTROL: u32 = 1 << 2;
pub(crate) const EF_ALT: u32 = 1 << 3;
pub(crate) const EF_LEFT_MOUSE: u32 = 1 << 4;
pub(crate) const EF_MIDDLE_MOUSE: u32 = 1 << 5;
pub(crate) const EF_RIGHT_MOUSE: u32 = 1 << 6;
pub(crate) const EF_COMMAND: u32 = 1 << 7;
pub(crate) const EF_IS_KEY_PAD: u32 = 1 << 9;

pub(crate) fn modifier_flags(m: Modifiers) -> u32 {
    (if m.shift() { EF_SHIFT } else { 0 })
        | (if m.control() { EF_CONTROL } else { 0 })
        | (if m.alt() { EF_ALT } else { 0 })
        | (if m.logo() { EF_COMMAND } else { 0 })
}

/// Windows virtual-key code for a physical key (layout independent, like Chromium expects).
fn vk_from_code(code: Code) -> Option<i32> {
    use Code::*;
    Some(match code {
        KeyA => 0x41,
        KeyB => 0x42,
        KeyC => 0x43,
        KeyD => 0x44,
        KeyE => 0x45,
        KeyF => 0x46,
        KeyG => 0x47,
        KeyH => 0x48,
        KeyI => 0x49,
        KeyJ => 0x4A,
        KeyK => 0x4B,
        KeyL => 0x4C,
        KeyM => 0x4D,
        KeyN => 0x4E,
        KeyO => 0x4F,
        KeyP => 0x50,
        KeyQ => 0x51,
        KeyR => 0x52,
        KeyS => 0x53,
        KeyT => 0x54,
        KeyU => 0x55,
        KeyV => 0x56,
        KeyW => 0x57,
        KeyX => 0x58,
        KeyY => 0x59,
        KeyZ => 0x5A,
        Digit0 => 0x30,
        Digit1 => 0x31,
        Digit2 => 0x32,
        Digit3 => 0x33,
        Digit4 => 0x34,
        Digit5 => 0x35,
        Digit6 => 0x36,
        Digit7 => 0x37,
        Digit8 => 0x38,
        Digit9 => 0x39,
        F1 => 0x70,
        F2 => 0x71,
        F3 => 0x72,
        F4 => 0x73,
        F5 => 0x74,
        F6 => 0x75,
        F7 => 0x76,
        F8 => 0x77,
        F9 => 0x78,
        F10 => 0x79,
        F11 => 0x7A,
        F12 => 0x7B,
        Numpad0 => 0x60,
        Numpad1 => 0x61,
        Numpad2 => 0x62,
        Numpad3 => 0x63,
        Numpad4 => 0x64,
        Numpad5 => 0x65,
        Numpad6 => 0x66,
        Numpad7 => 0x67,
        Numpad8 => 0x68,
        Numpad9 => 0x69,
        NumpadMultiply => 0x6A,
        NumpadAdd => 0x6B,
        NumpadSubtract => 0x6D,
        NumpadDecimal => 0x6E,
        NumpadDivide => 0x6F,
        NumpadEnter => 0x0D,
        Backspace => 0x08,
        Tab => 0x09,
        Enter => 0x0D,
        ShiftLeft | ShiftRight => 0x10,
        ControlLeft | ControlRight => 0x11,
        AltLeft | AltRight => 0x12,
        Pause => 0x13,
        CapsLock => 0x14,
        Escape => 0x1B,
        Space => 0x20,
        PageUp => 0x21,
        PageDown => 0x22,
        End => 0x23,
        Home => 0x24,
        ArrowLeft => 0x25,
        ArrowUp => 0x26,
        ArrowRight => 0x27,
        ArrowDown => 0x28,
        PrintScreen => 0x2C,
        Insert => 0x2D,
        Delete => 0x2E,
        SuperLeft => 0x5B,
        SuperRight => 0x5C,
        ContextMenu => 0x5D,
        NumLock => 0x90,
        ScrollLock => 0x91,
        Semicolon => 0xBA,
        Equal => 0xBB,
        Comma => 0xBC,
        Minus => 0xBD,
        Period => 0xBE,
        Slash => 0xBF,
        Backquote => 0xC0,
        BracketLeft => 0xDB,
        Backslash => 0xDC,
        BracketRight => 0xDD,
        Quote => 0xDE,
        IntlBackslash => 0xE2,
        Lang1 => 0x15, // VK_HANGUL (한/영)
        Lang2 => 0x19, // VK_HANJA
        _ => return None,
    })
}

/// Fallback when the physical key is unknown (e.g. synthetic events).
fn vk_from_key(key: &Key) -> Option<i32> {
    Some(match key {
        Key::Named(n) => match n {
            Named::Backspace => 0x08,
            Named::Tab => 0x09,
            Named::Enter => 0x0D,
            Named::Shift => 0x10,
            Named::Control => 0x11,
            Named::Alt => 0x12,
            Named::Escape => 0x1B,
            Named::Space => 0x20,
            Named::PageUp => 0x21,
            Named::PageDown => 0x22,
            Named::End => 0x23,
            Named::Home => 0x24,
            Named::ArrowLeft => 0x25,
            Named::ArrowUp => 0x26,
            Named::ArrowRight => 0x27,
            Named::ArrowDown => 0x28,
            Named::Insert => 0x2D,
            Named::Delete => 0x2E,
            Named::HangulMode => 0x15,
            Named::HanjaMode => 0x19,
            Named::F1 => 0x70,
            Named::F5 => 0x74,
            Named::F12 => 0x7B,
            _ => return None,
        },
        Key::Character(c) => {
            let ch = c.chars().next()?.to_ascii_uppercase();
            if ch.is_ascii_alphanumeric() {
                ch as i32
            } else {
                return None;
            }
        }
        Key::Unidentified => return None,
    })
}

fn is_keypad(physical: &Physical) -> bool {
    matches!(physical, Physical::Code(c) if format!("{c:?}").starts_with("Numpad"))
}

/// `KeyEvent::native_key_code` of a key: CEF derives `KeyboardEvent.code` from it, in the OS's
/// own numbering. Windows: the scan code of the virtual key (0xE0xx = extended); macOS: the
/// `kVK_*` virtual key code; Linux: the X11 keycode (evdev code + 8). 0 when unknown.
fn native_key_code(vk: i32, code: Option<Code>) -> i32 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC_EX, MapVirtualKeyW};
        let _ = code;
        // SAFETY: pure table lookup in user32; no input is generated.
        unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC_EX) as i32 }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = vk;
        code.and_then(mac_key_code).unwrap_or(0)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = vk;
        code.and_then(|c| evdev_code(c).map(|e| e + 8)).unwrap_or(0)
    }
}

/// macOS `kVK_*` codes (Carbon `Events.h`).
#[cfg(any(target_os = "macos", test))]
fn mac_key_code(code: Code) -> Option<i32> {
    use Code::*;
    Some(match code {
        KeyA => 0x00, KeyS => 0x01, KeyD => 0x02, KeyF => 0x03, KeyH => 0x04, KeyG => 0x05,
        KeyZ => 0x06, KeyX => 0x07, KeyC => 0x08, KeyV => 0x09, KeyB => 0x0B, KeyQ => 0x0C,
        KeyW => 0x0D, KeyE => 0x0E, KeyR => 0x0F, KeyY => 0x10, KeyT => 0x11, Digit1 => 0x12,
        Digit2 => 0x13, Digit3 => 0x14, Digit4 => 0x15, Digit6 => 0x16, Digit5 => 0x17,
        Equal => 0x18, Digit9 => 0x19, Digit7 => 0x1A, Minus => 0x1B, Digit8 => 0x1C,
        Digit0 => 0x1D, BracketRight => 0x1E, KeyO => 0x1F, KeyU => 0x20, BracketLeft => 0x21,
        KeyI => 0x22, KeyP => 0x23, Enter => 0x24, KeyL => 0x25, KeyJ => 0x26, Quote => 0x27,
        KeyK => 0x28, Semicolon => 0x29, Backslash => 0x2A, Comma => 0x2B, Slash => 0x2C,
        KeyN => 0x2D, KeyM => 0x2E, Period => 0x2F, Tab => 0x30, Space => 0x31, Backquote => 0x32,
        Backspace => 0x33, Escape => 0x35, SuperRight => 0x36, SuperLeft => 0x37,
        ShiftLeft => 0x38, CapsLock => 0x39, AltLeft => 0x3A, ControlLeft => 0x3B,
        ShiftRight => 0x3C, AltRight => 0x3D, ControlRight => 0x3E, F5 => 0x60, F6 => 0x61,
        F7 => 0x62, F3 => 0x63, F8 => 0x64, F9 => 0x65, F11 => 0x67, F10 => 0x6D, F12 => 0x6F,
        Insert => 0x72, Home => 0x73, PageUp => 0x74, Delete => 0x75, F4 => 0x76, End => 0x77,
        F2 => 0x78, PageDown => 0x79, F1 => 0x7A, ArrowLeft => 0x7B, ArrowRight => 0x7C,
        ArrowDown => 0x7D, ArrowUp => 0x7E,
        _ => return None,
    })
}

/// Linux evdev codes (`input-event-codes.h`).
#[cfg(any(not(any(windows, target_os = "macos")), test))]
fn evdev_code(code: Code) -> Option<i32> {
    use Code::*;
    Some(match code {
        Escape => 1, Digit1 => 2, Digit2 => 3, Digit3 => 4, Digit4 => 5, Digit5 => 6,
        Digit6 => 7, Digit7 => 8, Digit8 => 9, Digit9 => 10, Digit0 => 11, Minus => 12,
        Equal => 13, Backspace => 14, Tab => 15, KeyQ => 16, KeyW => 17, KeyE => 18, KeyR => 19,
        KeyT => 20, KeyY => 21, KeyU => 22, KeyI => 23, KeyO => 24, KeyP => 25,
        BracketLeft => 26, BracketRight => 27, Enter => 28, ControlLeft => 29, KeyA => 30,
        KeyS => 31, KeyD => 32, KeyF => 33, KeyG => 34, KeyH => 35, KeyJ => 36, KeyK => 37,
        KeyL => 38, Semicolon => 39, Quote => 40, Backquote => 41, ShiftLeft => 42,
        Backslash => 43, KeyZ => 44, KeyX => 45, KeyC => 46, KeyV => 47, KeyB => 48, KeyN => 49,
        KeyM => 50, Comma => 51, Period => 52, Slash => 53, ShiftRight => 54, AltLeft => 56,
        Space => 57, CapsLock => 58, F1 => 59, F2 => 60, F3 => 61, F4 => 62, F5 => 63, F6 => 64,
        F7 => 65, F8 => 66, F9 => 67, F10 => 68, F11 => 87, F12 => 88, ControlRight => 97,
        AltRight => 100, Home => 102, ArrowUp => 103, PageUp => 104, ArrowLeft => 105,
        ArrowRight => 106, End => 107, ArrowDown => 108, PageDown => 109, Insert => 110,
        Delete => 111, SuperLeft => 125, SuperRight => 126,
        _ => return None,
    })
}

/// Keys the OS IME consumed (composition in progress): nothing to forward.
pub(crate) fn is_ime_processed(key: &Key) -> bool {
    matches!(key, Key::Named(Named::Process) | Key::Unidentified)
}

/// Forwards one iced keyboard event to CEF (RAWKEYDOWN + CHAR for text, KEYUP).
pub(crate) fn send_key(host: &BrowserHost, ev: &keyboard::Event) {
    match ev {
        keyboard::Event::KeyPressed {
            key,
            physical_key,
            modifiers,
            text,
            ..
        } => {
            let mut flags = modifier_flags(*modifiers);
            if is_keypad(physical_key) {
                flags |= EF_IS_KEY_PAD;
            }
            let code = match physical_key {
                Physical::Code(c) => Some(*c),
                _ => None,
            };
            let vk = match physical_key {
                Physical::Code(c) => vk_from_code(*c),
                _ => None,
            }
            .or_else(|| vk_from_key(key));
            if let Some(vk) = vk {
                host.send_key_event(Some(&KeyEvent {
                    type_: KeyEventType::RAWKEYDOWN,
                    modifiers: flags,
                    windows_key_code: vk,
                    native_key_code: native_key_code(vk, code),
                    is_system_key: modifiers.alt() as _,
                    ..Default::default()
                }));
            }
            // Enter produces no `text` on some paths but needs a CHAR for forms/textarea.
            let enter = matches!(key, Key::Named(Named::Enter));
            let text: Option<&str> = match text.as_deref() {
                Some(t) => Some(t),
                None if enter => Some("\r"),
                None => None,
            };
            if let Some(t) = text {
                // Ctrl (Cmd on macOS) + key is a shortcut.
                if (modifiers.control() || modifiers.logo()) && !modifiers.alt() {
                    return; // Ctrl+<key> is a shortcut (handled from the RAWKEYDOWN), not text.
                }
                for u in t.encode_utf16() {
                    if u < 0x20 && u != 0x0D {
                        continue;
                    }
                    host.send_key_event(Some(&KeyEvent {
                        type_: KeyEventType::CHAR,
                        modifiers: flags,
                        windows_key_code: u as i32,
                        native_key_code: vk.map(|vk| native_key_code(vk, code)).unwrap_or(0),
                        character: u,
                        unmodified_character: u,
                        ..Default::default()
                    }));
                }
            }
        }
        keyboard::Event::KeyReleased {
            key,
            physical_key,
            modifiers,
            ..
        } => {
            let code = match physical_key {
                Physical::Code(c) => Some(*c),
                _ => None,
            };
            let vk = match physical_key {
                Physical::Code(c) => vk_from_code(*c),
                _ => None,
            }
            .or_else(|| vk_from_key(key));
            if let Some(vk) = vk {
                host.send_key_event(Some(&KeyEvent {
                    type_: KeyEventType::KEYUP,
                    modifiers: modifier_flags(*modifiers),
                    windows_key_code: vk,
                    native_key_code: native_key_code(vk, code),
                    is_system_key: modifiers.alt() as _,
                    ..Default::default()
                }));
            }
        }
        keyboard::Event::ModifiersChanged(_) => {}
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;

    #[test]
    fn native_key_codes_of_other_platforms() {
        assert_eq!(mac_key_code(Code::KeyA), Some(0x00));
        assert_eq!(mac_key_code(Code::Enter), Some(0x24));
        assert_eq!(mac_key_code(Code::ArrowUp), Some(0x7E));
        assert_eq!(evdev_code(Code::KeyA), Some(30));
        assert_eq!(evdev_code(Code::Escape), Some(1));
        assert_eq!(evdev_code(Code::F12), Some(88));
    }
}
