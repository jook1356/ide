//! Local git actions of the Changes / History tabs end to end through `ide_client::Client`
//! (discard, stash, commit actions, sync / push / pull, branch picker, 새 브랜치로 이어가기)
//! over the memory transport and a real `ide-server` child (`--no-daemon`), in a session
//! worktree whose repository has a local bare `origin`.
//! Run with `IDE_TEST_TMP=<dir inside the workspace>`.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use ide_client::{
    BranchCheckout, BranchRefreshOptions, ChangeKind, Client, ClientOptions, CommitAction,
    GitChange, LocalChild, NewSession, PullMode, StashAction, SwitchMode,
};
use ide_core::Paths;
use ide_server::{Server, ServerConfig};

fn opts() -> ClientOptions {
    ClientOptions {
        backoff_min: Duration::from_millis(50),
        backoff_max: Duration::from_millis(500),
        ..ClientOptions::default()
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A repository with a local bare `origin` (main pushed) and a second clone `other` that
/// pushes to it. Returns (repo, origin, other).
fn repos(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let r = dir.join("저장소 repo");
    init_repo(&r);
    git(&r, &["config", "core.autocrlf", "false"]);
    std::fs::write(r.join("a.txt"), "a\n").unwrap();
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "a"]);
    let origin = dir.join("origin.git");
    git(
        dir,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            &origin.display().to_string(),
        ],
    );
    git(
        &r,
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    git(&r, &["push", "-q", "-u", "origin", "main"]);
    let other = dir.join("other");
    git(
        dir,
        &[
            "clone",
            "-q",
            &origin.display().to_string(),
            &other.display().to_string(),
        ],
    );
    for (k, v) in [
        ("user.email", "o@example.com"),
        ("user.name", "o"),
        ("core.autocrlf", "false"),
    ] {
        git(&other, &["config", k, v]);
    }
    (r, origin, other)
}

async fn git_actions_scenario(c: &Client, repo: &Path, other: &Path) {
    let r = c.register_repo(&repo.display().to_string()).await.unwrap();
    let (s, failures) = c
        .create_session(NewSession {
            provider: "shell".into(),
            account: None,
            configuration: "default".into(),
            repos: vec![r.id.clone()],
            original_repo: None,
            refresh_base: false,
            bases: Default::default(),
            title: "actions".into(),
            start: false,
            cols: 80,
            rows: 24,
        })
        .await
        .expect("create session");
    assert!(failures.is_empty(), "{failures:?}");
    let (sid, rp) = (s.id.as_str(), r.id.as_str());
    let wt = PathBuf::from(&s.repos[0].path);
    let branch = s.repos[0].branch.clone();

    // Unknown session / repo.
    let e = c.git_stash_list("nope", rp).await.unwrap_err();
    assert_eq!(e.id, "session-not-found");
    let e = c.git_branches(sid, "0123abcd").await.unwrap_err();
    assert_eq!(e.id, "repo-not-attached");

    // Sync: a new session branch is unpublished with nothing to push.
    let st = c.git_sync_status(sid, rp, false, None).await.unwrap();
    assert!(st.unpublished && !st.no_remote && st.ahead == 0, "{st:?}");

    // Discard: a modified and an untracked file.
    std::fs::write(wt.join("a.txt"), "changed\n").unwrap();
    std::fs::write(wt.join("새 파일.txt"), "n\n").unwrap();
    c.git_discard(
        sid,
        rp,
        GitChange {
            path: "a.txt".into(),
            orig_path: None,
            kind: ChangeKind::Modified,
        },
    )
    .await
    .unwrap();
    c.git_discard(
        sid,
        rp,
        GitChange {
            path: "새 파일.txt".into(),
            orig_path: None,
            kind: ChangeKind::Untracked,
        },
    )
    .await
    .unwrap();
    let gs = c.git_status(sid, rp, None).await.unwrap();
    assert!(gs.unstaged.is_empty() && gs.untracked.is_empty(), "{gs:?}");

    // Stash: partial push, list, pop.
    std::fs::write(wt.join("a.txt"), "stashed\n").unwrap();
    std::fs::write(wt.join("README.md"), "kept\n").unwrap();
    c.git_stash_push(sid, rp, "메모", Some(vec!["a.txt".into()]))
        .await
        .unwrap();
    let list = c.git_stash_list(sid, rp).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        (
            list[0].message.as_str(),
            list[0].branch.as_str(),
            list[0].auto
        ),
        ("메모", branch.as_str(), false)
    );
    assert_eq!(list[0].files[0].path, "a.txt");
    let e = c
        .git_stash_push(sid, rp, "", Some(vec!["../x".into()]))
        .await
        .unwrap_err();
    assert_eq!(e.id, "path-denied");
    git(&wt, &["checkout", "--", "README.md"]);
    let res = c
        .git_stash_action(sid, rp, &list[0].sha, StashAction::Pop)
        .await
        .unwrap();
    assert!(res.conflicts.is_empty());
    assert_eq!(
        std::fs::read_to_string(wt.join("a.txt")).unwrap(),
        "stashed\n"
    );
    let e = c
        .git_stash_action(sid, rp, &list[0].sha, StashAction::Drop)
        .await
        .unwrap_err();
    assert_eq!(e.id, "git-stash-gone");
    let e = c.git_stash_push(sid, rp, "", None).await;
    assert!(e.is_ok());
    let e = c.git_stash_push(sid, rp, "", None).await.unwrap_err();
    assert_eq!(e.id, "git-stash-empty");

    // Commit, publish, sync.
    std::fs::write(wt.join("w.txt"), "w\n").unwrap();
    git(&wt, &["add", "-A"]);
    git(&wt, &["commit", "-q", "-m", "w"]);
    let w_sha = git(&wt, &["rev-parse", "HEAD"]);
    let st = c.git_sync_status(sid, rp, false, None).await.unwrap();
    assert_eq!((st.ahead, st.unpublished), (1, true));
    assert_eq!(c.git_push(sid, rp, false, None).await.unwrap(), branch);
    let st = c.git_sync_status(sid, rp, true, None).await.unwrap();
    let upstream = format!("origin/{branch}");
    assert_eq!(
        (st.ahead, st.behind, st.upstream.as_deref()),
        (0, 0, Some(upstream.as_str()))
    );

    // Someone else pushes to the branch: behind after a fetch, then pull.
    git(other, &["fetch", "-q", "origin"]);
    git(other, &["switch", "-q", "-c", &branch, &upstream]);
    std::fs::write(other.join("o.txt"), "o\n").unwrap();
    git(other, &["add", "-A"]);
    git(other, &["commit", "-q", "-m", "o"]);
    git(other, &["push", "-q", "origin", &branch]);
    let st = c.git_sync_status(sid, rp, true, None).await.unwrap();
    assert_eq!((st.ahead, st.behind), (0, 1));
    assert_eq!(c.git_pull(sid, rp, PullMode::Merge).await.unwrap(), branch);
    assert!(wt.join("o.txt").exists());

    // Commit actions.
    let e = c
        .git_commit_action(sid, rp, &w_sha[..7], CommitAction::Revert)
        .await
        .unwrap_err();
    assert_eq!(e.id, "git-bad-rev");
    std::fs::write(wt.join("dirty.txt"), "d\n").unwrap();
    let e = c
        .git_commit_action(sid, rp, &w_sha, CommitAction::Revert)
        .await
        .unwrap_err();
    assert_eq!(e.id, "git-dirty");
    std::fs::remove_file(wt.join("dirty.txt")).unwrap();
    c.git_commit_action(sid, rp, &w_sha, CommitAction::Revert)
        .await
        .unwrap();
    assert!(!wt.join("w.txt").exists());
    c.git_commit_action(sid, rp, &w_sha, CommitAction::CherryPick)
        .await
        .unwrap();
    assert!(wt.join("w.txt").exists());

    // Branch picker: the main repo's `main`, this worktree's branch; create / switch back.
    let b = c.git_branches(sid, rp).await.unwrap();
    assert_eq!(b.current.as_deref(), Some(branch.as_str()));
    let of = |n: &str| {
        b.local
            .iter()
            .find(|l| l.name == n)
            .map(|l| l.checkout.clone())
    };
    assert_eq!(of(&branch), Some(BranchCheckout::Here));
    assert!(matches!(of("main"), Some(BranchCheckout::MainRepo(_))));
    assert!(b.remote.iter().any(|r| r.short_ref == "origin/main"));
    let f = c.git_fetch_remotes(sid, rp).await.unwrap();
    assert!(f.ok, "{f:?}");
    std::fs::write(wt.join("a.txt"), "carry me\n").unwrap();
    c.git_create_branch(sid, rp, "기능/x", "origin/main", SwitchMode::Stash)
        .await
        .unwrap();
    let auto = c.git_stash_list(sid, rp).await.unwrap();
    assert!(auto[0].auto && auto[0].branch == branch, "{auto:?}");
    c.git_switch_branch(sid, rp, &branch, None, SwitchMode::Carry)
        .await
        .unwrap();
    c.git_stash_action(sid, rp, &auto[0].sha, StashAction::Pop)
        .await
        .unwrap();
    git(&wt, &["checkout", "--", "a.txt"]);

    // 새 브랜치로 이어가기: main moved on; continue on a fresh branch with the picked commits.
    git(other, &["switch", "-q", "main"]);
    git(other, &["pull", "-q", "--ff-only"]);
    std::fs::write(other.join("m.txt"), "m\n").unwrap();
    git(other, &["add", "-A"]);
    git(other, &["commit", "-q", "-m", "m"]);
    git(other, &["push", "-q", "origin", "main"]);
    let info = c.git_branch_refresh_info(sid, rp, true).await.unwrap();
    assert_eq!(
        (info.branch.as_str(), info.base.as_str()),
        (branch.as_str(), "main")
    );
    assert!(!info.fetch_failed);
    assert_eq!(info.suggested, format!("{branch}-2"));
    let picks: Vec<String> = info
        .commits
        .iter()
        .filter(|c| c.subject == "w")
        .map(|c| c.full.clone())
        .collect();
    assert_eq!(picks.len(), 2, "{:?}", info.commits); // the original and the cherry-pick
    let check = c
        .git_branch_refresh_check(sid, rp, picks[..1].to_vec(), false)
        .await
        .unwrap();
    assert!(check.conflict.is_none(), "{check:?}");
    let e = c
        .git_branch_refresh_check(sid, rp, vec!["0".repeat(40)], false)
        .await
        .unwrap_err();
    assert_eq!(e.id, "git-refresh-moved");
    let res = c
        .git_refresh_branch(
            sid,
            rp,
            BranchRefreshOptions {
                name: None,
                commits: picks[..1].to_vec(),
                move_files: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (res.branch.clone(), res.previous.as_str()),
        (format!("{branch}-2"), branch.as_str())
    );
    assert_eq!(res.moved_commits, 1);
    assert!(wt.join("m.txt").exists() && wt.join("w.txt").exists());
    // The session records the new branch.
    let s2 = c
        .sessions()
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.id == s.id)
        .unwrap();
    assert_eq!(s2.repos[0].branch, res.branch);
    let st = c.git_sync_status(sid, rp, false, None).await.unwrap();
    assert!(st.unpublished && st.ahead == 1, "{st:?}");

    // Reset to the upstream needs one.
    let e = c.git_pull(sid, rp, PullMode::Reset).await.unwrap_err();
    assert_eq!(e.id, "git-no-upstream");

    c.delete_session(sid, true).await.unwrap();
    c.unregister_repo(&r.id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_actions_over_memory() {
    let dir = tmp_dir("gamem");
    let (repo, _origin, other) = repos(&dir);
    let server = Server::new(ServerConfig::for_paths(Paths::new(dir.join("data"))));
    let c = Client::connect(memory(&server), opts()).await.unwrap();
    git_actions_scenario(&c, &repo, &other).await;
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_actions_over_real_child() {
    let dir = tmp_dir("gachild");
    let (repo, _origin, other) = repos(&dir);
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let t = LocalChild::new(env!("CARGO_BIN_EXE_ide-server"))
        .no_daemon()
        .env("IDE_DATA_DIR", data.display().to_string());
    let c = Client::connect(t, opts()).await.expect("connect child");
    git_actions_scenario(&c, &repo, &other).await;
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}
