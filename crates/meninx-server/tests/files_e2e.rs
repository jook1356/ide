//! Phase 2 end to end through `meninx_client::Client`: files, watch, folder picker, search, git
//! and go-to-definition over the memory transport and a real `meninx-server` child
//! (`--no-daemon`), all scoped to `(session, repo)` worktrees.
//! Run with `MENINX_TEST_TMP=<dir outside any git repository>`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use meninx_client::{
    ChangeKind, Client, ClientOptions, DefResult, DiffAgainst, EntryKind, GitRev, LineKind,
    LocalChild, NewSession, Notify, SearchEvent, SessionInfo, TextSearch,
};
use meninx_core::Paths;
use meninx_server::{Server, ServerConfig};

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
        repo: None,
        query: q.into(),
        regex: false,
        case_sensitive: false,
        whole_word: false,
        globs: Vec::new(),
        include_ignored: false,
        max_results: 0,
    }
}

/// Register `repo` and create a shell session with a worktree of it; returns the session and
/// the repo id.
async fn new_session(c: &Client, repo: &Path) -> (SessionInfo, String) {
    let r = c.register_repo(&repo.display().to_string()).await.unwrap();
    let (s, failures) = c
        .create_session(NewSession {
            provider: "shell".into(),
            account: None,
            configuration: "default".into(),
            repos: vec![r.id.clone()],
            original_repo: None,
            refresh_base: false,
            background: false,
            bases: Default::default(),
            title: "files".into(),
            start: false,
            cols: 80,
            rows: 24,
        })
        .await
        .expect("create session");
    assert!(failures.is_empty(), "{failures:?}");
    (s, r.id)
}

async fn wait_fs_changed(col: &Shared, session: &str, path: &str) -> bool {
    wait_until(Duration::from_secs(10), || {
        col.lock().unwrap().notifies.iter().any(|n| match n {
            Notify::FsChanged {
                session: s,
                paths,
                rescan,
                ..
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
    for cap in ["files", "repos", "search", "git", "def"] {
        assert!(caps.iter().any(|x| x == cap), "{cap}");
    }

    // The folder picker (for registering repos) is not jailed: start list, then any folder.
    let outside = root.parent().unwrap().display().to_string();
    assert!(caps.iter().any(|x| x == "browse-pick"));
    let start = c.browse_pick_list(None).await.unwrap();
    assert_eq!(start.path, "");
    assert!(start.parent.is_none() && !start.entries.is_empty());
    let picked = c.browse_pick_list(Some(&outside)).await.unwrap();
    let root_name = root.file_name().unwrap().to_string_lossy();
    let e = picked.entries.iter().find(|e| e.name == root_name).unwrap();
    assert!(!e.is_git_repo);
    let inside = c.browse_pick_list(Some(&e.path)).await.unwrap();
    assert!(
        inside
            .entries
            .iter()
            .any(|e| e.name == "저장소 repo" && e.is_git_repo)
    );
    assert!(inside.parent.is_some());
    assert_eq!(
        c.browse_pick_list(Some("relative")).await.unwrap_err().id,
        "path-invalid"
    );
    // A plain folder needs `init` (the app asks first); sessions only take registered repos.
    assert_eq!(c.register_repo(&outside).await.unwrap_err().id, "not-a-git-folder");
    let bad = c
        .create_session(NewSession {
            provider: "shell".into(),
            account: None,
            configuration: "default".into(),
            repos: vec!["0badc0de".into()],
            original_repo: None,
            refresh_base: false,
            background: false,
            bases: Default::default(),
            title: String::new(),
            start: false,
            cols: 0,
            rows: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(bad.id, "repo-not-found");

    let (s, rid) = new_session(c, repo).await;
    let sid = s.id.as_str();
    let rp = rid.as_str();
    let wt_path = s.repos[0].path.clone();
    assert!(Path::new(&wt_path).starts_with(&s.root));

    // List (dirs first, .git hidden, ignored only on request) with git badges.
    std::fs::create_dir_all(Path::new(&wt_path).join("target")).unwrap();
    std::fs::write(Path::new(&wt_path).join("new.txt"), "n\n").unwrap();
    let list = c.fs_list(sid, rp, "", false).await.unwrap();
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
    let all = c.fs_list(sid, rp, "", true).await.unwrap();
    assert!(all.iter().any(|e| e.name == "target" && e.ignored));
    assert_eq!(c.fs_list(sid, rp, "src", false).await.unwrap().len(), 2);

    // Read / write with versions and conflicts; Korean names.
    let f = c.fs_read(sid, rp, "한글.txt", 0).await.unwrap();
    assert_eq!(f.text, "첫 줄\n둘째 줄\n");
    assert_eq!(f.encoding.label, "UTF-8");
    let v2 = c
        .fs_write(
            sid,
            rp,
            "한글.txt",
            "첫 줄\n바뀜\n",
            Some(&f.version),
            Some(f.encoding.clone()),
        )
        .await
        .unwrap();
    let e = c
        .fs_write(sid, rp, "한글.txt", "stale", Some(&f.version), None)
        .await
        .unwrap_err();
    assert_eq!(
        (e.id.as_str(), e.get("actual")),
        ("fs-conflict", Some(v2.as_str()))
    );
    assert_eq!(
        c.fs_stat(sid, rp, "한글.txt").await.unwrap().size,
        "첫 줄\n바뀜\n".len() as u64
    );
    c.fs_create_dir(sid, rp, "docs/새 폴더").await.unwrap();
    c.fs_create_file(sid, rp, "docs/새 폴더/a.md", "# a\n")
        .await
        .unwrap();
    assert_eq!(
        c.fs_create_file(sid, rp, "docs/새 폴더/a.md", "")
            .await
            .unwrap_err()
            .id,
        "fs-exists"
    );
    c.fs_rename(sid, rp, "docs/새 폴더/a.md", "docs/b.md")
        .await
        .unwrap();
    assert_eq!(
        c.fs_delete(sid, rp, "docs", false).await.unwrap_err().id,
        "fs-dir-not-empty"
    );
    c.fs_delete(sid, rp, "docs", true).await.unwrap();
    assert_eq!(c.fs_stat(sid, rp, "docs").await.unwrap_err().id, "fs-not-found");
    // Jail.
    for (p, id) in [
        ("../x", "path-denied"),
        ("a/../../x", "path-denied"),
        ("C:/Windows/win.ini", "path-invalid"),
        ("/etc/passwd", "path-invalid"),
        ("\\\\server\\share\\x", "path-invalid"),
    ] {
        assert_eq!(c.fs_read(sid, rp, p, 0).await.unwrap_err().id, id, "{p}");
    }
    assert_eq!(
        c.fs_write(sid, rp, ".git", "x", None, None)
            .await
            .unwrap_err()
            .id,
        "path-denied"
    );
    assert_eq!(
        c.fs_read("nope", rp, "x", 0).await.unwrap_err().id,
        "session-not-found"
    );
    // Jail scope = this session's worktree of this repo: another registered repo that is not
    // attached, an unknown id, and the session folder itself are unreachable.
    let other = root.join("other");
    init_repo(&other);
    let o = c.register_repo(&other.display().to_string()).await.unwrap();
    for repo_id in [o.id.as_str(), "0badc0de", "", "../x"] {
        assert_eq!(
            c.fs_list(sid, repo_id, "", false).await.unwrap_err().id,
            "repo-not-attached",
            "{repo_id:?}"
        );
    }
    assert_eq!(
        c.git_status(sid, &o.id, None).await.unwrap_err().id,
        "repo-not-attached"
    );

    // Watch: an external change arrives as FsChanged; unwatch stops it.
    c.fs_watch(sid, rp).await.unwrap();
    c.fs_watch(sid, rp).await.unwrap(); // idempotent
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(Path::new(&wt_path).join("watched.txt"), "w").unwrap();
    assert!(wait_fs_changed(&col, sid, "watched.txt").await, "FsChanged");
    c.fs_unwatch(sid, rp).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    col.lock().unwrap().notifies.clear();
    std::fs::write(Path::new(&wt_path).join("after.txt"), "a").unwrap();
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
    assert!(hits.iter().all(|h| h.path.starts_with("src/") && h.repo == rid));
    let mut bad = text_query(sid, "(");
    bad.regex = true;
    assert_eq!(c.search_text(bad).await.unwrap_err().id, "search-bad-regex");
    // Cancel: many files, cancel right away.
    for i in 0..300 {
        std::fs::write(
            Path::new(&wt_path).join(format!("bulk{i}.txt")),
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
    let fm = c.search_files(sid, None, "utiljs", 10).await.unwrap();
    assert_eq!((fm[0].repo.as_str(), fm[0].path.as_str()), (rp, "src/util.js"));

    // Git.
    std::fs::write(
        Path::new(&wt_path).join("src/util.js"),
        "function helper() {}\n",
    )
    .unwrap();
    let st = c.git_status(sid, rp, None).await.unwrap();
    assert_eq!(st.branch.as_deref(), Some(s.repos[0].branch.as_str()));
    assert_eq!(st.unstaged[0].path, "src/util.js");
    assert!(st.untracked.iter().any(|p| p == "new.txt"));
    assert_eq!(st.base.as_ref().unwrap().reference, "main");
    let d = c
        .git_diff(sid, rp, Some("src/util.js"), DiffAgainst::WorkingTree, None)
        .await
        .unwrap();
    assert_eq!(d[0].hunks[0].lines.last().unwrap().kind, LineKind::Del);
    let d = c
        .git_diff(sid, rp, Some("new.txt"), DiffAgainst::Head, None)
        .await
        .unwrap();
    assert_eq!(d[0].kind, ChangeKind::Untracked);
    let old = c
        .git_file_at(sid, rp, "src/util.js", GitRev::Head, None)
        .await
        .unwrap()
        .unwrap();
    assert!(old.text.contains("module.exports"));
    assert!(
        c.git_file_at(sid, rp, "new.txt", GitRev::Head, None)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        c.git_file_at(sid, rp, "../x", GitRev::Head, None)
            .await
            .unwrap_err()
            .id,
        "path-denied"
    );

    // History (Hermes Desk's History tab): commit the util.js edit on the session branch.
    git(Path::new(&wt_path), &["commit", "-q", "-m", "history", "--", "src/util.js"]);
    let page = c.git_log(sid, rp, 0, 2).await.unwrap();
    assert_eq!((page.commits.len(), page.has_more), (2, true));
    assert_eq!(page.commits[0].subject, "history");
    let all = c.git_log(sid, rp, 0, 0).await.unwrap();
    assert_eq!(
        all.commits.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(),
        ["history", "files", "init"]
    );
    assert!(!all.has_more);
    let sha = all.commits[0].sha.clone();
    let files = c.git_commit_files(sid, rp, &sha).await.unwrap();
    assert_eq!(
        files.iter().map(|f| (f.path.as_str(), f.kind)).collect::<Vec<_>>(),
        [("src/util.js", ChangeKind::Modified)]
    );
    let root_files = c.git_commit_files(sid, rp, &all.commits[2].sha).await.unwrap();
    assert_eq!(root_files[0].kind, ChangeKind::Added);
    let d = c
        .git_commit_diff(sid, rp, &all.commits[0].short, Some("src/util.js".into()))
        .await
        .unwrap();
    assert_eq!(d[0].hunks[0].lines.last().unwrap().kind, LineKind::Del);
    let d = c.git_commit_diff(sid, rp, &all.commits[1].sha, None).await.unwrap();
    assert_eq!(d.len(), 4);
    assert!(d.iter().all(|f| f.kind == ChangeKind::Added));
    assert_eq!(
        c.git_commit_diff(sid, rp, "-p", None).await.unwrap_err().id,
        "git-bad-rev"
    );
    assert_eq!(
        c.git_commit_files(sid, rp, "HEAD").await.unwrap_err().id,
        "git-bad-rev"
    );
    assert_eq!(
        c.git_commit_diff(sid, rp, &sha, Some("../x".into()))
            .await
            .unwrap_err()
            .id,
        "path-denied"
    );
    // No remote: nothing is marked unpushed.
    assert!(c.git_unpushed(sid, rp).await.unwrap().is_empty());

    // Go to definition (file on disk, then unsaved editor text).
    let r = c.def_lookup(sid, rp, "src/main.js", 2, 2, None).await.unwrap();
    match r {
        DefResult::One(t) => assert_eq!((t.path.as_str(), t.line, t.col), ("src/util.js", 1, 10)),
        other => panic!("{other:?}"),
    }
    let r = c
        .def_lookup(
            sid,
            rp,
            "src/x.js",
            2,
            1,
            Some("function 한글() {}\n한글()".into()),
        )
        .await
        .unwrap();
    assert!(matches!(r, DefResult::One(_)), "{r:?}");
    let r = c
        .def_lookup(sid, rp, "src/x.js", 2, 1, Some("x\n한글()".into()))
        .await
        .unwrap();
    assert!(matches!(r, DefResult::None { reason } if reason.id == "def-not-found"));
    match c.def_lookup(sid, rp, "README.md", 1, 1, None).await.unwrap() {
        DefResult::None { reason } => assert_eq!(reason.id, "def-unsupported-file"),
        other => panic!("{other:?}"),
    }

    c.delete_session(sid, true).await.unwrap();
    assert!(!Path::new(&s.root).exists());
    c.unregister_repo(&o.id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_over_memory() {
    let dir = tmp_dir("p2mem");
    let root = dir.join("roots");
    std::fs::create_dir_all(&root).unwrap();
    let repo = repo(&root);
    let server = Server::new(ServerConfig::for_paths(Paths::new(dir.join("data"))));
    let c = Client::connect(memory(&server), opts()).await.unwrap();
    files_scenario(&c, &repo, &root).await;
    // A second connection's watch is released when it disconnects.
    let sessions = c.sessions().await.unwrap();
    assert!(sessions.is_empty());
    let c2 = Client::connect(memory(&server), opts()).await.unwrap();
    let (s, r) = new_session(&c2, &repo).await;
    c2.fs_watch(&s.id, &r).await.unwrap();
    let ctx = server.authorize(server.user()).unwrap();
    assert_eq!(ctx.watching(&s.id, &r), 1);
    c.fs_watch(&s.id, &r).await.unwrap();
    assert_eq!(ctx.watching(&s.id, &r), 2);
    drop(c2);
    assert!(wait_until(Duration::from_secs(5), || ctx.watching(&s.id, &r) == 1).await);
    c.fs_unwatch(&s.id, &r).await.unwrap();
    assert_eq!(ctx.watching(&s.id, &r), 0);
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
        data.join(meninx_server::SERVER_CONFIG_FILE),
        serde_json::json!({ "browse_roots": [root.display().to_string()] }).to_string(),
    )
    .unwrap();
    // Legacy browse roots: the migration registers the ones that are git repositories (the
    // roots folder itself is none, the repo inside it is not a root).
    let t = LocalChild::new(env!("CARGO_BIN_EXE_meninx-server"))
        .no_daemon()
        .env("MENINX_DATA_DIR", data.display().to_string())
        .env("USERPROFILE", root.display().to_string())
        .env("HOME", root.display().to_string());
    let c = Client::connect(t, opts()).await.expect("connect child");
    assert!(c.repos().await.unwrap().is_empty());
    files_scenario(&c, &repo, &root).await;
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}
