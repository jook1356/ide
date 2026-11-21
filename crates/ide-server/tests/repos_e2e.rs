//! Repository registry and multi-repo session workspaces through the client (memory
//! transport): register / list / unregister, configuration default repos, a session with two
//! worktrees, attach / detach, the jail scope, delete cleanup, and the migration of a
//! single-repo session written by the old layout.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use common::*;
use ide_client::{Client, ClientOptions, NewSession, Notify, SessionMode};

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    !git_out(repo, &["branch", "--list", branch]).is_empty()
}

fn shell(repos: Vec<String>) -> NewSession {
    NewSession {
        provider: "shell".into(),
        account: None,
        configuration: "default".into(),
        repos,
        original_repo: None,
        refresh_base: false,
        bases: Default::default(),
        title: String::new(),
        start: false,
        cols: 80,
        rows: 24,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registry_workspaces_attach_detach_delete() {
    let dir = tmp_dir("repos-e2e");
    let a = dir.join("src").join("app");
    let b = dir.join("src").join("라이브러리");
    let extra = dir.join("src").join("extra");
    for r in [&a, &b, &extra] {
        init_repo(r);
    }
    let server = server(&dir.join("data"));
    let c = Client::connect(memory(&server), ClientOptions::default())
        .await
        .unwrap();
    let col = collect(&c, Arc::default());

    // Register (a subfolder resolves to the top level; twice = the same entry).
    std::fs::create_dir_all(a.join("sub")).unwrap();
    let ra = c.register_repo(&a.join("sub").display().to_string()).await.unwrap();
    assert_eq!(c.register_repo(&a.display().to_string()).await.unwrap().id, ra.id);
    let rb = c.register_repo(&b.display().to_string()).await.unwrap();
    let rx = c.register_repo(&extra.display().to_string()).await.unwrap();
    assert_eq!((ra.name.as_str(), ra.base.as_str()), ("app", "main"));
    assert_eq!(rb.name, "라이브러리");
    let list = c.repos().await.unwrap();
    assert_eq!(list.len(), 3);
    assert!(wait_until(std::time::Duration::from_secs(5), || {
        col.lock()
            .unwrap()
            .notifies
            .iter()
            .any(|n| matches!(n, Notify::ReposChanged))
    })
    .await);

    // Configuration default repos round trip (None = all; the dialog pre-checks them).
    let mut cfg = c.configurations().await.unwrap().remove(0);
    assert_eq!(cfg.default_repos, None);
    cfg.default_repos = Some(vec![ra.id.clone(), rb.id.clone()]);
    let cfg = c.update_configuration(cfg).await.unwrap();
    assert_eq!(cfg.default_repos.as_deref().unwrap(), [ra.id.clone(), rb.id.clone()]);

    // A session with both default repos: one worktree each under the session folder.
    let (s, failures) = c
        .create_session(shell(cfg.default_repos.clone().unwrap()))
        .await
        .unwrap();
    assert!(failures.is_empty());
    assert_eq!(s.mode, SessionMode::Workspace);
    let root = PathBuf::from(&s.root);
    assert!(root.starts_with(dir.join("data").join("workspaces")));
    let folders: Vec<&str> = s.repos.iter().map(|r| r.folder.as_str()).collect();
    assert_eq!(folders, ["app", "라이브러리"]);
    let branch = format!("ide/{}", s.id);
    for r in &s.repos {
        assert_eq!(PathBuf::from(&r.path), root.join(&r.folder));
        assert_eq!(r.branch, branch);
        assert_eq!(r.base, "main");
        assert!(Path::new(&r.path).join("README.md").is_file());
    }
    let counts = c.repos().await.unwrap();
    assert_eq!(counts.iter().find(|r| r.id == ra.id).unwrap().sessions, 1);
    assert_eq!(c.unregister_repo(&ra.id).await.unwrap_err().id, "repo-in-use");

    // Explorer/changes per repo; the jail stops at the worktree of an attached repo.
    let la = c.fs_list(&s.id, &ra.id, "", false).await.unwrap();
    assert!(la.iter().any(|e| e.name == "README.md"));
    std::fs::write(Path::new(&s.repos[1].path).join("new.txt"), "x").unwrap();
    let st = c.git_status(&s.id, &rb.id, None).await.unwrap();
    assert_eq!(st.untracked, ["new.txt"]);
    assert!(c.git_status(&s.id, &ra.id, None).await.unwrap().untracked.is_empty());
    assert_eq!(
        c.fs_list(&s.id, &rx.id, "", false).await.unwrap_err().id,
        "repo-not-attached"
    );
    assert_eq!(
        c.fs_read(&s.id, &ra.id, "../라이브러리/new.txt", 0)
            .await
            .unwrap_err()
            .id,
        "path-denied"
    );
    // Search covers every repo, grouped by repo id.
    let mut q = ide_client::TextSearch {
        session: s.id.clone(),
        repo: None,
        query: "hi".into(),
        regex: false,
        case_sensitive: false,
        whole_word: true,
        globs: Vec::new(),
        include_ignored: false,
        max_results: 0,
    };
    let (hits, _) = c.search_text(q.clone()).await.unwrap().collect().await;
    let mut repos: Vec<&str> = hits.iter().map(|h| h.repo.as_str()).collect();
    repos.dedup();
    assert_eq!(repos, [ra.id.as_str(), rb.id.as_str()]);
    q.repo = Some(rb.id.clone());
    let (hits, _) = c.search_text(q).await.unwrap().collect().await;
    assert!(hits.iter().all(|h| h.repo == rb.id));

    // Attach a third repo, detach it again (worktree gone, branch kept), attach again.
    let s2 = c.attach_repo(&s.id, &rx.id).await.unwrap();
    assert_eq!(s2.repos.len(), 3);
    let xpath = PathBuf::from(&s2.repos[2].path);
    assert!(xpath.join("README.md").is_file());
    assert_eq!(c.attach_repo(&s.id, &rx.id).await.unwrap().repos.len(), 3);
    let s3 = c.detach_repo(&s.id, &rx.id).await.unwrap();
    assert_eq!(s3.repos.len(), 2);
    assert!(!xpath.exists());
    assert!(branch_exists(&extra, &branch));
    assert_eq!(
        c.detach_repo(&s.id, &rx.id).await.unwrap_err().id,
        "repo-not-attached"
    );
    assert_eq!(c.attach_repo(&s.id, &rx.id).await.unwrap().repos.len(), 3);

    // Original-repo sessions run in the repo itself and cannot attach.
    let (o, _) = c
        .create_session(NewSession {
            original_repo: Some(rx.id.clone()),
            refresh_base: false,
            bases: Default::default(),
            ..shell(Vec::new())
        })
        .await
        .unwrap();
    assert_eq!(o.mode, SessionMode::OriginalRepo);
    assert!(ide_core::same_path(Path::new(&o.root), &extra));
    assert_eq!(
        c.attach_repo(&o.id, &ra.id).await.unwrap_err().id,
        "not-a-workspace-session"
    );
    c.delete_session(&o.id, true).await.unwrap();
    assert!(extra.join("README.md").is_file());

    // Delete: every worktree, branch and the folder go; the originals stay.
    c.delete_session(&s.id, true).await.unwrap();
    assert!(!root.exists());
    for r in [&a, &b, &extra] {
        assert!(!branch_exists(r, &branch), "{}", r.display());
        assert_eq!(git_out(r, &["worktree", "list"]).lines().count(), 1);
        assert!(r.join("README.md").is_file());
    }
    c.unregister_repo(&ra.id).await.unwrap();
    assert_eq!(c.repos().await.unwrap().len(), 2);
    // An empty workspace (no repos) is allowed.
    let (e, _) = c.create_session(shell(Vec::new())).await.unwrap();
    assert!(e.repos.is_empty() && Path::new(&e.root).is_dir());
    c.delete_session(&e.id, true).await.unwrap();
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A single-repo session written by the old layout (`worktrees/<slug>/<id>`) is migrated on
/// server start: its repo gets registered, the worktree stays where it is and becomes the
/// session's one attached repo, the agent cwd becomes a new session folder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_repo_session_migration() {
    let dir = tmp_dir("repos-migrate");
    let repo = dir.join("repo");
    init_repo(&repo);
    let data = dir.join("data");
    let wt = data.join("worktrees").join("repo-1234").join("old1");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    let out = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["worktree", "add", "-q", "-b", "ide/old1"])
        .arg(&wt)
        .output()
        .unwrap();
    assert!(out.status.success());
    std::fs::write(
        data.join("sessions.json"),
        serde_json::json!({ "sessions": [{
            "id": "old1", "title": "old", "provider": "shell", "configuration": "default",
            "repo_path": repo, "worktree_path": wt, "branch": "ide/old1",
            "created_at": 1, "last_active": 1
        }]})
        .to_string(),
    )
    .unwrap();
    let server = server(&data);
    let c = Client::connect(memory(&server), ClientOptions::default())
        .await
        .unwrap();
    let repos = c.repos().await.unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!((repos[0].name.as_str(), repos[0].sessions), ("repo", 1));
    let s = &c.sessions().await.unwrap()[0];
    assert!(ide_core::same_path(
        Path::new(&s.root),
        &data.join("workspaces").join("old1")
    ));
    assert_eq!(s.repos.len(), 1);
    assert!(ide_core::same_path(Path::new(&s.repos[0].path), &wt));
    assert_eq!(s.repos[0].branch, "ide/old1");
    assert!(
        c.fs_list(&s.id, &repos[0].id, "", false)
            .await
            .unwrap()
            .iter()
            .any(|e| e.name == "README.md")
    );
    // The launch spec runs in the new folder and reaches the old worktree by --add-dir... the
    // shell ignores it, so check the session guidance instead.
    let ctx = server.authorize(server.user()).unwrap();
    let session = ctx.resolve_session("old1").unwrap();
    let g = ide_core::session_guidance(&session, &ide_core::RepoStore::open(&ide_core::Paths::new(&data)).unwrap())
        .unwrap();
    assert!(g.contains(&wt.display().to_string().replace('\\', "/")), "{g}");
    c.delete_session("old1", true).await.unwrap();
    assert!(!wt.exists());
    assert!(!branch_exists(&repo, "ide/old1"));
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Hermes Desk's base-branch behaviour through the RPCs, against a local bare `origin`:
/// 기준 브랜치 최신화 (fetch before the worktree; local branch unmoved), a base picked per
/// session, fixed and rule bases on the repo, the session's own base, an unreachable origin
/// (that repo fails with 최신화, works without), and registering a plain folder (git init).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn base_branches_fetch_offline_and_plain_folder_init() {
    let dir = tmp_dir("repos-base");
    let seed = dir.join("seed");
    init_repo(&seed);
    let seed_s = seed.display().to_string();
    git_ok(&dir, &["clone", "-q", "--bare", &seed_s, "origin.git"]);
    let origin = dir.join("origin.git").display().to_string();
    git_ok(&dir, &["clone", "-q", &origin, "app"]);
    git_ok(&dir, &["clone", "-q", &origin, "pusher"]);
    let app = dir.join("app");
    let pusher = dir.join("pusher");
    let old = git_ok(&app, &["rev-parse", "main"]);
    git_ok(&pusher, &["commit", "-q", "--allow-empty", "-m", "remote work"]);
    git_ok(&pusher, &["branch", "release/2026.9.0"]);
    git_ok(&pusher, &["branch", "release/2026.19.0"]);
    git_ok(&pusher, &["push", "-q", "origin", "main", "release/2026.9.0", "release/2026.19.0"]);
    let new = git_ok(&pusher, &["rev-parse", "main"]);

    let server = server(&dir.join("data"));
    let c = Client::connect(memory(&server), ClientOptions::default())
        .await
        .unwrap();
    let ra = c.register_repo(&app.display().to_string()).await.unwrap();
    assert_eq!((ra.base.as_str(), ra.default_base.as_deref()), ("main", None));
    let head = |p: &str| git_ok(Path::new(p), &["rev-parse", "HEAD"]);
    let req = |refresh: bool, pick: Option<&str>| NewSession {
        refresh_base: refresh,
        bases: pick
            .map(|p| [(ra.id.clone(), p.to_owned())].into_iter().collect())
            .unwrap_or_default(),
        ..shell(vec![ra.id.clone()])
    };
    let mut made = Vec::new();

    // Without 최신화 the last-fetched origin/main; with it origin's latest, local main unmoved.
    let (s, f) = c.create_session(req(false, None)).await.unwrap();
    assert!(f.is_empty());
    assert_eq!(head(&s.repos[0].path), old);
    made.push(s.id.clone());
    let (s, f) = c.create_session(req(true, None)).await.unwrap();
    assert!(f.is_empty(), "{f:?}");
    assert_eq!(head(&s.repos[0].path), new);
    assert_eq!(git_ok(&app, &["rev-parse", "main"]), old);
    made.push(s.id.clone());

    // origin's branches (base pickers) and a base picked for one session.
    let names = c.repo_origin_branches(&ra.id).await.unwrap();
    assert!(names.contains(&"main".to_owned()), "{names:?}");
    let (s, f) = c.create_session(req(false, Some("release/2026.19.0"))).await.unwrap();
    assert_eq!(f[0].error.id, "origin-branch-missing");
    assert!(s.repos.is_empty());
    made.push(s.id.clone());
    let (s, f) = c.create_session(req(true, Some("release/2026.19.0"))).await.unwrap();
    assert!(f.is_empty(), "{f:?}");
    assert_eq!(s.repos[0].base, "release/2026.19.0");
    made.push(s.id.clone());

    // Fixed base: chosen from the refs, stored as the full ref.
    let (cur, refs) = c.repo_branches(&ra.id).await.unwrap();
    assert_eq!(cur.as_deref(), Some("main"));
    // Only the picked branch was fetched (Desk fetches one branch, not the whole remote).
    assert!(refs.contains(&"refs/remotes/origin/release/2026.19.0".to_owned()), "{refs:?}");
    assert!(!refs.contains(&"refs/remotes/origin/release/2026.9.0".to_owned()), "{refs:?}");
    let r = c
        .set_repo_base_branch(&ra.id, Some("refs/remotes/origin/release/2026.19.0"))
        .await
        .unwrap();
    assert_eq!(r.default_base.as_deref(), Some("release/2026.19.0"));
    let (s, f) = c.create_session(req(false, None)).await.unwrap();
    assert!(f.is_empty(), "{f:?}");
    assert_eq!(s.repos[0].base, "release/2026.19.0");
    made.push(s.id.clone());
    assert_eq!(
        c.set_repo_base_branch(&ra.id, Some("nope")).await.unwrap_err().id,
        "invalid-base-branch"
    );

    // Rule: preview (live ls-remote), save (replaces the fixed base), session from the pick.
    let rule = ide_client::BaseRuleInfo {
        remote: "origin".into(),
        pattern: "release/*".into(),
        matches: None,
    };
    let pv = c.preview_repo_base_rule(&ra.id, rule.clone()).await.unwrap();
    assert_eq!(pv.pick.as_deref(), Some("release/2026.19.0"));
    assert_eq!(pv.matched, ["release/2026.19.0", "release/2026.9.0"]);
    let bad = ide_client::BaseRuleInfo {
        pattern: "nothing/*".into(),
        ..rule.clone()
    };
    assert_eq!(
        c.set_repo_base_rule(&ra.id, Some(bad)).await.unwrap_err().id,
        "base-rule-no-match"
    );
    let r = c.set_repo_base_rule(&ra.id, Some(rule.clone())).await.unwrap();
    assert_eq!(r.base_branch, None);
    assert_eq!(r.base_rule.as_ref(), Some(&rule));
    assert_eq!(r.base, "release/2026.19.0");
    let (s, f) = c.create_session(req(false, None)).await.unwrap();
    assert!(f.is_empty(), "{f:?}");
    assert_eq!(s.repos[0].base, "release/2026.19.0");
    made.push(s.id.clone());
    // A newer release pushed later is picked up by the next session and reported.
    git_ok(&pusher, &["branch", "release/2026.20.0"]);
    git_ok(&pusher, &["push", "-q", "origin", "release/2026.20.0"]);
    let (s, f, changes) = c.create_session_full(req(false, None)).await.unwrap();
    assert!(f.is_empty(), "{f:?}");
    assert_eq!(s.repos[0].base, "release/2026.20.0");
    assert_eq!(changes, ["app: origin/release/2026.19.0 → origin/release/2026.20.0"]);
    made.push(s.id.clone());

    // The session's own base: origin branches only; None = back to the repo default.
    assert_eq!(
        c.set_session_base(&s.id, &ra.id, Some("nope")).await.unwrap_err().id,
        "origin-branch-unknown"
    );
    let s2 = c.set_session_base(&s.id, &ra.id, Some("main")).await.unwrap();
    assert_eq!(s2.repos[0].base, "main");
    c.set_repo_base_rule(&ra.id, None).await.unwrap();
    let s3 = c.set_session_base(&s.id, &ra.id, None).await.unwrap();
    assert_eq!(s3.repos[0].base, "release/2026.20.0"); // the worktree's own base

    // Unreachable origin: 최신화 fails that repo (the session stays); without it, local state.
    git_ok(&app, &["remote", "set-url", "origin", &dir.join("gone.git").display().to_string()]);
    let (s, f) = c.create_session(req(true, None)).await.unwrap();
    assert_eq!(f[0].error.id, "base-refresh-failed");
    assert!(s.repos.is_empty());
    made.push(s.id.clone());
    let (s, f) = c.create_session(req(false, None)).await.unwrap();
    assert!(f.is_empty(), "{f:?}");
    assert_eq!(head(&s.repos[0].path), new);
    made.push(s.id.clone());
    assert_eq!(
        c.preview_repo_base_rule(&ra.id, rule).await.unwrap_err().id,
        "remote-list-failed"
    );

    // A plain folder: refused without init, git-initialised (main, "Initial commit") with it.
    let plain = dir.join("일반 폴더");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("notes.txt"), "x").unwrap();
    let ps = plain.display().to_string();
    assert_eq!(c.register_repo(&ps).await.unwrap_err().id, "not-a-git-folder");
    assert!(!plain.join(".git").exists());
    let rp = c.register_repo_init(&ps, true).await.unwrap();
    assert_eq!((rp.name.as_str(), rp.base.as_str()), ("일반 폴더", "main"));
    assert_eq!(git_out(&plain, &["log", "--format=%s"]), "Initial commit");
    assert_eq!(git_out(&plain, &["ls-files"]), "notes.txt");
    // With 최신화 a repo without a remote fails with base-no-remote (as Desk).
    let (s, f) = c
        .create_session(NewSession {
            refresh_base: true,
            ..shell(vec![rp.id.clone()])
        })
        .await
        .unwrap();
    assert_eq!(f[0].error.id, "base-no-remote");
    made.push(s.id.clone());

    for id in made {
        c.delete_session(&id, true).await.unwrap();
    }
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}
