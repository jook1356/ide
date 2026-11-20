//! Shared helpers for the server/client integration tests.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ide_client::{Client, ClientEvent, Memory, Notify};
use ide_core::Paths;
use ide_server::{Server, ServerConfig};

/// A fresh directory under `IDE_TEST_TMP` (or the system temp dir).
pub fn tmp_dir(tag: &str) -> PathBuf {
    let base = std::env::var_os("IDE_TEST_TMP")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("ide-server-tests"));
    let d = base.join(format!("{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A git repo with one commit on `main`.
pub fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("README.md"), "hi\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

/// An in-process server for `data`.
pub fn server(data: &Path) -> Arc<Server> {
    Server::new(ServerConfig::for_paths(Paths::new(data)))
}

/// A memory transport to `server`.
pub fn memory(server: &Arc<Server>) -> Memory {
    let server = server.clone();
    Memory::new(move || {
        let (a, b) = ide_proto::memory_pair();
        let s = server.clone();
        let user = s.user().clone();
        tokio::spawn(async move { s.serve(b, user).await });
        Ok(Box::new(a))
    })
}

/// Terminal output of one session as seen by one client.
#[derive(Debug, Default, Clone)]
pub struct TermBuf {
    pub base: u64,
    pub data: Vec<u8>,
    pub resets: usize,
    pub chunks: usize,
}

impl TermBuf {
    pub fn end(&self) -> u64 {
        self.base + self.data.len() as u64
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
    pub fn count(&self, needle: &str) -> usize {
        self.text().matches(needle).count()
    }
}

/// Everything a client received.
#[derive(Debug, Default)]
pub struct Collected {
    pub terms: HashMap<String, TermBuf>,
    pub connected: usize,
    pub disconnected: usize,
    pub notifies: Vec<Notify>,
    /// Contiguity violations (gaps/duplicates); must stay empty.
    pub errors: Vec<String>,
}

pub type Shared = Arc<Mutex<Collected>>;

/// Consume `client`'s events into a [`Collected`]; `delay_ms` (changeable) sleeps after each
/// terminal chunk to simulate a slow client.
pub fn collect(client: &Client, delay_ms: Arc<AtomicU64>) -> Shared {
    let shared: Shared = Arc::default();
    let mut events = client.events().expect("events");
    let s = shared.clone();
    tokio::spawn(async move {
        while let Some(ev) = events.next().await {
            let is_term = matches!(ev, ClientEvent::Term(_));
            {
                let mut c = s.lock().unwrap();
                match ev {
                    ClientEvent::Connected(_) => c.connected += 1,
                    ClientEvent::Disconnected { .. } => c.disconnected += 1,
                    ClientEvent::Notify(n) => c.notifies.push(n),
                    ClientEvent::AttachFailed { session, error } => {
                        c.errors.push(format!("attach {session} failed: {error}"))
                    }
                    ClientEvent::Term(chunk) => {
                        let mut err = None;
                        {
                            let t = c.terms.entry(chunk.session.clone()).or_default();
                            t.chunks += 1;
                            if chunk.reset {
                                t.resets += 1;
                                t.base = chunk.offset;
                                t.data.clear();
                            } else if chunk.offset != t.end() {
                                err = Some(format!(
                                    "{}: chunk at {} but expected {}",
                                    chunk.session,
                                    chunk.offset,
                                    t.end()
                                ));
                            }
                            t.data.extend_from_slice(&chunk.bytes);
                        }
                        if let Some(e) = err {
                            c.errors.push(e);
                        }
                    }
                }
            }
            let d = delay_ms.load(Ordering::Relaxed);
            if is_term && d > 0 {
                tokio::time::sleep(Duration::from_millis(d)).await;
            }
        }
    });
    shared
}

pub fn term(c: &Shared, session: &str) -> TermBuf {
    c.lock()
        .unwrap()
        .terms
        .get(session)
        .cloned()
        .unwrap_or_default()
}

/// Poll `f` until true or `timeout`.
pub async fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < timeout {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    f()
}

/// Python on PATH (for the DSR and flood helpers), if any.
pub fn python() -> Option<PathBuf> {
    ide_core::which("python").or_else(|| ide_core::which("python3"))
}
