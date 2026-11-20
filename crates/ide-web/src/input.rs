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

/// Scan code for `KeyEvent::native_key_code`: CEF derives `KeyboardEvent.code` from it.
fn native_key_code(vk: i32, up: bool) -> i32 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC_EX, MapVirtualKeyW};
        // SAFETY: pure table lookup in user32; no input is generated.
        let sc = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC_EX) };
        let _ = up;
        // CEF maps `native_key_code` to the DOM `code` as a Windows scan code (0xE0xx = extended).
        sc as i32
    }
    #[cfg(not(windows))]
    {
        let _ = (vk, up);
        0
    }
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
                    native_key_code: native_key_code(vk, false),
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
                if modifiers.control() && !modifiers.alt() {
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
                        native_key_code: vk.map(|vk| native_key_code(vk, false)).unwrap_or(0),
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
                    native_key_code: native_key_code(vk, true),
                    is_system_key: modifiers.alt() as _,
                    ..Default::default()
                }));
            }
        }
        keyboard::Event::ModifiersChanged(_) => {}
    }
}
