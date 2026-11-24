//! Chrome DevTools protocol messages (pure: no CEF), for [`crate::DevTools`]: the request
//! envelope, synthetic mouse and key events (`Input.dispatchMouseEvent` /
//! `Input.dispatchKeyEvent`, delivered inside the browser, never as OS input) and key names.

use serde_json::{Value, json};

/// A CDP request as `send_dev_tools_message` takes it.
pub fn request(id: i32, method: &str, params: &Value) -> Vec<u8> {
    let params = if params.is_null() { json!({}) } else { params.clone() };
    json!({"id": id, "method": method, "params": params})
        .to_string()
        .into_bytes()
}

/// The answer of `on_dev_tools_method_result`: the `result` object, or the protocol's error
/// (`{"code", "message"}`) as text.
pub fn answer(success: bool, body: &[u8]) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    if success {
        Ok(v)
    } else {
        Err(v
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned()))
    }
}

/// `Input.dispatchMouseEvent` parameters of a click at `(x, y)` (CSS pixels of the viewport):
/// move, press, release.
pub fn click(x: f64, y: f64, button: &str, count: u32) -> Vec<Value> {
    let count = count.max(1);
    vec![
        json!({"type": "mouseMoved", "x": x, "y": y, "button": "none"}),
        json!({"type": "mousePressed", "x": x, "y": y, "button": button, "clickCount": count}),
        json!({"type": "mouseReleased", "x": x, "y": y, "button": button, "clickCount": count}),
    ]
}

/// `Input.dispatchMouseEvent` parameters of a wheel turn at `(x, y)`.
pub fn wheel(x: f64, y: f64, dx: f64, dy: f64) -> Value {
    json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy})
}

/// CDP modifier bits.
const ALT: i64 = 1;
const CTRL: i64 = 2;
const META: i64 = 4;
const SHIFT: i64 = 8;

/// One key press of a key spec such as `Enter`, `ctrl+a`, `shift+Tab`, `ArrowDown`, `F5` or a
/// single character.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyPress {
    /// DOM `key`.
    pub key: String,
    /// DOM `code`.
    pub code: String,
    /// Windows virtual key code (Chromium needs it for editing keys).
    pub vk: i64,
    /// Text the key types (none for control keys and with ctrl / alt / meta).
    pub text: Option<String>,
    pub modifiers: i64,
}

impl KeyPress {
    /// `Input.dispatchKeyEvent` parameters: down (`keyDown` with text, else `rawKeyDown`), up.
    pub fn events(&self) -> Vec<Value> {
        let mut down = json!({
            "type": if self.text.is_some() { "keyDown" } else { "rawKeyDown" },
            "key": self.key,
            "code": self.code,
            "windowsVirtualKeyCode": self.vk,
            "nativeVirtualKeyCode": self.vk,
            "modifiers": self.modifiers,
        });
        if let Some(t) = &self.text {
            down["text"] = json!(t);
            down["unmodifiedText"] = json!(t);
        }
        let up = json!({
            "type": "keyUp",
            "key": self.key,
            "code": self.code,
            "windowsVirtualKeyCode": self.vk,
            "nativeVirtualKeyCode": self.vk,
            "modifiers": self.modifiers,
        });
        vec![down, up]
    }
}

/// Parses a key spec: modifiers (`ctrl`, `control`, `alt`, `shift`, `meta`, `cmd`) joined with
/// `+`, then a key name (case-insensitive for named keys) or one character.
pub fn parse_key(spec: &str) -> Result<KeyPress, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("`key` is empty".into());
    }
    // `+` alone or as the last part ("ctrl++") is the plus key.
    let (mods, name) = match spec.strip_suffix("++") {
        Some(m) => (m, "+"),
        None if spec == "+" => ("", "+"),
        None => match spec.rsplit_once('+') {
            Some((m, k)) => (m, k),
            None => ("", spec),
        },
    };
    let mut modifiers = 0;
    for m in mods.split('+').filter(|m| !m.is_empty()) {
        modifiers |= match m.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => CTRL,
            "alt" | "option" => ALT,
            "shift" => SHIFT,
            "meta" | "cmd" | "command" | "win" => META,
            other => return Err(format!("unknown modifier `{other}` in `{spec}`")),
        };
    }
    let named = |key: &str, code: &str, vk: i64, text: Option<&str>| KeyPress {
        key: key.into(),
        code: code.into(),
        vk,
        text: text.map(str::to_owned),
        modifiers,
    };
    let lower = name.to_ascii_lowercase();
    let mut press = match lower.as_str() {
        "enter" | "return" => named("Enter", "Enter", 13, Some("\r")),
        "tab" => named("Tab", "Tab", 9, None),
        "escape" | "esc" => named("Escape", "Escape", 27, None),
        "backspace" => named("Backspace", "Backspace", 8, None),
        "delete" | "del" => named("Delete", "Delete", 46, None),
        "space" => named(" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => named("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => named("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => named("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => named("ArrowRight", "ArrowRight", 39, None),
        "home" => named("Home", "Home", 36, None),
        "end" => named("End", "End", 35, None),
        "pageup" => named("PageUp", "PageUp", 33, None),
        "pagedown" => named("PageDown", "PageDown", 34, None),
        "insert" => named("Insert", "Insert", 45, None),
        f if f.len() >= 2
            && f.starts_with('f')
            && f[1..].parse::<i64>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            let n: i64 = f[1..].parse().unwrap_or(1);
            let key = format!("F{n}");
            named(&key, &key, 111 + n, None)
        }
        _ => {
            let mut chars = name.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return Err(format!("unknown key `{name}`"));
            };
            char_key(c, modifiers)
        }
    };
    // Shortcuts type nothing (ctrl+a selects, it does not type "a").
    if modifiers & (CTRL | ALT | META) != 0 {
        press.text = None;
    }
    Ok(press)
}

/// A one-character key: letters and digits get their `code` and virtual key; other characters
/// only type themselves.
fn char_key(c: char, modifiers: i64) -> KeyPress {
    let up = c.to_ascii_uppercase();
    let (code, vk) = if c.is_ascii_alphabetic() {
        (format!("Key{up}"), up as i64)
    } else if c.is_ascii_digit() {
        (format!("Digit{c}"), c as i64)
    } else {
        (String::new(), 0)
    };
    let shifted = modifiers & SHIFT != 0 && c.is_ascii_alphabetic();
    let typed = if shifted { up } else { c };
    KeyPress {
        key: typed.to_string(),
        code,
        vk,
        text: Some(typed.to_string()),
        modifiers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_envelope() {
        let r: Value = serde_json::from_slice(&request(7, "Runtime.evaluate", &json!({"expression": "1+1"}))).unwrap();
        assert_eq!(r, json!({"id": 7, "method": "Runtime.evaluate", "params": {"expression": "1+1"}}));
        let r: Value = serde_json::from_slice(&request(8, "Page.enable", &Value::Null)).unwrap();
        assert_eq!(r["params"], json!({}));
    }

    #[test]
    fn answers() {
        assert_eq!(answer(true, br#"{"data":"x"}"#).unwrap()["data"], "x");
        assert_eq!(answer(false, br#"{"code":-32000,"message":"No node"}"#).unwrap_err(), "No node");
        assert_eq!(answer(false, b"boom").unwrap_err(), "boom");
    }

    #[test]
    fn mouse_events() {
        let c = click(10.0, 20.5, "left", 1);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0]["type"], "mouseMoved");
        assert_eq!(c[1], json!({"type": "mousePressed", "x": 10.0, "y": 20.5, "button": "left", "clickCount": 1}));
        assert_eq!(c[2]["type"], "mouseReleased");
        assert_eq!(click(0.0, 0.0, "left", 0)[1]["clickCount"], 1);
        assert_eq!(wheel(1.0, 2.0, 0.0, 300.0)["deltaY"], 300.0);
    }

    #[test]
    fn keys() {
        let k = parse_key("Enter").unwrap();
        assert_eq!((k.key.as_str(), k.vk, k.text.as_deref()), ("Enter", 13, Some("\r")));
        let e = k.events();
        assert_eq!(e[0]["type"], "keyDown");
        assert_eq!(e[1]["type"], "keyUp");

        let k = parse_key("ctrl+a").unwrap();
        assert_eq!((k.key.as_str(), k.code.as_str(), k.vk, k.modifiers), ("a", "KeyA", 65, CTRL));
        assert_eq!(k.text, None);
        assert_eq!(k.events()[0]["type"], "rawKeyDown");

        let k = parse_key("Shift+Tab").unwrap();
        assert_eq!((k.key.as_str(), k.modifiers), ("Tab", SHIFT));
        let k = parse_key("shift+b").unwrap();
        assert_eq!((k.key.as_str(), k.text.as_deref()), ("B", Some("B")));
        assert_eq!(parse_key("down").unwrap().key, "ArrowDown");
        assert_eq!(parse_key("F5").unwrap().vk, 116);
        assert_eq!(parse_key("7").unwrap().code, "Digit7");
        assert_eq!(parse_key("ctrl++").unwrap().key, "+");
        assert_eq!(parse_key("한").unwrap().text.as_deref(), Some("한"));
        assert!(parse_key("hyper+x").is_err());
        assert!(parse_key("Enterr").is_err());
        assert!(parse_key(" ").is_err());
    }
}
