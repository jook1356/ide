//! A widget page's direct access to ide (`widget.host`, `docs/widgets.md` "Host API"):
//! running programs (`exec`), files (`fs.*`), user terminals (`term.*`) and the sessions
//! (`sessions.*`). Unlike Hermes Desk's sandboxed mods a widget is the user's own tool, so
//! these work anywhere the user can; what a widget may use is its `tools` list (allowed at
//! registration) minus what the user blocked for it (`widget::Policy`). Answers are JSON text.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use base64::Engine as _;
use ide_core::Session;
use ide_proto::{AgentToolResult, Request, Response};
use serde_json::{Value, json};

use super::terminals::{key_bytes, key_names, state_text, text_bytes};
use super::{OpenArgs, UserCtx};

/// `exec`'s default and longest run.
const EXEC_DEFAULT: Duration = Duration::from_secs(120);
const EXEC_MAX: Duration = Duration::from_secs(3600);
/// Most output kept per stream of `exec` (the rest is read and dropped).
const MAX_OUTPUT: usize = 16 * 1024 * 1024;
/// Largest file `fs.read` returns.
const MAX_READ: u64 = 32 * 1024 * 1024;

type HostResult = Result<Value, String>;

fn ok(v: Value) -> AgentToolResult {
    AgentToolResult {
        text: v.to_string(),
        is_error: false,
        image: None,
    }
}

fn fail(t: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        text: t.into(),
        is_error: true,
        image: None,
    }
}

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

fn required<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    s(a, k).filter(|v| !v.is_empty()).ok_or_else(|| format!("`{k}` is required"))
}

/// `p` as an absolute path: relative ones are under the session folder.
fn resolve(session: &Session, p: &str) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() { p.to_path_buf() } else { session.root.join(p) }
}

fn mtime(m: &std::fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// Read all of a pipe, keeping at most [`MAX_OUTPUT`] bytes.
fn drain(mut r: impl std::io::Read) -> (Vec<u8>, bool) {
    let mut keep = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    let mut cut = false;
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = MAX_OUTPUT.saturating_sub(keep.len());
                keep.extend_from_slice(&buf[..n.min(room)]);
                cut |= n > room;
            }
        }
    }
    (keep, cut)
}

/// `exec`: run a program (or a command line through the system shell) and wait for it.
fn exec(session: &Session, a: &Value) -> HostResult {
    let mut cmd = match (s(a, "program").filter(|p| !p.is_empty()), s(a, "command").filter(|c| !c.is_empty())) {
        (Some(p), None) => {
            let mut c = Command::new(p);
            if let Some(args) = a.get("args").and_then(Value::as_array) {
                for x in args {
                    match x.as_str() {
                        Some(x) => c.arg(x),
                        None => c.arg(x.to_string()),
                    };
                }
            }
            c
        }
        (None, Some(line)) => {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt as _;
                let mut c = Command::new("cmd");
                c.args(["/d", "/s", "/c"]).raw_arg(format!("\"{line}\""));
                c
            }
            #[cfg(not(windows))]
            {
                let mut c = Command::new("sh");
                c.args(["-c", line]);
                c
            }
        }
        _ => return Err("give `program` (with `args`) or `command` (a command line for the system shell)".into()),
    };
    let cwd = s(a, "cwd").filter(|c| !c.is_empty()).map_or_else(|| session.root.clone(), |c| resolve(session, c));
    cmd.current_dir(&cwd);
    if let Some(env) = a.get("env").and_then(Value::as_object) {
        for (k, v) in env {
            match v {
                Value::Null => cmd.env_remove(k),
                Value::String(v) => cmd.env(k, v),
                v => cmd.env(k, v.to_string()),
            };
        }
    }
    let stdin = s(a, "stdin").map(str::to_owned);
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let timeout = a
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .map_or(EXEC_DEFAULT, Duration::from_millis)
        .min(EXEC_MAX);
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| format!("could not start it in {}: {e}", cwd.display()))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        });
    }
    let out = child.stdout.take().map(|p| std::thread::spawn(move || drain(p)));
    let err = child.stderr.take().map(|p| std::thread::spawn(move || drain(p)));
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if started.elapsed() >= timeout => {
                timed_out = true;
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(format!("waiting for it failed: {e}")),
        }
    };
    let join = |h: Option<std::thread::JoinHandle<(Vec<u8>, bool)>>| h.and_then(|h| h.join().ok()).unwrap_or_default();
    let (stdout, out_cut) = join(out);
    let (stderr, err_cut) = join(err);
    Ok(json!({
        "code": status.and_then(|s| s.code()),
        "stdout": String::from_utf8_lossy(&stdout),
        "stderr": String::from_utf8_lossy(&stderr),
        "timed_out": timed_out,
        "truncated": out_cut || err_cut,
        "ms": started.elapsed().as_millis() as u64,
    }))
}

/// `fs.*`.
fn fs(session: &Session, method: &str, a: &Value) -> HostResult {
    let path = resolve(session, required(a, "path")?);
    let io = |e: std::io::Error| format!("{}: {e}", path.display());
    match method {
        "fs.read" => {
            let m = std::fs::metadata(&path).map_err(io)?;
            if m.len() > MAX_READ {
                return Err(format!("{}: larger than {} MiB", path.display(), MAX_READ >> 20));
            }
            let mut data = Vec::new();
            std::fs::File::open(&path).and_then(|mut f| f.read_to_end(&mut data)).map_err(io)?;
            Ok(if s(a, "encoding") == Some("base64") {
                json!({ "base64": base64::engine::general_purpose::STANDARD.encode(&data), "size": data.len() })
            } else {
                json!({ "text": String::from_utf8_lossy(&data), "size": data.len() })
            })
        }
        "fs.write" => {
            let data = match (s(a, "text"), s(a, "base64")) {
                (Some(t), None) => t.as_bytes().to_vec(),
                (None, Some(b)) => base64::engine::general_purpose::STANDARD
                    .decode(b)
                    .map_err(|e| format!("`base64`: {e}"))?,
                _ => return Err("give `text` or `base64`".into()),
            };
            if a.get("mkdirs").and_then(Value::as_bool) != Some(false)
                && let Some(parent) = path.parent()
            {
                std::fs::create_dir_all(parent).map_err(io)?;
            }
            if a.get("append").and_then(Value::as_bool) == Some(true) {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut f| f.write_all(&data))
                    .map_err(io)?;
            } else {
                // Whole files are replaced atomically: readers never see half of one.
                let mut tmp = path.as_os_str().to_owned();
                tmp.push(format!(".{}.tmp", std::process::id()));
                let tmp = PathBuf::from(tmp);
                std::fs::write(&tmp, &data)
                    .and_then(|_| std::fs::rename(&tmp, &path))
                    .map_err(|e| {
                        let _ = std::fs::remove_file(&tmp);
                        io(e)
                    })?;
            }
            Ok(json!({ "path": path, "size": data.len() }))
        }
        "fs.list" => {
            let mut rows = Vec::new();
            for e in std::fs::read_dir(&path).map_err(io)?.flatten() {
                let Ok(m) = e.metadata() else { continue };
                rows.push(json!({
                    "name": e.file_name().to_string_lossy(),
                    "dir": m.is_dir(),
                    "size": m.len(),
                    "modified": mtime(&m),
                }));
            }
            rows.sort_by(|x, y| x["name"].as_str().cmp(&y["name"].as_str()));
            Ok(json!({ "path": path, "entries": rows }))
        }
        "fs.stat" => Ok(match std::fs::metadata(&path) {
            Ok(m) => json!({ "path": path, "exists": true, "dir": m.is_dir(), "size": m.len(), "modified": mtime(&m) }),
            Err(_) => json!({ "path": path, "exists": false }),
        }),
        "fs.delete" => {
            let deleted = match std::fs::symlink_metadata(&path) {
                Err(_) => false,
                Ok(m) if m.is_dir() => {
                    if a.get("recursive").and_then(Value::as_bool) == Some(true) {
                        std::fs::remove_dir_all(&path).map_err(io)?;
                    } else {
                        std::fs::remove_dir(&path).map_err(io)?;
                    }
                    true
                }
                Ok(_) => {
                    std::fs::remove_file(&path).map_err(io)?;
                    true
                }
            };
            Ok(json!({ "path": path, "deleted": deleted }))
        }
        "fs.mkdir" => {
            std::fs::create_dir_all(&path).map_err(io)?;
            Ok(json!({ "path": path }))
        }
        _ => Err(format!("unknown method `{method}`")),
    }
}

fn session_json(s: &Session) -> Value {
    json!({
        "id": s.id.as_str(),
        "title": s.title,
        "folder": s.root,
        "parent": s.parent.as_ref().map(|p| p.as_str().to_owned()),
        "archived": s.archived,
        "repositories": s.worktrees.iter().map(|w| json!({
            "name": w.name,
            "repo": w.repo.to_string(),
            "folder": w.path,
            "branch": w.branch,
            "base": w.base,
        })).collect::<Vec<_>>(),
    })
}

impl UserCtx {
    /// `widget.host`: a widget page (shown in `session`) uses an ide feature. The caller
    /// checked the widget may.
    pub(crate) async fn host_call(self: &Arc<Self>, session: &str, method: &str, args: &str) -> AgentToolResult {
        let a: Value = match serde_json::from_str(if args.trim().is_empty() { "{}" } else { args }) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => return fail("arguments must be a JSON object"),
            Err(e) => return fail(format!("invalid arguments: {e}")),
        };
        let (me, session, method) = (self.clone(), session.to_owned(), method.to_owned());
        // Blocking: processes, files, port scans.
        let r = tokio::task::spawn_blocking(move || {
            let s = me.resolve_session(&session).map_err(|e| format!("session not available: {e}"))?;
            match method.as_str() {
                "exec" => exec(&s, &a),
                m if m.starts_with("fs.") => fs(&s, m, &a),
                m if m.starts_with("term.") => me.host_term(&s, m, &a),
                "sessions.list" => {
                    let all = me.sessions.lock().unwrap();
                    Ok(json!({ "sessions": all.list().iter().map(session_json).collect::<Vec<_>>() }))
                }
                "sessions.get" => match s_opt(&a, "id") {
                    None => Ok(session_json(&s)),
                    Some(id) => me
                        .resolve_session(id)
                        .map(|x| session_json(&x))
                        .map_err(|e| format!("no session `{id}`: {e}")),
                },
                other => Err(format!("unknown method `{other}`")),
            }
        })
        .await;
        match r {
            Ok(Ok(v)) => ok(v),
            Ok(Err(e)) => fail(e),
            Err(_) => fail("the call failed"),
        }
    }

    /// `term.*`: user terminals of any session (by id), opened in the widget's session or
    /// another one.
    fn host_term(&self, s: &Session, method: &str, a: &Value) -> HostResult {
        let id = || required(a, "id").map(str::to_owned);
        let unit = |r: Result<Response, ide_proto::ProtoError>| r.map(|_| json!({})).map_err(|e| e.to_string());
        match method {
            "term.open" => {
                let session = s_opt(a, "session").unwrap_or(s.id.as_str()).to_owned();
                let base = if session == s.id.as_str() {
                    s.clone()
                } else {
                    self.resolve_session(&session).map_err(|e| e.to_string())?
                };
                let info = self
                    .uterm_open(OpenArgs {
                        session,
                        repo: None,
                        folder: None,
                        command: s_opt(a, "command").map(str::to_owned),
                        title: s_opt(a, "title").map(str::to_owned),
                        shell: s_opt(a, "shell").map(str::to_owned),
                        start: None,
                        dir: s_opt(a, "cwd").map(|c| resolve(&base, c)),
                    })
                    .map_err(|e| e.to_string())?;
                let pid = self.uterm_host(&info.id).and_then(|h| h.pid());
                Ok(json!({ "id": info.id, "session": info.session, "title": info.title, "command": info.command, "folder": info.cwd, "pid": pid }))
            }
            "term.list" => {
                let want = s_opt(a, "session").unwrap_or(s.id.as_str()).to_owned();
                let rows: Vec<(String, String, String, String, PathBuf, Arc<crate::pty::TermHost>)> = {
                    let u = self.uterms.lock().unwrap();
                    u.list
                        .iter()
                        .filter(|t| want == "*" || t.session == want)
                        .map(|t| (t.id.clone(), t.session.clone(), t.title.clone(), t.command.clone(), t.cwd.clone(), t.host.clone()))
                        .collect()
                };
                let pids: Vec<u32> = rows.iter().filter_map(|r| r.5.pid()).collect();
                let ports = if a.get("ports").and_then(Value::as_bool) == Some(false) {
                    Vec::new()
                } else {
                    crate::ports::by_root(&pids)
                };
                let list: Vec<Value> = rows
                    .iter()
                    .map(|(id, session, title, command, cwd, host)| {
                        let pid = host.pid();
                        json!({
                            "id": id,
                            "session": session,
                            "title": title,
                            "command": command,
                            "folder": cwd,
                            "state": state_text(host.state()),
                            "running": host.is_running(),
                            "pid": pid,
                            "ports": ports
                                .iter()
                                .filter(|(root, _)| Some(*root) == pid)
                                .map(|(_, p)| json!({ "port": p.port, "process": p.process }))
                                .collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                Ok(json!({ "terminals": list }))
            }
            "term.read" => {
                let id = id()?;
                let (_, host) = self.uterm_find(&id).map_err(|e| e.to_string())?;
                let lines = a.get("lines").and_then(Value::as_u64).map_or(200, |n| n as usize).min(super::terminals::SCROLLBACK);
                let (mut history, _) = host.history_text(lines);
                history.extend(host.screen_text());
                while history.last().is_some_and(|l| l.trim().is_empty()) {
                    history.pop();
                }
                Ok(json!({
                    "id": id,
                    "state": state_text(host.state()),
                    "running": host.is_running(),
                    "text": history.join("\n"),
                }))
            }
            "term.write" => {
                let id = id()?;
                let (_, host) = self.uterm_find(&id).map_err(|e| e.to_string())?;
                let mut bytes = text_bytes(s_opt(a, "text").unwrap_or(""));
                let app_cursor = host.app_cursor();
                for k in key_names(a.get("keys"))? {
                    bytes.extend(key_bytes(&k, app_cursor).ok_or_else(|| format!("unknown key `{k}`"))?);
                }
                host.write(bytes).map_err(|e| e.to_string())?;
                Ok(json!({}))
            }
            "term.stop" => unit(self.handle_uterm(Request::UTermStop { id: id()? })),
            "term.rerun" => unit(self.handle_uterm(Request::UTermRerun { id: id()? })),
            "term.remove" => unit(self.handle_uterm(Request::UTermRemove { id: id()? })),
            other => Err(format!("unknown method `{other}`")),
        }
    }
}

fn s_opt<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    s(a, k).map(str::trim).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(root: &Path) -> Session {
        let mut s: Session = serde_json::from_value(json!({
            "id": "s1", "title": "t", "created_at": 0, "last_active": 0
        }))
        .unwrap();
        s.root = root.to_path_buf();
        s
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ide-whost-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn files() {
        let d = tmp("fs");
        let s = session(&d);
        let w = fs(&s, "fs.write", &json!({ "path": "a/b.txt", "text": "héllo" })).unwrap();
        assert_eq!(w["size"], 6);
        assert_eq!(fs(&s, "fs.read", &json!({ "path": "a/b.txt" })).unwrap()["text"], "héllo");
        fs(&s, "fs.write", &json!({ "path": "a/b.txt", "text": "!", "append": true })).unwrap();
        let b = fs(&s, "fs.read", &json!({ "path": d.join("a/b.txt"), "encoding": "base64" })).unwrap();
        assert_eq!(b["base64"], base64::engine::general_purpose::STANDARD.encode("héllo!"));
        let l = fs(&s, "fs.list", &json!({ "path": "a" })).unwrap();
        assert_eq!(l["entries"][0]["name"], "b.txt");
        assert_eq!(fs(&s, "fs.stat", &json!({ "path": "a" })).unwrap()["dir"], true);
        assert_eq!(fs(&s, "fs.stat", &json!({ "path": "zz" })).unwrap()["exists"], false);
        assert!(fs(&s, "fs.delete", &json!({ "path": "a" })).is_err());
        assert_eq!(fs(&s, "fs.delete", &json!({ "path": "a", "recursive": true })).unwrap()["deleted"], true);
        assert!(fs(&s, "fs.read", &json!({})).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn exec_runs_and_times_out() {
        let d = tmp("exec");
        let s = session(&d);
        let r = exec(&s, &json!({ "command": "echo hi" })).unwrap();
        assert_eq!(r["code"], 0);
        assert_eq!(r["stdout"].as_str().unwrap().trim(), "hi");
        #[cfg(windows)]
        let (prog, args) = ("cmd", json!(["/d", "/c", "more"]));
        #[cfg(not(windows))]
        let (prog, args) = ("cat", json!([]));
        let r = exec(&s, &json!({ "program": prog, "args": args, "stdin": "piped\n" })).unwrap();
        assert!(r["stdout"].as_str().unwrap().contains("piped"), "{r}");
        #[cfg(windows)]
        let slow = "ping -n 5 127.0.0.1";
        #[cfg(not(windows))]
        let slow = "sleep 5";
        let r = exec(&s, &json!({ "command": slow, "timeout_ms": 300 })).unwrap();
        assert_eq!(r["timed_out"], true);
        assert!(exec(&s, &json!({})).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
