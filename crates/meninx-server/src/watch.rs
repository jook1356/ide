//! `fs.watch`: one debounced `notify` watcher per watched (session, repo) worktree, shared by
//! every connection watching it and dropped when the last one unwatches (or disconnects).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meninx_proto::{Notify, ProtoError};
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::broadcast;

use crate::jail::RootJail;

/// Quiet time that ends a batch.
const QUIET: Duration = Duration::from_millis(80);
/// Longest time a batch is held back.
const MAX_DELAY: Duration = Duration::from_millis(400);
/// More changed paths than this in one batch are sent as `rescan`.
const MAX_PATHS: usize = 1000;

enum Raw {
    Fs(Vec<PathBuf>),
    Git,
    Rescan,
}

struct Entry {
    refs: usize,
    tx: broadcast::Sender<Notify>,
    /// Cleared on release: events still in flight from the stopped watcher are dropped.
    alive: Arc<AtomicBool>,
    _watchers: Vec<notify::RecommendedWatcher>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

/// The watch key of a session's repo worktree.
pub fn key(session: &str, repo: &str) -> String {
    format!("{session}:{repo}")
}

/// Watchers of one user, keyed by [`key`].
#[derive(Default)]
pub struct Watchers {
    map: Mutex<HashMap<String, Entry>>,
}

impl Watchers {
    /// Start (or join) watching the worktree of `repo` in `session`, rooted at `jail`;
    /// `git_dir` = the per-worktree git dir (watched for index/HEAD changes when it lies
    /// outside the root).
    pub fn acquire(
        &self,
        session: &str,
        repo: &str,
        jail: &RootJail,
        git_dir: Option<PathBuf>,
    ) -> Result<broadcast::Receiver<Notify>, ProtoError> {
        let k = key(session, repo);
        let mut map = self.map.lock().unwrap();
        if let Some(e) = map.get_mut(&k) {
            e.refs += 1;
            return Ok(e.tx.subscribe());
        }
        let (tx, rx) = broadcast::channel(256);
        let alive = Arc::new(AtomicBool::new(true));
        let watchers = start(session, repo, jail, git_dir, tx.clone(), alive.clone())?;
        map.insert(
            k,
            Entry {
                refs: 1,
                tx,
                alive,
                _watchers: watchers,
            },
        );
        Ok(rx)
    }

    /// Drop one reference; the watcher stops with the last one.
    pub fn release(&self, session: &str) {
        let mut map = self.map.lock().unwrap();
        if let Some(e) = map.get_mut(session) {
            e.refs -= 1;
            if e.refs == 0 {
                map.remove(session);
            }
        }
    }

    /// Number of connections watching `session` (tests, diagnostics).
    pub fn watching(&self, session: &str) -> usize {
        self.map.lock().unwrap().get(session).map_or(0, |e| e.refs)
    }

    /// Stop watching every worktree of `session` (session deleted).
    pub fn remove(&self, session: &str) {
        let prefix = format!("{session}:");
        self.map
            .lock()
            .unwrap()
            .retain(|k, _| !k.starts_with(&prefix));
    }

    /// Stop watching one worktree (detached).
    pub fn remove_key(&self, key: &str) {
        self.map.lock().unwrap().remove(key);
    }
}

fn start(
    session: &str,
    repo: &str,
    jail: &RootJail,
    git_dir: Option<PathBuf>,
    tx: broadcast::Sender<Notify>,
    alive: Arc<AtomicBool>,
) -> Result<Vec<notify::RecommendedWatcher>, ProtoError> {
    let fail = |e: notify::Error| ProtoError::new("watch-failed").arg("detail", e.to_string());
    let (raw_tx, raw_rx) = mpsc::channel::<Raw>();
    let root = jail.root().to_path_buf();
    let mut watchers = Vec::new();

    let t = raw_tx.clone();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let _ = match res {
            Ok(ev) if matches!(ev.kind, EventKind::Access(_)) => Ok(()),
            Ok(ev) if ev.need_rescan() => t.send(Raw::Rescan),
            Ok(ev) => t.send(Raw::Fs(ev.paths)),
            Err(_) => t.send(Raw::Rescan),
        };
    })
    .map_err(fail)?;
    w.watch(&root, RecursiveMode::Recursive).map_err(fail)?;
    watchers.push(w);

    if let Some(gd) = git_dir.filter(|g| !g.starts_with(&root) && g.is_dir()) {
        let t = raw_tx.clone();
        let mut g = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res
                && !matches!(ev.kind, EventKind::Access(_))
            {
                let _ = t.send(Raw::Git);
            }
        })
        .map_err(fail)?;
        // Index, HEAD and logs of a linked worktree live here (small, recursive is fine).
        if g.watch(&gd, RecursiveMode::Recursive).is_ok() {
            watchers.push(g);
        }
    }
    drop(raw_tx);

    let jail = jail.clone();
    let session = session.to_owned();
    let repo = repo.to_owned();
    std::thread::Builder::new()
        .name(format!("watch-{session}-{repo}"))
        .spawn(move || debounce(&session, &repo, &jail, raw_rx, tx, &alive))
        .map_err(|e| ProtoError::new("watch-failed").arg("detail", e.to_string()))?;
    Ok(watchers)
}

/// Batch raw events until [`QUIET`] passes without one (at most [`MAX_DELAY`]), then send
/// `FsChanged` (root-relative, deduplicated, `.git` excluded) and/or `GitChanged`. Ends when
/// the watchers are dropped.
fn debounce(
    session: &str,
    repo: &str,
    jail: &RootJail,
    rx: mpsc::Receiver<Raw>,
    tx: broadcast::Sender<Notify>,
    alive: &AtomicBool,
) {
    while let Ok(first) = rx.recv() {
        let start = Instant::now();
        let mut paths = BTreeSet::new();
        let mut git = false;
        let mut rescan = false;
        let mut add = |r: Raw, paths: &mut BTreeSet<String>| match r {
            Raw::Rescan => rescan = true,
            Raw::Git => git = true,
            Raw::Fs(ps) => {
                for p in ps {
                    match jail.rel_of(&p) {
                        Some(rel) if is_git_path(&rel) => git = true,
                        Some(rel) if !rel.is_empty() => {
                            paths.insert(rel);
                        }
                        _ => {}
                    }
                }
            }
        };
        add(first, &mut paths);
        loop {
            let left = MAX_DELAY.saturating_sub(start.elapsed());
            if left.is_zero() {
                break;
            }
            match rx.recv_timeout(QUIET.min(left)) {
                Ok(r) => add(r, &mut paths),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if !alive.load(Ordering::Acquire) {
            break;
        }
        if rescan || paths.len() > MAX_PATHS {
            let _ = tx.send(Notify::FsChanged {
                session: session.to_owned(),
                repo: repo.to_owned(),
                paths: Vec::new(),
                rescan: true,
            });
        } else if !paths.is_empty() {
            let _ = tx.send(Notify::FsChanged {
                session: session.to_owned(),
                repo: repo.to_owned(),
                paths: paths.into_iter().collect(),
                rescan: false,
            });
        }
        if git {
            let _ = tx.send(Notify::GitChanged {
                session: session.to_owned(),
                repo: repo.to_owned(),
            });
        }
    }
    tracing::debug!("watcher for {session} stopped");
}

fn is_git_path(rel: &str) -> bool {
    rel.split('/').any(|s| s.eq_ignore_ascii_case(".git"))
}

/// The per-worktree git dir of `root` (for [`Watchers::acquire`]).
pub fn git_dir_of(root: &Path) -> Option<PathBuf> {
    crate::fsops::per_worktree_git_dir(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jail::testutil::*;

    fn next(rx: &mut broadcast::Receiver<Notify>, within: Duration) -> Option<Notify> {
        let end = Instant::now() + within;
        while Instant::now() < end {
            match rx.try_recv() {
                Ok(n) => return Some(n),
                Err(broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => return None,
            }
        }
        None
    }

    /// Wait until an `FsChanged` mentions `path` (other batches may come first).
    fn wait_path(rx: &mut broadcast::Receiver<Notify>, path: &str) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if let Some(Notify::FsChanged { paths, rescan, .. }) = next(rx, Duration::from_secs(1))
                && (rescan || paths.iter().any(|p| p == path))
            {
                return true;
            }
        }
        false
    }

    #[test]
    fn notifications_and_refcount() {
        let t = tmp("watch");
        let repo = t.join("repo");
        init_repo(&repo, &[("a.txt", "a")]);
        let wt = t.join("wt 작업");
        git(
            &repo,
            &["worktree", "add", "-q", "-b", "meninx/w", wt.to_str().unwrap()],
        );
        let jail = RootJail::new(&wt).unwrap();
        let w = Watchers::default();
        let mut rx = w.acquire("s", "r", &jail, git_dir_of(jail.root())).unwrap();
        let mut rx2 = w.acquire("s", "r", &jail, git_dir_of(jail.root())).unwrap();
        assert_eq!(w.watching(&key("s", "r")), 2);
        std::thread::sleep(Duration::from_millis(200));

        std::fs::create_dir_all(wt.join("새 폴더")).unwrap();
        std::fs::write(wt.join("새 폴더/파일.txt"), "x").unwrap();
        assert!(wait_path(&mut rx, "새 폴더/파일.txt"));
        assert!(wait_path(&mut rx2, "새 폴더/파일.txt"));
        // A burst becomes few batches with every path.
        for i in 0..50 {
            std::fs::write(wt.join(format!("f{i}.txt")), "x").unwrap();
        }
        let mut seen = BTreeSet::new();
        let end = Instant::now() + Duration::from_secs(10);
        let mut batches = 0;
        while seen.len() < 50 && Instant::now() < end {
            if let Some(Notify::FsChanged { paths, .. }) = next(&mut rx, Duration::from_secs(1)) {
                batches += 1;
                seen.extend(paths.into_iter().filter(|p| p.starts_with('f')));
            }
        }
        assert_eq!(seen.len(), 50);
        assert!(batches < 50, "debounced into {batches} batches");
        // Staging changes the index in the linked worktree's git dir: GitChanged.
        git(&wt, &["add", "f1.txt"]);
        let end = Instant::now() + Duration::from_secs(10);
        let mut got_git = false;
        while !got_git && Instant::now() < end {
            got_git = matches!(
                next(&mut rx, Duration::from_secs(1)),
                Some(Notify::GitChanged { .. })
            );
        }
        assert!(got_git);

        w.release(&key("s", "r"));
        assert_eq!(w.watching(&key("s", "r")), 1);
        w.release(&key("s", "r"));
        assert_eq!(w.watching(&key("s", "r")), 0);
        // Released: the channel closes once the watcher thread ends.
        std::fs::write(wt.join("after.txt"), "x").unwrap();
        std::thread::sleep(Duration::from_millis(800));
        let mut closed = false;
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            match rx.try_recv() {
                Err(broadcast::error::TryRecvError::Closed) => {
                    closed = true;
                    break;
                }
                Ok(Notify::FsChanged { paths, .. }) => {
                    assert!(!paths.contains(&"after.txt".to_string()));
                }
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        assert!(closed, "watcher released");
        let _ = std::fs::remove_dir_all(&t);
    }
}
