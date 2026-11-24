//! The agent's terminal tools (`docs/terminals.md`, Hermes Desk's `terminal_inspect` /
//! `terminal_input`): list the session's user terminals, read one (its screen and the lines
//! above it), type into one. Reading also works for related sessions' terminals; typing only
//! into the session's own, after the user's approval (`terminal.input`, `docs/approvals.md`).

use std::sync::Arc;

use ide_proto::{AgentToolResult, SessionState};

use super::UserCtx;
use crate::bridge::{TOOL_TERMINAL_INPUT, TOOL_TERMINAL_LIST, TOOL_TERMINAL_READ};
use crate::pty::TermHost;

/// Rows a user terminal keeps above its screen (what `ide_terminal_read` can return).
pub(crate) const SCROLLBACK: usize = 2000;
/// The capability of typing into a terminal (target: the terminal id).
pub(crate) const CAP_TERMINAL_INPUT: &str = "terminal.input";
/// `ide_terminal_read`'s default number of scrollback lines.
const DEFAULT_LINES: usize = 50;
/// The most text one `ide_terminal_input` types.
const MAX_TEXT: usize = 16 * 1024;

fn ok(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        text: text.into(),
        is_error: false,
        image: None,
    }
}

fn err(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        text: text.into(),
        is_error: true,
        image: None,
    }
}

/// The bytes a named key sends (`enter`, `ctrl+c`, `shift+tab`, `alt+f`, `up`, `f5`, a single
/// character, ...; case-insensitive names, modifiers `ctrl` / `alt` / `shift` joined with `+`).
/// `app_cursor`: the program switched the cursor keys to application mode (`ESC[?1h`).
/// `None`: not a key.
pub(crate) fn key_bytes(name: &str, app_cursor: bool) -> Option<Vec<u8>> {
    let name = name.trim();
    let (mods, key) = if name.len() > 1 && name.ends_with("++") {
        (&name[..name.len() - 2], "+")
    } else {
        match name.rsplit_once('+') {
            Some((m, k)) if !k.is_empty() => (m, k),
            _ => ("", name),
        }
    };
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    for m in mods.split('+').filter(|m| !m.is_empty()) {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" | "meta" | "option" => alt = true,
            "shift" => shift = true,
            _ => return None,
        }
    }
    // xterm's modifier parameter: 1 + shift + 2 alt + 4 ctrl.
    let m = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let lower = key.to_ascii_lowercase();
    let cursor = match lower.as_str() {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(c) = cursor {
        let s = if m > 1 {
            format!("\x1b[1;{m}{c}")
        } else if app_cursor {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        };
        return Some(s.into_bytes());
    }
    let tilde = match lower.as_str() {
        "insert" => Some(2),
        "delete" | "del" => Some(3),
        "pageup" | "pgup" => Some(5),
        "pagedown" | "pgdn" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(n) = tilde {
        let s = if m > 1 { format!("\x1b[{n};{m}~") } else { format!("\x1b[{n}~") };
        return Some(s.into_bytes());
    }
    let ss3 = match lower.as_str() {
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    };
    if let Some(c) = ss3 {
        let s = if m > 1 { format!("\x1b[1;{m}{c}") } else { format!("\x1bO{c}") };
        return Some(s.into_bytes());
    }
    let esc = |b: Vec<u8>| if alt { [vec![0x1b], b].concat() } else { b };
    let plain: Vec<u8> = match lower.as_str() {
        "enter" | "return" => b"\r".to_vec(),
        "tab" if shift => return Some(esc(b"\x1b[Z".to_vec())),
        "tab" => b"\t".to_vec(),
        "backtab" => return Some(esc(b"\x1b[Z".to_vec())),
        "esc" | "escape" => vec![0x1b],
        "backspace" if ctrl => return Some(esc(vec![0x08])),
        "backspace" => vec![0x7f],
        "space" if ctrl => return Some(esc(vec![0])),
        "space" => b" ".to_vec(),
        _ => {
            let mut chars = key.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            if c.is_control() {
                return None;
            }
            if ctrl {
                let b = match c.to_ascii_lowercase() {
                    c @ 'a'..='z' => c as u8 - b'a' + 1,
                    '@' | '2' => 0,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '-' | '7' => 0x1f,
                    '?' | '8' => 0x7f,
                    _ => return None,
                };
                return Some(esc(vec![b]));
            }
            let c = if shift { c.to_uppercase().next().unwrap_or(c) } else { c };
            return Some(esc(c.to_string().into_bytes()));
        }
    };
    if ctrl || (shift && !matches!(lower.as_str(), "enter" | "return" | "esc" | "escape" | "space")) {
        // No standard encoding (e.g. ctrl+enter): refuse rather than send something else.
        return None;
    }
    Some(esc(plain))
}

/// Text as typed: line breaks become Enter (`\r`), like pressing it.
fn text_bytes(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
}

/// `ide_terminal_read`'s answer: `head`, then up to the asked scrollback lines (`history`,
/// oldest first, of `total` kept; leading blank lines dropped), then the screen (trailing
/// blank rows dropped).
fn render_read(head: &str, history: &[String], total: usize, screen: &[String]) -> String {
    let mut out = format!("{head}\n");
    let first = history.iter().position(|l| !l.is_empty()).unwrap_or(history.len());
    if first < history.len() {
        let shown = history.len() - first;
        out.push_str(&format!(
            "--- scrollback: {shown} line(s) above the screen ({total} kept{}) ---\n",
            if shown < total { "; ask for more with `lines`" } else { "" }
        ));
        for l in &history[first..] {
            out.push_str(l);
            out.push('\n');
        }
    }
    out.push_str("--- screen ---\n");
    let end = screen.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
    if end == 0 {
        out.push_str("(empty)\n");
    }
    for l in &screen[..end] {
        out.push_str(l);
        out.push('\n');
    }
    out
}

fn state_text(s: SessionState) -> String {
    match s {
        SessionState::Running => "running".into(),
        SessionState::Stopped => "stopped".into(),
        SessionState::Exited(Some(c)) => format!("exited (code {c})"),
        SessionState::Exited(None) => "exited".into(),
    }
}

/// The keys of `ide_terminal_input` (`keys`: an array, or one string of names separated by
/// blanks).
fn key_names(v: Option<&serde_json::Value>) -> Result<Vec<String>, String> {
    match v {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::String(s)) => Ok(s.split_whitespace().map(str::to_owned).collect()),
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .map(|k| k.as_str().map(|s| s.trim().to_owned()).ok_or_else(|| "`keys` are strings".to_owned()))
            .collect(),
        Some(_) => Err("`keys` is an array of key names".into()),
    }
}

/// A user terminal, as found for a tool.
struct Found {
    id: String,
    session: String,
    title: String,
    command: String,
    cwd: String,
    host: Arc<TermHost>,
}

impl UserCtx {
    /// "<terminal title> (<session title>)" of a user terminal id (approval questions).
    pub(crate) fn uterm_label(&self, id: &str) -> Option<String> {
        let (title, session) = {
            let u = self.uterms.lock().unwrap();
            let t = u.list.iter().find(|t| t.id == id)?;
            (t.title.clone(), t.session.clone())
        };
        let st = self.resolve_session(&session).map(|s| s.title).unwrap_or_default();
        Some(if st.is_empty() { title } else { format!("{title} ({st})") })
    }

    /// The user terminal `id` that `session` may use: its own, or (`write == false`) a related
    /// session's.
    fn tool_terminal(&self, session: &str, id: &str, write: bool) -> Result<Found, String> {
        let id = id.trim();
        if id.is_empty() {
            return Err("`terminal` is required (an id from ide_terminal_list)".into());
        }
        let found = {
            let u = self.uterms.lock().unwrap();
            u.list.iter().find(|t| t.id == id).map(|t| Found {
                id: t.id.clone(),
                session: t.session.clone(),
                title: t.title.clone(),
                command: t.command.clone(),
                cwd: t.cwd.display().to_string(),
                host: t.host.clone(),
            })
        };
        let missing = || format!("no terminal `{id}` (see ide_terminal_list)");
        let t = found.ok_or_else(missing)?;
        if t.session == session {
            return Ok(t);
        }
        let s = self.resolve_session(session).map_err(|e| e.to_string())?;
        if t.session.is_empty() || self.related_target(&s, &t.session).is_err() {
            return Err(missing());
        }
        if write {
            return Err(format!(
                "`{id}` belongs to session `{}`: you may only type into this session's own terminals \
                 (ask that session with ide_send_to_session)",
                t.session
            ));
        }
        Ok(t)
    }

    /// `ide_terminal_list`: the user terminals of this session (or of a related one).
    fn terminal_list(&self, session: &str, args: &serde_json::Value) -> AgentToolResult {
        let own = match self.resolve_session(session) {
            Ok(s) => s,
            Err(e) => return err(format!("session not available: {e}")),
        };
        let want = args.get("session").and_then(|v| v.as_str()).unwrap_or("").trim();
        let of = if want.is_empty() || want == own.id.as_str() {
            own.id.to_string()
        } else {
            match self.related_target(&own, want) {
                Ok(t) => t.id.to_string(),
                Err(e) => return err(e),
            }
        };
        let terms: Vec<Found> = {
            let u = self.uterms.lock().unwrap();
            u.list
                .iter()
                .filter(|t| t.session == of)
                .map(|t| Found {
                    id: t.id.clone(),
                    session: t.session.clone(),
                    title: t.title.clone(),
                    command: t.command.clone(),
                    cwd: t.cwd.display().to_string(),
                    host: t.host.clone(),
                })
                .collect()
        };
        let pids: Vec<u32> = terms.iter().filter_map(|t| t.host.pid()).collect();
        let ports = crate::ports::by_root(&pids);
        let rows: Vec<serde_json::Value> = terms
            .iter()
            .map(|t| {
                let pid = t.host.pid();
                serde_json::json!({
                    "id": t.id,
                    "title": t.title,
                    "command": t.command,
                    "folder": t.cwd,
                    "state": state_text(t.host.state()),
                    "running": t.host.is_running(),
                    "ports": ports
                        .iter()
                        .filter(|(root, _)| Some(*root) == pid)
                        .map(|(_, p)| serde_json::json!({ "port": p.port, "process": p.process }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        let v = serde_json::json!({
            "session": of,
            "terminals": rows,
            "note": if of == own.id.as_str() {
                "read with ide_terminal_read, type with ide_terminal_input (asks the user)"
            } else {
                "another session's terminals: read only"
            },
        });
        ok(serde_json::to_string_pretty(&v).expect("json"))
    }

    /// `ide_terminal_read`.
    fn terminal_read(&self, session: &str, args: &serde_json::Value) -> AgentToolResult {
        let id = args.get("terminal").and_then(|v| v.as_str()).unwrap_or("");
        let t = match self.tool_terminal(session, id, false) {
            Ok(t) => t,
            Err(e) => return err(e),
        };
        let lines = args
            .get("lines")
            .and_then(|v| v.as_u64())
            .map_or(DEFAULT_LINES, |n| (n as usize).min(SCROLLBACK));
        let (history, total) = t.host.history_text(lines);
        let screen = t.host.screen_text();
        let (cols, rows) = t.host.size();
        let head = format!(
            "terminal {} \"{}\" ({}; command {}; folder {}; {cols}x{rows}{})",
            t.id,
            t.title,
            state_text(t.host.state()),
            t.command,
            t.cwd,
            if t.session == session { String::new() } else { format!("; session {}", t.session) },
        );
        ok(render_read(&head, &history, total, &screen))
    }

    /// The terminal tools; `None` = not one of them.
    pub(crate) async fn terminal_tool(self: &Arc<Self>, session: &str, tool: &str, args: &str) -> Option<AgentToolResult> {
        if ![TOOL_TERMINAL_LIST, TOOL_TERMINAL_READ, TOOL_TERMINAL_INPUT].contains(&tool) {
            return None;
        }
        let args: serde_json::Value = match serde_json::from_str(if args.trim().is_empty() { "{}" } else { args }) {
            Ok(v) => v,
            Err(e) => return Some(err(format!("invalid arguments: {e}"))),
        };
        if tool == TOOL_TERMINAL_INPUT {
            return Some(self.terminal_input(session, &args).await);
        }
        let (me, session, tool) = (self.clone(), session.to_owned(), tool.to_owned());
        // Blocking: the port scan reads the OS' sockets and processes.
        let r = tokio::task::spawn_blocking(move || {
            if tool == TOOL_TERMINAL_LIST {
                me.terminal_list(&session, &args)
            } else {
                me.terminal_read(&session, &args)
            }
        })
        .await;
        Some(r.unwrap_or_else(|_| err("the tool failed")))
    }

    /// `ide_terminal_input`: type text and keys into one of this session's terminals, once the
    /// user allowed it.
    async fn terminal_input(self: &Arc<Self>, session: &str, args: &serde_json::Value) -> AgentToolResult {
        let id = args.get("terminal").and_then(|v| v.as_str()).unwrap_or("");
        let t = match self.tool_terminal(session, id, true) {
            Ok(t) => t,
            Err(e) => return err(e),
        };
        let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
        if text.len() > MAX_TEXT {
            return err(format!("`text` is too long (at most {MAX_TEXT} bytes at a time)"));
        }
        let names = match key_names(args.get("keys")) {
            Ok(k) => k,
            Err(e) => return err(e),
        };
        if text.is_empty() && names.is_empty() {
            return err("give `text`, `keys` or both");
        }
        let app_cursor = t.host.app_cursor();
        let mut bytes = text_bytes(text);
        for k in &names {
            match key_bytes(k, app_cursor) {
                Some(b) => bytes.extend(b),
                None => {
                    return err(format!(
                        "unknown key `{k}`: use enter, tab, shift+tab, esc, backspace, delete, \
                         insert, up, down, left, right, home, end, pageup, pagedown, f1-f12, space, \
                         a single character, and ctrl+ / alt+ / shift+ with them (ctrl+c, alt+f, ctrl+left)"
                    ));
                }
            }
        }
        if !t.host.is_running() {
            return err(format!("terminal `{}` is not running ({})", t.id, state_text(t.host.state())));
        }
        let mut detail: String = text
            .chars()
            .take(400)
            .map(|c| if c.is_control() && c != '\n' && c != '\t' { '\u{fffd}' } else { c })
            .collect();
        if text.chars().count() > 400 {
            detail.push('…');
        }
        if !names.is_empty() {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str(&names.iter().map(|k| format!("[{k}]")).collect::<Vec<_>>().join(" "));
        }
        if let Err(why) = self.approve(session, CAP_TERMINAL_INPUT, &t.id, &detail).await {
            return err(why);
        }
        if let Err(e) = t.host.write(bytes) {
            return err(format!("could not type into `{}`: {e}", t.id));
        }
        self.audit("terminal.input", &format!("{session} -> {}", t.id));
        ok(format!(
            "typed into terminal {} ({} character(s){}). Read the result with ide_terminal_read.",
            t.id,
            text.chars().count(),
            if names.is_empty() { String::new() } else { format!(", keys {}", names.join(" ")) }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(name: &str) -> Option<Vec<u8>> {
        key_bytes(name, false)
    }

    #[test]
    fn key_table() {
        assert_eq!(k("enter").unwrap(), b"\r");
        assert_eq!(k("Return").unwrap(), b"\r");
        assert_eq!(k("tab").unwrap(), b"\t");
        assert_eq!(k("shift+tab").unwrap(), b"\x1b[Z");
        assert_eq!(k("esc").unwrap(), b"\x1b");
        assert_eq!(k("backspace").unwrap(), b"\x7f");
        assert_eq!(k("ctrl+backspace").unwrap(), b"\x08");
        assert_eq!(k("delete").unwrap(), b"\x1b[3~");
        assert_eq!(k("pageup").unwrap(), b"\x1b[5~");
        assert_eq!(k("PgDn").unwrap(), b"\x1b[6~");
        assert_eq!(k("up").unwrap(), b"\x1b[A");
        assert_eq!(key_bytes("up", true).unwrap(), b"\x1bOA");
        assert_eq!(k("left").unwrap(), b"\x1b[D");
        assert_eq!(k("home").unwrap(), b"\x1b[H");
        assert_eq!(k("ctrl+left").unwrap(), b"\x1b[1;5D");
        assert_eq!(key_bytes("shift+end", true).unwrap(), b"\x1b[1;2F");
        assert_eq!(k("f1").unwrap(), b"\x1bOP");
        assert_eq!(k("f5").unwrap(), b"\x1b[15~");
        assert_eq!(k("ctrl+f5").unwrap(), b"\x1b[15;5~");
        assert_eq!(k("f12").unwrap(), b"\x1b[24~");
        assert_eq!(k("ctrl+c").unwrap(), b"\x03");
        assert_eq!(k("Ctrl+C").unwrap(), b"\x03");
        assert_eq!(k("ctrl+d").unwrap(), b"\x04");
        assert_eq!(k("ctrl+z").unwrap(), b"\x1a");
        assert_eq!(k("ctrl+[").unwrap(), b"\x1b");
        assert_eq!(k("ctrl+\\").unwrap(), b"\x1c");
        assert_eq!(k("ctrl+space").unwrap(), b"\x00");
        assert_eq!(k("alt+f").unwrap(), b"\x1bf");
        assert_eq!(k("alt+enter").unwrap(), b"\x1b\r");
        assert_eq!(k("ctrl+alt+c").unwrap(), b"\x1b\x03");
        assert_eq!(k("space").unwrap(), b" ");
        assert_eq!(k("y").unwrap(), b"y");
        assert_eq!(k("shift+y").unwrap(), b"Y");
        assert_eq!(k("+").unwrap(), b"+");
        assert_eq!(k("alt++").unwrap(), b"\x1b+");
        assert_eq!(k("é").unwrap(), "é".as_bytes());
        for bad in ["", "enterr", "ctrl+enter", "hyper+c", "ctrl+é", "ab", "shift+backspace"] {
            assert_eq!(k(bad), None, "{bad}");
        }
    }

    #[test]
    fn text_and_key_names() {
        assert_eq!(text_bytes("ls\n"), b"ls\r");
        assert_eq!(text_bytes("a\r\nb"), b"a\rb");
        assert_eq!(key_names(Some(&serde_json::json!("ctrl+c  enter"))).unwrap(), ["ctrl+c", "enter"]);
        assert_eq!(key_names(Some(&serde_json::json!(["up", " enter "]))).unwrap(), ["up", "enter"]);
        assert!(key_names(None).unwrap().is_empty());
        assert!(key_names(Some(&serde_json::json!(3))).is_err());
        assert!(key_names(Some(&serde_json::json!([1]))).is_err());
    }

    #[test]
    fn read_output_is_trimmed() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        let out = render_read("terminal t", &s(&["", "", "old 1", "old 2"]), 10, &s(&["$ ls", "a b", "", ""]));
        assert_eq!(
            out,
            "terminal t\n--- scrollback: 2 line(s) above the screen (10 kept; ask for more with `lines`) ---\n\
             old 1\nold 2\n--- screen ---\n$ ls\na b\n"
        );
        // Everything kept shown: no hint; blank rows inside the screen stay.
        let out = render_read("h", &s(&["x"]), 1, &s(&["a", "", "b", ""]));
        assert_eq!(out, "h\n--- scrollback: 1 line(s) above the screen (1 kept) ---\nx\n--- screen ---\na\n\nb\n");
        // No scrollback asked or kept, empty screen.
        assert_eq!(render_read("h", &[], 0, &s(&["", ""])), "h\n--- screen ---\n(empty)\n");
        assert_eq!(render_read("h", &s(&["", ""]), 5, &s(&["p"])), "h\n--- screen ---\np\n");
    }

    #[test]
    fn scrollback_keeps_rows_above_the_screen() {
        let (ev, _) = tokio::sync::broadcast::channel(16);
        let host = TermHost::with_scrollback("t", 20, 3, ev, 5);
        for i in 1..=10 {
            host.ingest(format!("line {i}\r\n").as_bytes());
        }
        // Screen: line 9, line 10, the empty prompt row; above it at most 5 rows.
        assert_eq!(host.screen_text(), ["line 9", "line 10", ""]);
        let (h, total) = host.history_text(100);
        assert_eq!(total, 5);
        assert_eq!(h, ["line 4", "line 5", "line 6", "line 7", "line 8"]);
        assert_eq!(host.history_text(2).0, ["line 7", "line 8"]);
        assert!(!host.app_cursor());
        host.ingest(b"\x1b[?1h");
        assert!(host.app_cursor());
        // Sessions' agent terminals keep no scrollback.
        let (ev, _) = tokio::sync::broadcast::channel(16);
        let plain = TermHost::new("p", 20, 3, ev);
        for i in 1..=10 {
            plain.ingest(format!("line {i}\r\n").as_bytes());
        }
        assert_eq!(plain.history_text(100), (Vec::new(), 0));
    }
}
