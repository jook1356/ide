//! Phase 2 end to end through `ide_client::Client`: files, watch, browse, search, git and
//! go-to-definition over the memory transport and a real `ide-server` child (`--no-daemon`).
//! Run with `IDE_TEST_TMP=<dir inside the workspace>`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use ide_client::{
    Agent, ChangeKind, Client, ClientOptions, DefResult, DiffAgainst, EntryKind, GitRev, LineKind,
    LocalChild, NewSession, Notify, SearchEvent, SessionInfo, TextSearch,
};
use ide_core::Paths;
use ide_server::{Server, ServerConfig};

const T: Duration = Duration::from_secs(30);

fn opts() -> ClientOptions {
    ClientOptions {
        backoff_min: Duration::from_millis(50),
        backoff_max: Duration::from_millis(500),
        ..ClientOptions::default()
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Repo with a few source files and a `.gitignore`.
fn repo(dir: &Path) -> PathBuf {
    let r = dir.join("저장소 repo");
    init_repo(&r);
    git(&r, &["config", "core.autocrlf", "false"]);
    std::fs::write(r.join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(r.join("src")).unwrap();
    std::fs::write(
        r.join("src/util.js"),
        "function helper() {}\nmodule.exports = { helper }\n",
    )
    .unwrap();
    std::fs::write(
        r.join("src/main.js"),
        "const { helper } = require('./util')\nhelper()\n",
    )
    .unwrap();
    std::fs::write(r.join("한글.txt"), "첫 줄\n둘째 줄\n").unwrap();
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "files"]);
    r
}

fn text_query(session: &str, q: &str) -> TextSearch {
    TextSearch {
        session: session.into(),
        query: q.into(),
        regex: false,
        case_sensitive: false,
        whole_word: false,
        globs: Vec::new(),
        include_ignored: false,
        max_results: 0,
    }
}

async fn new_session(c: &Client, repo: &Path) -> SessionInfo {
    c.create_session(NewSession {
        profile: "default".into(),
        repo_path: repo.display().to_string(),
        agent: Agent::Shell,
        title: "files".into(),
        base_ref: String::new(),
        start: false,
        cols: 80,
        rows: 24,
    })
    .await
    .expect("create session")
}

async fn wait_fs_changed(col: &Shared, session: &str, path: &str) -> bool {
    wait_until(Duration::from_secs(10), || {
        col.lock().unwrap().notifies.iter().any(|n| match n {
            Notify::FsChanged {
                session: s,
                paths,
                rescan,
            } => s == session && (*rescan || paths.iter().any(|p| p == path)),
            _ => false,
        })
    })
    .await
}

/// The shared scenario: every phase 2 RPC through `c`.
async fn files_scenario(c: &Client, repo: &Path, root: &Path) {
    let col = collect(c, Arc::default());
    let caps = c.welcome().unwrap().capabilities;
    for cap in ["files", "browse", "search", "git", "def"] {
        assert!(caps.iter().any(|x| x == cap), "{cap}");
    }

    // Browse roots + repo picker; a repo outside the roots is refused.
    let roots = c.browse_roots().await.unwrap();
    assert_eq!(roots.len(), 1);
    let l = c.browse_list("").await.unwrap();
    let e = l.entries.iter().find(|e| e.name == "저장소 repo").unwrap();
    assert!(e.is_git_repo);
    assert_eq!(c.browse_list(&e.path).await.unwrap().entries[0].name, "src");
    let outside = root.parent().unwrap().display().to_string();
    assert_eq!(c.browse_list(&outside).await.unwrap_err().id, "path-denied");
    let bad = c
        .create_session(NewSession {
            profile: "default".into(),
            repo_path: outside.clone(),
            agent: Agent::Shell,
            title: String::new(),
            base_ref: String::new(),
            start: false,
            cols: 0,
            rows: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(bad.id, "path-denied");

    let s = new_session(c, repo).await;
    let sid = s.id.as_str();

    // List (dirs first, .git hidden, ignored only on request) with git badges.
    std::fs::create_dir_all(Path::new(&s.worktree_path).join("target")).unwrap();
    std::fs::write(Path::new(&s.worktree_path).join("new.txt"), "n\n").unwrap();
    let list = c.fs_list(sid, "", false).await.unwrap();
    let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["src", ".gitignore", "new.txt", "README.md", "한글.txt"]
    );
    assert_eq!(list[0].kind, EntryKind::Dir);
    assert_eq!(
        list.iter().find(|e| e.name == "new.txt").unwrap().git,
        Some(ChangeKind::Untracked)
    );
    let all = c.fs_list(sid, "", true).await.unwrap();
    assert!(all.iter().any(|e| e.name == "target" && e.ignored));
    assert_eq!(c.fs_list(sid, "src", false).await.unwrap().len(), 2);

    // Read / write with versions and conflicts; Korean names.
    let f = c.fs_read(sid, "한글.txt", 0).await.unwrap();
    assert_eq!(f.text, "첫 줄\n둘째 줄\n");
    assert_eq!(f.encoding.label, "UTF-8");
    let v2 = c
        .fs_write(
            sid,
            "한글.txt",
            "첫 줄\n바뀜\n",
            Some(&f.version),
            Some(f.encoding.clone()),
        )
        .await
        .unwrap();
    let e = c
        .fs_write(sid, "한글.txt", "stale", Some(&f.version), None)
        .await
        .unwrap_err();
    assert_eq!(
        (e.id.as_str(), e.get("actual")),
        ("fs-conflict", Some(v2.as_str()))
    );
    assert_eq!(
        c.fs_stat(sid, "한글.txt").await.unwrap().size,
        "첫 줄\n바뀜\n".len() as u64
    );
    c.fs_create_dir(sid, "docs/새 폴더").await.unwrap();
    c.fs_create_file(sid, "docs/새 폴더/a.md", "# a\n")
        .await
        .unwrap();
    assert_eq!(
        c.fs_create_file(sid, "docs/새 폴더/a.md", "")
            .await
            .unwrap_err()
            .id,
        "fs-exists"
    );
    c.fs_rename(sid, "docs/새 폴더/a.md", "docs/b.md")
        .await
        .unwrap();
    assert_eq!(
        c.fs_delete(sid, "docs", false).await.unwrap_err().id,
        "fs-dir-not-empty"
    );
    c.fs_delete(sid, "docs", true).await.unwrap();
    assert_eq!(c.fs_stat(sid, "docs").await.unwrap_err().id, "fs-not-found");
    // Jail.
    for (p, id) in [
        ("../x", "path-denied"),
        ("a/../../x", "path-denied"),
        ("C:/Windows/win.ini", "path-invalid"),
        ("/etc/passwd", "path-invalid"),
        ("\\\\server\\share\\x", "path-invalid"),
    ] {
        assert_eq!(c.fs_read(sid, p, 0).await.unwrap_err().id, id, "{p}");
    }
    assert_eq!(
        c.fs_write(sid, ".git", "x", None, None)
            .await
            .unwrap_err()
            .id,
        "path-denied"
    );
    assert_eq!(
        c.fs_read("nope", "x", 0).await.unwrap_err().id,
        "session-not-found"
    );

    // Watch: an external change arrives as FsChanged; unwatch stops it.
    c.fs_watch(sid).await.unwrap();
    c.fs_watch(sid).await.unwrap(); // idempotent
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(Path::new(&s.worktree_path).join("watched.txt"), "w").unwrap();
    assert!(wait_fs_changed(&col, sid, "watched.txt").await, "FsChanged");
    c.fs_unwatch(sid).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    col.lock().unwrap().notifies.clear();
    std::fs::write(Path::new(&s.worktree_path).join("after.txt"), "a").unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        !col.lock()
            .unwrap()
            .notifies
            .iter()
            .any(|n| matches!(n, Notify::FsChanged { .. })),
        "no events after unwatch"
    );

    // Text search (streamed) and file-name search.
    let (hits, sum) = c
        .search_text(text_query(sid, "helper"))
        .await
        .unwrap()
        .collect()
        .await;
    let sum = sum.expect("done");
    assert_eq!((sum.matches, sum.files), (4, 2));
    assert!(hits.iter().all(|h| h.path.starts_with("src/")));
    let mut bad = text_query(sid, "(");
    bad.regex = true;
    assert_eq!(c.search_text(bad).await.unwrap_err().id, "search-bad-regex");
    // Cancel: many files, cancel right away.
    for i in 0..300 {
        std::fs::write(
            Path::new(&s.worktree_path).join(format!("bulk{i}.txt")),
            "needle\n".repeat(50),
        )
        .unwrap();
    }
    let mut q = text_query(sid, "needle");
    q.max_results = 100_000;
    let mut st = c.search_text(q).await.unwrap();
    st.cancel();
    let mut done = None;
    while let Some(e) = tokio::time::timeout(T, st.next()).await.unwrap() {
        if let SearchEvent::Done(s) = e {
            done = Some(s);
        }
    }
    let done = done.expect("done after cancel");
    assert!(done.cancelled || done.matches == 15_000, "{done:?}");
    let fm = c.search_files(sid, "utiljs", 10).await.unwrap();
    assert_eq!(fm[0].path, "src/util.js");

    // Git.
    std::fs::write(
        Path::new(&s.worktree_path).join("src/util.js"),
        "function helper() {}\n",
    )
    .unwrap();
    let st = c.git_status(sid, None).await.unwrap();
    assert_eq!(st.branch.as_deref(), Some(s.branch.as_str()));
    assert_eq!(st.unstaged[0].path, "src/util.js");
    assert!(st.untracked.iter().any(|p| p == "new.txt"));
    assert_eq!(st.base.as_ref().unwrap().reference, "main");
    let d = c
        .git_diff(sid, Some("src/util.js"), DiffAgainst::WorkingTree, None)
        .await
        .unwrap();
    assert_eq!(d[0].hunks[0].lines.last().unwrap().kind, LineKind::Del);
    let d = c
        .git_diff(sid, Some("new.txt"), DiffAgainst::Head, None)
        .await
        .unwrap();
    assert_eq!(d[0].kind, ChangeKind::Untracked);
    let old = c
        .git_file_at(sid, "src/util.js", GitRev::Head, None)
        .await
        .unwrap()
        .unwrap();
    assert!(old.text.contains("module.exports"));
    assert!(
        c.git_file_at(sid, "new.txt", GitRev::Head, None)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        c.git_file_at(sid, "../x", GitRev::Head, None)
            .await
            .unwrap_err()
            .id,
        "path-denied"
    );

    // Go to definition (file on disk, then unsaved editor text).
    let r = c.def_lookup(sid, "src/main.js", 2, 2, None).await.unwrap();
    match r {
        DefResult::One(t) => assert_eq!((t.path.as_str(), t.line, t.col), ("src/util.js", 1, 10)),
        other => panic!("{other:?}"),
    }
    let r = c
        .def_lookup(
            sid,
            "src/x.js",
            2,
            1,
            Some("function 한글() {}\n한글()".into()),
        )
        .await
        .unwrap();
    assert!(matches!(r, DefResult::One(_)), "{r:?}");
    let r = c
        .def_lookup(sid, "src/x.js", 2, 1, Some("x\n한글()".into()))
        .await
        .unwrap();
    assert!(matches!(r, DefResult::None { reason } if reason.id == "def-not-found"));
    match c.def_lookup(sid, "README.md", 1, 1, None).await.unwrap() {
        DefResult::None { reason } => assert_eq!(reason.id, "def-unsupported-file"),
        other => panic!("{other:?}"),
    }

    c.delete_session(sid, true).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_over_memory() {
    let dir = tmp_dir("p2mem");
    let root = dir.join("roots");
    std::fs::create_dir_all(&root).unwrap();
    let repo = repo(&root);
    let mut cfg = ServerConfig::for_paths(Paths::new(dir.join("data")));
    cfg.browse_roots = Some(vec![root.clone()]);
    let server = Server::new(cfg);
    let c = Client::connect(memory(&server), opts()).await.unwrap();
    files_scenario(&c, &repo, &root).await;
    // A second connection's watch is released when it disconnects.
    let sessions = c.sessions().await.unwrap();
    assert!(sessions.is_empty());
    let c2 = Client::connect(memory(&server), opts()).await.unwrap();
    let s = new_session(&c2, &repo).await;
    c2.fs_watch(&s.id).await.unwrap();
    let ctx = server.authorize(server.user()).unwrap();
    assert_eq!(ctx.watching(&s.id), 1);
    c.fs_watch(&s.id).await.unwrap();
    assert_eq!(ctx.watching(&s.id), 2);
    drop(c2);
    assert!(wait_until(Duration::from_secs(5), || ctx.watching(&s.id) == 1).await);
    c.fs_unwatch(&s.id).await.unwrap();
    assert_eq!(ctx.watching(&s.id), 0);
    c.delete_session(&s.id, true).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_over_real_child() {
    let dir = tmp_dir("p2child");
    let root = dir.join("roots");
    std::fs::create_dir_all(&root).unwrap();
    let repo = repo(&root);
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join(ide_server::SERVER_CONFIG_FILE),
        serde_json::json!({ "browse_roots": [root.display().to_string()] }).to_string(),
    )
    .unwrap();
    // The home dir is a browse root too; the scenario expects exactly one root, so point
    // the child's home at the roots folder.
    let t = LocalChild::new(env!("CARGO_BIN_EXE_ide-server"))
        .no_daemon()
        .env("IDE_DATA_DIR", data.display().to_string())
        .env("USERPROFILE", root.display().to_string())
        .env("HOME", root.display().to_string());
    let c = Client::connect(t, opts()).await.expect("connect child");
    files_scenario(&c, &repo, &root).await;
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}
