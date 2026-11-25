//! Local git actions of the Changes / History tabs: port of Hermes Desk's `discardChange`,
//! stash list/push/action, `runCommitAction`, `syncStatus` / `pushBranch` / `pullBranch`,
//! the branch picker (`branchList`, `remoteBranchList`, `fetchRemotes`, `switchBranch`,
//! `switchRemoteBranch`, `createBranch`) and 새 브랜치로 이어가기 (`branchRefresh.ts`). Every
//! function works on one session worktree (`root`); the service resolves and jails it. See
//! `docs/changes-actions.md`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meninx_proto::{
    BranchCheckout, BranchCommit, BranchRefreshCheck, BranchRefreshInfo, BranchRefreshOptions,
    BranchRefreshResult, ChangeKind, CommitAction, GitBranches, GitChange, LocalBranch, ProtoError,
    PullMode, PushExpected, RefreshConflict, RemoteBranch, RemoteFetchStatus, STASH_TAG,
    StashAction, StashActionResult, StashEntry, StatusEntry, SwitchMode, SyncStatus,
};

use meninx_proto::{Request, Response};

use crate::jail::{RootJail, normalize_rel};
use crate::service::{UserCtx, core_error};

/// Push / pull (Desk: 180 s).
const PUSH_TIMEOUT: Duration = Duration::from_secs(180);
/// Fetch of one branch, commit actions, a remote branch for the picker (Desk: 60 s).
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
/// Fetch of the upstream before `reset` (Desk: 120 s).
const RESET_FETCH_TIMEOUT: Duration = Duration::from_secs(120);
/// One remote of the picker's `fetch --prune` (Desk: 30 s).
const REMOTES_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// The picker fetches a repository's remotes at most this often (Desk: 2 min).
const REMOTE_FETCH_INTERVAL: Duration = Duration::from_secs(120);
/// A failed picker fetch is retried sooner (Desk: 20 s).
const REMOTE_FETCH_RETRY: Duration = Duration::from_secs(20);
/// Undelivered commits counted at most (Desk's `--max-count=500`).
const UNDELIVERED_MAX: u32 = 500;
/// Commits listed by 새 브랜치로 이어가기 (the newest ones).
const REFRESH_MAX_COMMITS: usize = 100;
/// Newest commits examined for "already in the base" (one merge-tree each).
const REFRESH_MAX_IN_BASE_CHECKS: usize = 40;
/// Untracked paths looked up in the new branch's tree.
const REFRESH_MAX_UNTRACKED_CHECKS: usize = 200;
/// Longest stash message (Desk's IPC check).
const STASH_MESSAGE_MAX: usize = 10_000;

/// Never let git (or Git Credential Manager) prompt (Desk's `NO_PROMPT`).
const NO_PROMPT: [(&str, &str); 2] = [("GIT_TERMINAL_PROMPT", "0"), ("GCM_INTERACTIVE", "never")];

// ---------------------------------------------------------------------------------------
// git runner

struct Out {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Out {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_owned()
    }
    fn error<S: AsRef<OsStr>>(&self, args: &[S]) -> ProtoError {
        core_error(&meninx_core::Error::Git {
            args: args
                .iter()
                .map(|a| a.as_ref().to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            status: self
                .code
                .map_or_else(|| "signal".to_owned(), |c| format!("exit code: {c}")),
            stderr: String::from_utf8_lossy(&self.stderr).trim().to_owned(),
        })
    }
}

/// Run git in `root` with extra `env`; killed after `timeout` (`git-timeout`). Never prompts.
fn raw_env<S: AsRef<OsStr>>(
    root: &Path,
    args: &[S],
    env: &[(&str, &str)],
    timeout: Option<Duration>,
) -> Result<Out, ProtoError> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let Some(timeout) = timeout else {
        let mut all: Vec<(&str, &str)> = NO_PROMPT.to_vec();
        all.extend_from_slice(env);
        let o = meninx_core::git_raw(root, args, &all).map_err(|e| core_error(&e))?;
        return Ok(Out {
            code: o.code,
            stdout: o.stdout,
            stderr: o.stderr,
        });
    };
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in NO_PROMPT.iter().chain(env) {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let spawn_err = |e: std::io::Error| core_error(&meninx_core::Error::GitSpawn(e));
    let mut child = cmd.spawn().map_err(spawn_err)?;
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut b);
        }
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut b);
        }
        b
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait().map_err(spawn_err)? {
            Some(st) => break st,
            None if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                // The readers are left detached: helpers (ssh, credential managers) may hold
                // the pipes open.
                let command = args
                    .first()
                    .map(|a| a.as_ref().to_string_lossy().into_owned())
                    .unwrap_or_default();
                return Err(core_error(&meninx_core::Error::GitTimeout {
                    command,
                    secs: timeout.as_secs(),
                }));
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    Ok(Out {
        code: status.code(),
        stdout: out_t.join().unwrap_or_default(),
        stderr: err_t.join().unwrap_or_default(),
    })
}

fn raw(root: &Path, args: &[&str]) -> Result<Out, ProtoError> {
    raw_env(root, args, &[], None)
}

/// Run git; exit 0 required; trimmed stdout.
fn git(root: &Path, args: &[&str]) -> Result<String, ProtoError> {
    git_env(root, args, &[], None)
}

fn git_env(
    root: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: Option<Duration>,
) -> Result<String, ProtoError> {
    let o = raw_env(root, args, env, timeout)?;
    if o.ok() {
        Ok(o.text())
    } else {
        Err(o.error(args))
    }
}

/// Trimmed stdout, or `None` on any failure.
pub(crate) fn try_git(root: &Path, args: &[&str]) -> Option<String> {
    raw(root, args)
        .ok()
        .filter(Out::ok)
        .map(|o| o.text().trim().to_owned())
}

pub(crate) fn succeeds(root: &Path, args: &[&str]) -> bool {
    raw(root, args).is_ok_and(|o| o.ok())
}

fn err(id: &str) -> ProtoError {
    ProtoError::new(id)
}

fn bad_rev(rev: &str) -> ProtoError {
    ProtoError::new("git-bad-rev").arg("rev", rev)
}

fn bad_branch(name: &str) -> ProtoError {
    ProtoError::new("git-bad-branch").arg("name", name)
}

/// A full commit id (40 or 64 hex digits; Desk's commit action / stash id check).
fn is_full_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `git check-ref-format --branch` plus no leading `-` (would read as an option).
fn valid_branch_name(root: &Path, name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && succeeds(root, &["check-ref-format", "--branch", name])
}

fn current_branch(root: &Path) -> String {
    try_git(root, &["branch", "--show-current"]).unwrap_or_default()
}

/// `rev-parse --abbrev-ref HEAD`: the branch, or `git-detached`.
fn head_branch_or_detached(root: &Path) -> Result<String, ProtoError> {
    let head = git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let head = head.trim();
    if head == "HEAD" {
        Err(err("git-detached"))
    } else {
        Ok(head.to_owned())
    }
}

fn stash_top(root: &Path) -> String {
    try_git(root, &["rev-parse", "--verify", "-q", "refs/stash"]).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------
// Discard

/// `git.discard`: Desk's `discardChange` (GitHub Desktop's "Discard changes").
pub fn discard(j: &RootJail, file: &GitChange) -> Result<(), ProtoError> {
    let root = j.root();
    let path = normalize_rel(&file.path)?;
    if path.is_empty() {
        return Err(ProtoError::new("path-invalid").arg("path", ""));
    }
    j.resolve_entry(&path)?;
    let orig = match &file.orig_path {
        Some(o) => {
            let o = normalize_rel(o)?;
            j.resolve_entry(&o)?;
            Some(o)
        }
        None => None,
    };
    let restore = |p: &str| {
        git(
            root,
            &[
                "--literal-pathspecs",
                "restore",
                "--source=HEAD",
                "--staged",
                "--worktree",
                "--",
                p,
            ],
        )
    };
    let rm = |p: &str| git(root, &["--literal-pathspecs", "rm", "-q", "-f", "--", p]);
    match file.kind {
        ChangeKind::Untracked => remove_entry(j, &path),
        // In the index but not in HEAD: drop it from both.
        ChangeKind::Added | ChangeKind::Copied => rm(&path).map(drop),
        ChangeKind::Renamed => {
            if let Some(o) = &orig {
                restore(o)?;
            }
            if rm(&path).is_err() {
                remove_entry(j, &path)?;
            }
            Ok(())
        }
        _ => restore(&path).map(drop),
    }
}

/// Delete a file or folder (links not followed); already gone is fine (`rm --force`).
fn remove_entry(j: &RootJail, rel: &str) -> Result<(), ProtoError> {
    let abs = j.resolve_entry(rel)?;
    if std::fs::symlink_metadata(&abs).is_err() {
        return Ok(());
    }
    crate::fsops::delete(j, rel, true)
}

// ---------------------------------------------------------------------------------------
// Stash

/// git's stash subject: `On <branch>: <msg>` (`-m`) or `WIP on <branch>: <sha> <subject>`.
fn parse_stash_subject(subject: &str) -> (String, String) {
    let (rest, wip) = if let Some(r) = subject.strip_prefix("WIP on ") {
        (r, true)
    } else if let Some(r) = subject.strip_prefix("On ") {
        (r, false)
    } else {
        return (String::new(), subject.to_owned());
    };
    let Some(i) = rest.find(": ") else {
        return (String::new(), subject.to_owned());
    };
    let b = &rest[..i];
    if b.contains(':') {
        return (String::new(), subject.to_owned());
    }
    let branch = if b == "(no branch)" { "" } else { b };
    let message = if wip { "" } else { &rest[i + 2..] };
    (branch.to_owned(), message.to_owned())
}

/// Files of a stash (`stash show --name-status --include-untracked -z`), untracked ones (its
/// third parent) marked as such.
fn stash_files(root: &Path, sha: &str) -> Vec<GitChange> {
    let show = |extra: bool| {
        let mut a = vec!["stash", "show", "--name-status"];
        if extra {
            a.push("--include-untracked");
        }
        a.extend(["-z", sha]);
        raw(root, &a).ok().filter(Out::ok).map(|o| o.stdout)
    };
    let Some(out) = show(true).or_else(|| show(false)) else {
        return Vec::new();
    };
    let third = format!("{sha}^3");
    let untracked: HashSet<String> = if succeeds(root, &["rev-parse", "--verify", "-q", &third]) {
        raw(root, &["ls-tree", "-r", "--name-only", "-z", &third])
            .ok()
            .filter(Out::ok)
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split('\0')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    } else {
        HashSet::new()
    };
    let text = String::from_utf8_lossy(&out);
    let parts: Vec<&str> = text.split('\0').collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let status = parts[i];
        i += 1;
        let Some(code) = status.chars().next() else {
            continue;
        };
        if code == 'R' || code == 'C' {
            let orig = parts.get(i).copied().unwrap_or_default();
            let path = parts.get(i + 1).copied().unwrap_or_default();
            i += 2;
            if !path.is_empty() {
                files.push(GitChange {
                    path: path.to_owned(),
                    orig_path: Some(orig.to_owned()),
                    kind: if code == 'R' {
                        ChangeKind::Renamed
                    } else {
                        ChangeKind::Added
                    },
                });
            }
            continue;
        }
        let path = parts.get(i).copied().unwrap_or_default();
        i += 1;
        if path.is_empty() {
            continue;
        }
        let kind = if untracked.contains(path) {
            ChangeKind::Untracked
        } else {
            match code {
                'A' => ChangeKind::Added,
                'D' => ChangeKind::Deleted,
                _ => ChangeKind::Modified,
            }
        };
        files.push(GitChange {
            path: path.to_owned(),
            orig_path: None,
            kind,
        });
    }
    files
}

/// `git.stash_list`: every stash of the repository, newest first (empty when git fails).
pub fn stash_list(root: &Path) -> Vec<StashEntry> {
    let Some(out) = try_git(root, &["stash", "list", "--format=%H%x1f%ct%x1f%s"]) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|line| {
            let mut f = line.trim_end().split('\x1f');
            let sha = f.next()?.to_owned();
            let ct = f.next()?;
            let subject = f.next().unwrap_or_default();
            if !(40..=64).contains(&sha.len()) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let (branch, message) = parse_stash_subject(subject);
            let auto = message.starts_with(STASH_TAG);
            Some(StashEntry {
                files: stash_files(root, &sha),
                sha,
                message: if auto { String::new() } else { message },
                branch,
                auto,
                date: ct.trim().parse().unwrap_or(0),
            })
        })
        .collect()
}

/// `git.stash_push`: stash every change, or just `paths` (file names, not patterns),
/// untracked files included. Nothing new on the stash = `git-stash-empty`.
pub fn stash_push(j: &RootJail, message: &str, paths: Option<&[String]>) -> Result<(), ProtoError> {
    let root = j.root();
    if message.len() > STASH_MESSAGE_MAX {
        return Err(ProtoError::new("bad-request").arg("method", "git.stash_push"));
    }
    let mut rels = Vec::new();
    if let Some(paths) = paths {
        if paths.is_empty() {
            return Err(ProtoError::new("bad-request").arg("method", "git.stash_push"));
        }
        for p in paths {
            let n = normalize_rel(p)?;
            if n.is_empty() {
                return Err(ProtoError::new("path-invalid").arg("path", p));
            }
            j.resolve_entry(&n)?;
            rels.push(n);
        }
    }
    let before = stash_top(root);
    let msg = message.trim();
    let mut args: Vec<&str> = Vec::new();
    if paths.is_some() {
        args.push("--literal-pathspecs");
    }
    args.extend(["stash", "push", "--include-untracked"]);
    if !msg.is_empty() {
        args.extend(["-m", msg]);
    }
    if paths.is_some() {
        args.push("--");
        args.extend(rels.iter().map(String::as_str));
    }
    git(root, &args)?;
    // "No local changes to save" exits 0 (and is localized): nothing new on top = nothing saved.
    if stash_top(root) == before {
        return Err(err("git-stash-empty"));
    }
    Ok(())
}

/// `git.stash_action`: pop / apply / drop the stash with commit id `sha`, looked up right
/// before acting (other worktrees shift the `stash@{n}` indexes). A conflicting pop/apply
/// returns the conflicted paths (git keeps the stash).
pub fn stash_action(
    root: &Path,
    sha: &str,
    action: StashAction,
) -> Result<StashActionResult, ProtoError> {
    if !is_full_sha(sha) {
        return Err(bad_rev(sha));
    }
    let list = try_git(root, &["stash", "list", "--format=%H%x1f%gd"]).unwrap_or_default();
    let stash_ref = list.lines().find_map(|l| {
        let (s, r) = l.trim_end().split_once('\x1f')?;
        s.eq_ignore_ascii_case(sha).then(|| r.to_owned())
    });
    let Some(stash_ref) = stash_ref else {
        return Err(err("git-stash-gone"));
    };
    let verb = match action {
        StashAction::Drop => {
            git(root, &["stash", "drop", "-q", &stash_ref])?;
            return Ok(StashActionResult::default());
        }
        StashAction::Pop => "pop",
        StashAction::Apply => "apply",
    };
    let args = ["stash", verb, stash_ref.as_str()];
    let o = raw(root, &args)?;
    if o.ok() {
        return Ok(StashActionResult::default());
    }
    let conflicts = conflicted_paths(root);
    if conflicts.is_empty() {
        Err(o.error(&args))
    } else {
        Ok(StashActionResult { conflicts })
    }
}

fn conflicted_paths(root: &Path) -> Vec<String> {
    raw(root, &["diff", "--name-only", "--diff-filter=U", "-z"])
        .ok()
        .filter(Out::ok)
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------
// Commit actions

fn busy_set() -> &'static Mutex<HashSet<PathBuf>> {
    static BUSY: std::sync::OnceLock<Mutex<HashSet<PathBuf>>> = std::sync::OnceLock::new();
    BUSY.get_or_init(Default::default)
}

/// Releases a worktree's commit-action slot.
struct BusyGuard(PathBuf);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        busy_set().lock().unwrap().remove(&self.0);
    }
}

/// A merge / rebase / cherry-pick / revert in progress (Desk's state files).
fn operation_in_progress(root: &Path) -> Result<bool, ProtoError> {
    let git_dir = PathBuf::from(git(root, &["rev-parse", "--absolute-git-dir"])?.trim());
    Ok([
        "MERGE_HEAD",
        "REBASE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ]
    .iter()
    .any(|n| git_dir.join(n).exists()))
}

/// Desk's branch-name pre-check of a commit action (before `check-ref-format`).
fn plausible_branch(name: &str) -> bool {
    !name.is_empty()
        && name == name.trim()
        && !name.starts_with('-')
        && !name.contains("@{")
        && !name.chars().any(|c| (c as u32) <= 32 || c as u32 == 127)
}

/// `git.commit_action`: Desk's `runCommitAction` (allow-listed actions, clean tree, no
/// operation in progress, no merge commits for revert / cherry-pick, one action per worktree).
pub fn commit_action(root: &Path, sha: &str, action: &CommitAction) -> Result<(), ProtoError> {
    if !is_full_sha(sha) {
        return Err(bad_rev(sha));
    }
    if let CommitAction::Branch { name } = action
        && !plausible_branch(name)
    {
        return Err(bad_branch(name));
    }
    let key = root.to_path_buf();
    if !busy_set().lock().unwrap().insert(key.clone()) {
        return Err(err("git-busy"));
    }
    let _guard = BusyGuard(key);
    if operation_in_progress(root)? {
        return Err(err("git-in-progress"));
    }
    let dirty = raw(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    if !dirty.ok() {
        return Err(dirty.error(&["status"]));
    }
    if !dirty.stdout.is_empty() {
        return Err(err("git-dirty"));
    }
    if try_git(root, &["cat-file", "-t", sha]).as_deref() != Some("commit") {
        return Err(bad_rev(sha));
    }
    if matches!(action, CommitAction::Revert | CommitAction::CherryPick) {
        let parents = git(root, &["rev-list", "--parents", "-n", "1", sha])?;
        if parents.split_whitespace().count() > 2 {
            return Err(err("git-merge-commit"));
        }
    }
    let args: Vec<&str> = match action {
        CommitAction::Checkout => vec!["switch", "--detach", sha],
        CommitAction::Branch { name } => {
            if !succeeds(root, &["check-ref-format", &format!("refs/heads/{name}")]) {
                return Err(bad_branch(name));
            }
            vec!["switch", "--no-track", "-c", name, sha]
        }
        CommitAction::Revert => vec!["revert", "--no-edit", sha],
        CommitAction::CherryPick => vec!["cherry-pick", sha],
    };
    git_env(
        root,
        &args,
        &[("GIT_EDITOR", "true"), ("GIT_SEQUENCE_EDITOR", "true")],
        Some(FETCH_TIMEOUT),
    )
    .map(drop)
}

// ---------------------------------------------------------------------------------------
// Sync status, push, pull

fn has_remote(root: &Path) -> bool {
    try_git(root, &["remote"]).is_some_and(|r| !r.trim().is_empty())
}

/// The branch's upstream state from one `for-each-ref`: (full upstream ref, short, track).
fn upstream_of(root: &Path, branch: &str) -> (String, String, String) {
    let own = format!("refs/heads/{branch}");
    let out = try_git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(upstream)%00%(upstream:short)%00%(upstream:track)",
            &own,
        ],
    )
    .unwrap_or_default();
    // Patterns also match refs below them (refs/heads/a matches refs/heads/a/b): exact only.
    for line in out.lines() {
        let f: Vec<&str> = line.trim_end_matches('\r').split('\0').collect();
        if f.len() >= 4 && f[0] == own {
            return (f[1].to_owned(), f[2].to_owned(), f[3].to_owned());
        }
    }
    Default::default()
}

fn track_count(track: &str, word: &str) -> u32 {
    track
        .split(['[', ']', ','])
        .map(str::trim)
        .find_map(|p| p.strip_prefix(word)?.trim().parse().ok())
        .unwrap_or(0)
}

/// Desk's `undeliveredCommits` count: with an upstream `upstream..HEAD`, else
/// `HEAD --not --remotes`; commits reachable from `merged_head` (a merged PR's head) left out.
fn undelivered(root: &Path, merged_head: Option<&str>) -> u32 {
    let upstream = try_git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .unwrap_or_default();
    let merged = merged_head
        .filter(|m| (7..=64).contains(&m.len()) && m.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|m| {
            try_git(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{m}^{{commit}}"),
                ],
            )
        })
        .unwrap_or_default();
    let max = format!("--max-count={UNDELIVERED_MAX}");
    let range = format!("{upstream}..HEAD");
    let not_merged = format!("^{merged}");
    let mut args = vec!["rev-list", max.as_str()];
    if upstream.is_empty() {
        args.push("HEAD");
    } else {
        args.push(&range);
    }
    // `^merged` before `--not`: that flag flips every ref after it.
    if !merged.is_empty() {
        args.push(&not_merged);
    }
    if upstream.is_empty() {
        args.extend(["--not", "--remotes"]);
    }
    try_git(root, &args).map_or(0, |o| o.split_whitespace().count() as u32)
}

/// `git.sync_status`: Desk's `syncStatus`.
pub fn sync_status(
    root: &Path,
    fetch: bool,
    merged_head: Option<&str>,
) -> Result<SyncStatus, ProtoError> {
    // A folder git-initialised by the app has no remote: say so instead of counting commits.
    if !has_remote(root) {
        return Ok(SyncStatus {
            no_remote: true,
            ..Default::default()
        });
    }
    let branch = current_branch(root);
    let (up_ref, up_short, track) = if branch.is_empty() {
        Default::default()
    } else {
        upstream_of(root, &branch)
    };
    let gone = !up_ref.is_empty() && track.contains("gone");
    if up_ref.is_empty() || gone {
        // Not on the remote (never pushed, or its remote branch was deleted): publish.
        let merged = if branch.is_empty() { None } else { merged_head };
        return Ok(SyncStatus {
            ahead: undelivered(root, merged),
            unpublished: true,
            gone,
            ..Default::default()
        });
    }
    if !fetch {
        return Ok(SyncStatus {
            ahead: track_count(&track, "ahead"),
            behind: track_count(&track, "behind"),
            upstream: Some(up_short),
            ..Default::default()
        });
    }
    if let Some((remote, b)) = up_short.split_once('/') {
        let _ = git_env(
            root,
            &["fetch", "--quiet", remote, b],
            &[],
            Some(FETCH_TIMEOUT),
        );
    }
    let counts = git(
        root,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{up_short}...HEAD"),
        ],
    )?;
    let mut n = counts
        .split_whitespace()
        .map(|s| s.parse::<u32>().unwrap_or(0));
    let behind = n.next().unwrap_or(0);
    let ahead = n.next().unwrap_or(0);
    Ok(SyncStatus {
        ahead,
        behind,
        upstream: Some(up_short),
        ..Default::default()
    })
}

/// `git.push`: Desk's `pushBranch`. Returns the branch.
pub fn push(
    root: &Path,
    force: bool,
    expected: Option<&PushExpected>,
) -> Result<String, ProtoError> {
    let head = head_branch_or_detached(root)?;
    if let Some(e) = expected {
        if force || !is_full_sha(&e.head_sha) || head != e.branch {
            return Err(err("git-push-changed"));
        }
        let sha = git(root, &["rev-parse", "HEAD"])?;
        let dirty = git(root, &["status", "--porcelain"])?;
        if !sha.trim().eq_ignore_ascii_case(&e.head_sha) || !dirty.trim().is_empty() {
            return Err(err("git-push-changed"));
        }
        // Never publish a later agent turn's commits via the moving HEAD.
        let spec = format!("{}:refs/heads/{head}", e.head_sha);
        git_env(root, &["push", "origin", &spec], &[], Some(PUSH_TIMEOUT))?;
        return Ok(head);
    }
    let mut args = vec!["push", "-u"];
    if force {
        args.push("--force-with-lease");
    }
    args.extend(["origin", "HEAD"]);
    git_env(root, &args, &[], Some(PUSH_TIMEOUT))?;
    Ok(head)
}

/// `git.pull`: Desk's `pullBranch`. Returns the branch.
pub fn pull(root: &Path, mode: PullMode) -> Result<String, ProtoError> {
    let head = head_branch_or_detached(root)?;
    let env = [("GIT_MERGE_AUTOEDIT", "no")];
    if mode == PullMode::Reset {
        let upstream = try_git(
            root,
            &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        )
        .unwrap_or_default();
        let Some((remote, branch)) = upstream.split_once('/') else {
            return Err(err("git-no-upstream"));
        };
        git_env(
            root,
            &["fetch", "--quiet", remote, branch],
            &env,
            Some(RESET_FETCH_TIMEOUT),
        )?;
        git(root, &["reset", "--hard", "-q", &upstream])?;
        return Ok(head);
    }
    let how = if mode == PullMode::Rebase {
        "--rebase"
    } else {
        "--no-rebase"
    };
    git_env(
        root,
        &["pull", how, "--no-edit", "--autostash"],
        &env,
        Some(PUSH_TIMEOUT),
    )?;
    Ok(head)
}

// ---------------------------------------------------------------------------------------
// Branch picker

/// Remote names, longest first ("a/b" wins over "a" when splitting a ref).
fn remote_names(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = try_git(root, &["remote"])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    v.sort_by_key(|r| std::cmp::Reverse(r.len()));
    v
}

/// Who holds each branch (Desk's `branchHolders`): another linked worktree outranks the main
/// repository folder, which outranks this folder. `(path, main, here)`.
fn branch_holders(root: &Path) -> Result<HashMap<String, (String, bool, bool)>, ProtoError> {
    let out = git(root, &["worktree", "list", "--porcelain"])?;
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    let here = Path::new(top.trim());
    let mut holders: HashMap<String, (String, bool, bool)> = HashMap::new();
    let mut path = String::new();
    let mut index: i32 = -1;
    for line in out.lines().map(str::trim) {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = p.to_owned();
            index += 1;
        } else if let Some(name) = line.strip_prefix("branch refs/heads/") {
            let main = index == 0;
            let is_here = meninx_core::same_path(Path::new(&path), here);
            if is_here {
                holders
                    .entry(name.to_owned())
                    .or_insert((path.clone(), main, true));
                continue;
            }
            let replace = match holders.get(name) {
                None => true,
                Some((_, pm, ph)) => !main && (*pm || *ph),
            };
            if replace {
                holders.insert(name.to_owned(), (path.clone(), main, false));
            }
        }
    }
    Ok(holders)
}

/// Path of another linked worktree (a session's) that has `branch` checked out: git refuses
/// that switch. Held only by the main repository folder is allowed.
fn checked_out_elsewhere(root: &Path, branch: &str) -> Result<Option<String>, ProtoError> {
    Ok(branch_holders(root)?
        .remove(branch)
        .filter(|(_, main, here)| !main && !here)
        .map(|(p, _, _)| p))
}

/// `git.branches`.
pub fn branches(root: &Path) -> Result<GitBranches, ProtoError> {
    let holders = branch_holders(root)?;
    let refs = git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let remotes = remote_names(root);
    let mut local = Vec::new();
    let mut tracking = Vec::new();
    for r in refs.lines().map(str::trim) {
        if let Some(name) = r.strip_prefix("refs/heads/") {
            let checkout = match holders.get(name) {
                None => BranchCheckout::None,
                Some((_, _, true)) => BranchCheckout::Here,
                Some((p, true, _)) => BranchCheckout::MainRepo(p.clone()),
                Some((p, false, _)) => BranchCheckout::Worktree(p.clone()),
            };
            local.push(LocalBranch {
                name: name.to_owned(),
                checkout,
            });
        } else if let Some(short) = r.strip_prefix("refs/remotes/")
            && !r.ends_with("/HEAD")
        {
            tracking.push(short.to_owned());
        }
    }
    let names: HashSet<&str> = local.iter().map(|b| b.name.as_str()).collect();
    let remote = tracking
        .iter()
        .filter_map(|short| {
            // Refs of a removed remote: fall back to the first segment.
            let remote = remotes
                .iter()
                .find(|r| short.starts_with(&format!("{r}/")))
                .cloned()
                .unwrap_or_else(|| short.split('/').next().unwrap_or_default().to_owned());
            let branch = short.get(remote.len() + 1..).unwrap_or_default().to_owned();
            (!branch.is_empty()).then(|| RemoteBranch {
                has_local: names.contains(branch.as_str()),
                remote,
                branch,
                short_ref: short.clone(),
            })
        })
        .collect();
    let current = current_branch(root);
    Ok(GitBranches {
        current: (!current.is_empty()).then_some(current),
        local,
        remote,
    })
}

type FetchSlot = Arc<Mutex<Option<(Instant, RemoteFetchStatus)>>>;

fn fetch_slots() -> &'static Mutex<HashMap<String, FetchSlot>> {
    static SLOTS: std::sync::OnceLock<Mutex<HashMap<String, FetchSlot>>> =
        std::sync::OnceLock::new();
    SLOTS.get_or_init(Default::default)
}

/// `git.fetch_remotes`: Desk's `fetchRemotes`: `git fetch --prune` every remote, at most once
/// per [`REMOTE_FETCH_INTERVAL`] per repository (all its worktrees count as one; a failure is
/// retried after [`REMOTE_FETCH_RETRY`]); a fetch in flight is waited for and shared. Never
/// fails: errors are reported in the status.
pub fn fetch_remotes(root: &Path) -> RemoteFetchStatus {
    let key = match git(root, &["rev-parse", "--git-common-dir"]) {
        Ok(c) => {
            let p = Path::new(c.trim());
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                root.join(p)
            };
            abs.canonicalize()
                .unwrap_or(abs)
                .to_string_lossy()
                .to_lowercase()
        }
        Err(e) => {
            return RemoteFetchStatus {
                ok: false,
                fetched: false,
                error: Some(e.get("detail").unwrap_or(&e.id).to_owned()),
            };
        }
    };
    let slot = fetch_slots()
        .lock()
        .unwrap()
        .entry(key)
        .or_default()
        .clone();
    // Held for the whole fetch: a second caller waits and then reuses the fresh result.
    let mut slot = slot.lock().unwrap();
    if let Some((at, done)) = slot.as_ref() {
        let ttl = if done.ok {
            REMOTE_FETCH_INTERVAL
        } else {
            REMOTE_FETCH_RETRY
        };
        if at.elapsed() < ttl {
            return RemoteFetchStatus {
                fetched: false,
                ..done.clone()
            };
        }
    }
    let mut errors = Vec::new();
    for remote in remote_names(root) {
        if let Err(e) = git_env(
            root,
            &["fetch", "--prune", "--quiet", &remote],
            &[],
            Some(REMOTES_FETCH_TIMEOUT),
        ) {
            let detail = e.get("detail").unwrap_or(&e.id).to_owned();
            errors.push(format!("{remote}: {detail}"));
        }
    }
    let status = RemoteFetchStatus {
        ok: errors.is_empty(),
        fetched: true,
        error: (!errors.is_empty()).then(|| errors.join("\n")),
    };
    *slot = Some((Instant::now(), status.clone()));
    status
}

/// Switch (`switch_args`) with uncommitted changes carried or stashed (Desk's `switchBranch`
/// / `createBranch`): a stash is tagged with the branch left and put back if the switch fails.
fn switch_with(root: &Path, args: &[&str], mode: SwitchMode) -> Result<(), ProtoError> {
    if mode == SwitchMode::Carry {
        return git(root, args).map(drop);
    }
    let from = current_branch(root);
    let before = stash_top(root);
    let msg = format!("{STASH_TAG}{from}");
    git(root, &["stash", "push", "--include-untracked", "-m", &msg])?;
    let stashed = stash_top(root) != before;
    if let Err(e) = git(root, args) {
        // Put the changes back rather than leave them stranded in the stash.
        if stashed {
            let _ = git(root, &["stash", "pop"]);
        }
        return Err(e);
    }
    Ok(())
}

fn has_local_branch(root: &Path, branch: &str) -> bool {
    succeeds(
        root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
}

/// Switch to local `branch`, refused when another session's worktree has it.
fn switch_local(root: &Path, branch: &str, mode: SwitchMode) -> Result<(), ProtoError> {
    if let Some(path) = checked_out_elsewhere(root, branch)? {
        return Err(ProtoError::new("git-branch-checked-out")
            .arg("branch", branch)
            .arg("path", path));
    }
    switch_with(root, &["switch", "--ignore-other-worktrees", branch], mode)
}

/// `git.switch_branch`: Desk's `switchBranch` / `switchRemoteBranch`.
pub fn switch_branch(
    root: &Path,
    branch: &str,
    remote: Option<&str>,
    mode: SwitchMode,
) -> Result<(), ProtoError> {
    if !valid_branch_name(root, branch) {
        return Err(bad_branch(branch));
    }
    let Some(remote) = remote else {
        if !has_local_branch(root, branch) {
            return Err(bad_rev(branch));
        }
        return switch_local(root, branch, mode);
    };
    // Only a configured remote name may reach `git fetch`.
    if !remote_names(root).iter().any(|r| r == remote) {
        return Err(ProtoError::new("git-unknown-remote").arg("remote", remote));
    }
    if has_local_branch(root, branch) {
        return switch_local(root, branch, mode);
    }
    // Explicit refspec: updates refs/remotes/<remote>/<branch> whatever the fetch config.
    let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    git_env(
        root,
        &["fetch", "--quiet", remote, &spec],
        &[],
        Some(FETCH_TIMEOUT),
    )?;
    // Upstream set by hand: `--track` fails when the remote's fetch refspec is non-standard.
    git(
        root,
        &[
            "branch",
            "--no-track",
            branch,
            &format!("refs/remotes/{remote}/{branch}"),
        ],
    )?;
    let rest = (|| {
        git(
            root,
            &["config", &format!("branch.{branch}.remote"), remote],
        )?;
        git(
            root,
            &[
                "config",
                &format!("branch.{branch}.merge"),
                &format!("refs/heads/{branch}"),
            ],
        )?;
        switch_with(root, &["switch", "--ignore-other-worktrees", branch], mode)
    })();
    if rest.is_err() {
        // Only just created at the remote's tip: drop it so a retry starts clean.
        let _ = git(root, &["branch", "-D", branch]);
    }
    rest
}

/// `git.create_branch`: Desk's `createBranch` (no tracking of `base`).
pub fn create_branch(
    root: &Path,
    name: &str,
    base: &str,
    mode: SwitchMode,
) -> Result<(), ProtoError> {
    if !valid_branch_name(root, name) {
        return Err(bad_branch(name));
    }
    let bad = base.is_empty()
        || base.starts_with('-')
        || base.contains(':')
        || base.contains("..")
        || base.chars().any(|c| c.is_whitespace() || c.is_control());
    if bad
        || try_git(
            root,
            &["rev-parse", "--verify", "-q", &format!("{base}^{{commit}}")],
        )
        .is_none()
    {
        return Err(bad_rev(base));
    }
    switch_with(root, &["switch", "--no-track", "-c", name, base], mode)
}

// ---------------------------------------------------------------------------------------
// 새 브랜치로 이어가기 (branch refresh)

fn base_ref_of(base: &str) -> String {
    format!("refs/remotes/origin/{base}")
}

fn rev_parse(root: &Path, r: &str) -> String {
    try_git(root, &["rev-parse", "--verify", "--quiet", r]).unwrap_or_default()
}

fn has_commit(root: &Path, r: &str) -> bool {
    !rev_parse(root, &format!("{r}^{{commit}}")).is_empty()
}

fn is_sha(s: &str) -> bool {
    (40..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Local and origin branch names (`refs/heads/x` and `refs/remotes/origin/x` both -> x).
fn taken_names(root: &Path) -> HashSet<String> {
    try_git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes/origin",
        ],
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|r| {
        let r = r.trim();
        r.strip_prefix("refs/heads/")
            .or_else(|| r.strip_prefix("refs/remotes/origin/"))
            .map(str::to_owned)
    })
    .collect()
}

/// First free `<stem>-2`, `-3`... (stem: the branch without a trailing `-<digits>`).
pub fn suggest_name(branch: &str, taken: &HashSet<String>) -> String {
    let b = if branch.is_empty() { "branch" } else { branch };
    let stem = match b.rfind('-') {
        Some(i) if i + 1 < b.len() && b[i + 1..].bytes().all(|c| c.is_ascii_digit()) => &b[..i],
        _ => b,
    };
    (2..)
        .map(|i| format!("{stem}-{i}"))
        .find(|n| !taken.contains(n))
        .unwrap_or_default()
}

enum Merge {
    Tree(String),
    Conflict(Vec<String>),
}

/// Merge `b` into `a` without a work tree (`merge_base` given: a cherry-pick of `b` onto `a`
/// when it is b's parent). Needs git 2.40 (`git-merge-tree`).
fn merge_trees(
    root: &Path,
    a: &str,
    b: &str,
    merge_base: Option<&str>,
) -> Result<Merge, ProtoError> {
    let mb = merge_base.map(|m| format!("--merge-base={m}"));
    let mut args = vec!["merge-tree", "--write-tree", "--name-only", "--no-messages"];
    if let Some(m) = &mb {
        args.push(m);
    }
    args.extend([a, b]);
    let o = raw(root, &args)?;
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let tree = lines.first().copied().unwrap_or_default();
    match o.code {
        Some(0) if is_sha(tree) => Ok(Merge::Tree(tree.to_owned())),
        Some(1) if is_sha(tree) => {
            let mut seen = HashSet::new();
            Ok(Merge::Conflict(
                lines[1..]
                    .iter()
                    .filter(|l| !l.is_empty() && seen.insert(**l))
                    .map(|l| (*l).to_owned())
                    .collect(),
            ))
        }
        _ => Err(ProtoError::new("git-merge-tree").arg(
            "detail",
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .next()
                .unwrap_or_default()
                .trim(),
        )),
    }
}

/// `git log origin/<base>..HEAD`, oldest first (merges left out, the newest 100) and how many
/// more; `check_in_base`: mark the newest commit whose content the base already has, and all
/// before it.
fn branch_commits(root: &Path, base_ref: &str, check_in_base: bool) -> (Vec<BranchCommit>, u32) {
    if !has_commit(root, base_ref) {
        return (Vec::new(), 0);
    }
    let not_base = format!("^{base_ref}");
    let max = format!("--max-count={REFRESH_MAX_COMMITS}");
    let out = try_git(
        root,
        &[
            "log",
            "--no-merges",
            &max,
            "--format=%H%x09%h%x09%s",
            "HEAD",
            &not_base,
        ],
    )
    .unwrap_or_default();
    let mut commits: Vec<BranchCommit> = out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut f = line.splitn(3, '\t');
            BranchCommit {
                full: f.next().unwrap_or_default().to_owned(),
                sha: f.next().unwrap_or_default().to_owned(),
                subject: f.next().unwrap_or_default().to_owned(),
                in_base: false,
            }
        })
        .collect();
    commits.reverse();
    let count: usize = try_git(
        root,
        &["rev-list", "--count", "--no-merges", "HEAD", &not_base],
    )
    .and_then(|c| c.parse().ok())
    .unwrap_or(0);
    let more = count.saturating_sub(commits.len()) as u32;
    if !check_in_base || commits.is_empty() {
        return (commits, more);
    }
    let base_sha = rev_parse(root, base_ref);
    let fork = try_git(root, &["merge-base", base_ref, "HEAD"]).unwrap_or_default();
    // The base never moved since the branch left it: none of its commits can be in it.
    if fork == base_sha {
        return (commits, more);
    }
    let base_tree = rev_parse(root, &format!("{base_ref}^{{tree}}"));
    let oldest = commits.len().saturating_sub(REFRESH_MAX_IN_BASE_CHECKS);
    for i in (oldest..commits.len()).rev() {
        if let Ok(Merge::Tree(t)) = merge_trees(root, base_ref, &commits[i].full, None)
            && t == base_tree
        {
            for c in &mut commits[..=i] {
                c.in_base = true;
            }
            break;
        }
    }
    (commits, more)
}

/// The listed commits named by `shas`, in branch order; a name not listed means the branch
/// moved (`git-refresh-moved`).
fn resolve_picks(
    commits: &[BranchCommit],
    shas: &[String],
) -> Result<Vec<BranchCommit>, ProtoError> {
    let wanted: HashSet<String> = shas
        .iter()
        .filter(|s| is_sha(s))
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let picks: Vec<BranchCommit> = commits
        .iter()
        .filter(|c| wanted.contains(&c.full.to_ascii_lowercase()))
        .cloned()
        .collect();
    if picks.len() != wanted.len() {
        return Err(err("git-refresh-moved"));
    }
    Ok(picks)
}

struct Plan {
    planned: Vec<(BranchCommit, String)>,
    skipped: Vec<BranchCommit>,
    tree: String,
}

/// Cherry-pick `picks` onto `base_ref` in memory; the first conflict stops it.
fn plan_picks(
    root: &Path,
    base_ref: &str,
    picks: &[BranchCommit],
) -> Result<Result<Plan, RefreshConflict>, ProtoError> {
    let mut tree = rev_parse(root, &format!("{base_ref}^{{tree}}"));
    if tree.is_empty() {
        return Err(ProtoError::new("git-no-base").arg("base", base_ref));
    }
    let mut planned = Vec::new();
    let mut skipped = Vec::new();
    for c in picks {
        let parent = rev_parse(root, &format!("{}^", c.full));
        if parent.is_empty() {
            // A root commit cannot be moved.
            return Ok(Err(RefreshConflict {
                sha: c.full.clone(),
                subject: c.subject.clone(),
                files: Vec::new(),
            }));
        }
        match merge_trees(root, &tree, &c.full, Some(&parent))? {
            Merge::Conflict(files) => {
                return Ok(Err(RefreshConflict {
                    sha: c.full.clone(),
                    subject: c.subject.clone(),
                    files,
                }));
            }
            Merge::Tree(t) if t == tree => skipped.push(c.clone()),
            Merge::Tree(t) => {
                planned.push((c.clone(), t.clone()));
                tree = t;
            }
        }
    }
    Ok(Ok(Plan {
        planned,
        skipped,
        tree,
    }))
}

/// Uncommitted changes that would not apply on `tree`: tracked ones merged in memory
/// (`git stash create` touches nothing), untracked ones whose path the tree has.
fn uncommitted_conflicts(root: &Path, tree: &str) -> Result<Vec<String>, ProtoError> {
    let mut out: Vec<String> = Vec::new();
    let stash = try_git(root, &["stash", "create"]).unwrap_or_default();
    if is_sha(&stash)
        && let Ok(Merge::Conflict(files)) = merge_trees(root, tree, &stash, Some("HEAD"))
    {
        out.extend(files);
    }
    let untracked_raw = raw(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let text = String::from_utf8_lossy(&untracked_raw.stdout).into_owned();
    let untracked: Vec<&str> = text
        .split('\0')
        .filter(|s| !s.is_empty())
        .take(REFRESH_MAX_UNTRACKED_CHECKS)
        .collect();
    if !untracked.is_empty() {
        let mut args = vec!["ls-tree", "-r", "--name-only", "-z", tree, "--"];
        args.extend(untracked.iter().copied());
        if let Some(o) = raw(root, &args).ok().filter(Out::ok) {
            for f in String::from_utf8_lossy(&o.stdout).split('\0') {
                if !f.is_empty() && !out.iter().any(|x| x == f) {
                    out.push(f.to_owned());
                }
            }
        }
    }
    Ok(out)
}

/// `git status --porcelain -z --untracked-files=all` entries (old path of a rename skipped).
fn changed_files(root: &Path) -> Result<Vec<StatusEntry>, ProtoError> {
    let args = ["status", "--porcelain", "-z", "--untracked-files=all"];
    let o = raw(root, &args)?;
    if !o.ok() {
        return Err(o.error(&args));
    }
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    let parts: Vec<&str> = text.split('\0').collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let e = parts[i];
        i += 1;
        if e.len() < 4 {
            continue;
        }
        let status = &e[..2];
        files.push(StatusEntry {
            status: status.trim().to_owned(),
            path: e[3..].to_owned(),
        });
        if status.starts_with('R') || status.starts_with('C') {
            i += 1;
        }
    }
    Ok(files)
}

/// Making commits needs a committer: without a configured identity reuse the committer of the
/// newest commit being moved.
fn committer_env(root: &Path, commits: &[&BranchCommit]) -> Vec<(String, String)> {
    if succeeds(root, &["var", "GIT_COMMITTER_IDENT"]) {
        return Vec::new();
    }
    let Some(last) = commits.last() else {
        return Vec::new();
    };
    let out = try_git(root, &["log", "-1", "--format=%cn%x00%ce", &last.full]).unwrap_or_default();
    match out.split_once('\0') {
        Some((n, e)) if !n.is_empty() && !e.is_empty() => vec![
            ("GIT_COMMITTER_NAME".into(), n.to_owned()),
            ("GIT_COMMITTER_EMAIL".into(), e.to_owned()),
        ],
        _ => Vec::new(),
    }
}

/// A commit like `c` (author, date, message) with `tree` on top of `parent`.
fn recommit(
    root: &Path,
    c: &BranchCommit,
    tree: &str,
    parent: &str,
    committer: &[(String, String)],
) -> Result<String, ProtoError> {
    let meta = git(
        root,
        &[
            "log",
            "-1",
            "--date=raw",
            "--format=%an%x00%ae%x00%ad%x00%B",
            &c.full,
        ],
    )?;
    let mut f = meta.splitn(4, '\0');
    let name = f.next().unwrap_or_default().to_owned();
    let email = f.next().unwrap_or_default().to_owned();
    let date = f.next().unwrap_or_default().trim().to_owned();
    let body = f.next().unwrap_or_default().trim_end().to_owned();
    let message = if body.is_empty() {
        if c.subject.is_empty() {
            c.sha.clone()
        } else {
            c.subject.clone()
        }
    } else {
        body
    };
    let mut env: Vec<(&str, &str)> = vec![
        ("GIT_AUTHOR_NAME", &name),
        ("GIT_AUTHOR_EMAIL", &email),
        ("GIT_AUTHOR_DATE", &date),
    ];
    env.extend(committer.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let sha = git_env(
        root,
        &["commit-tree", tree, "-p", parent, "-m", &message],
        &env,
        Some(FETCH_TIMEOUT),
    )?;
    let sha = sha.trim().to_owned();
    if !is_sha(&sha) {
        return Err(ProtoError::new("git")
            .arg("args", "commit-tree")
            .arg("status", "")
            .arg("detail", sha));
    }
    Ok(sha)
}

fn check_base(root: &Path, base: &str) -> Result<(), ProtoError> {
    let ok = !base.is_empty()
        && !base.starts_with('-')
        && succeeds(root, &["check-ref-format", &format!("refs/heads/{base}")]);
    if ok {
        Ok(())
    } else {
        Err(ProtoError::new("git-no-base").arg("base", base))
    }
}

fn fetch_base(root: &Path, base: &str) -> Result<(), ProtoError> {
    let spec = format!("+refs/heads/{base}:{}", base_ref_of(base));
    git_env(
        root,
        &["fetch", "--quiet", "origin", &spec],
        &[],
        Some(FETCH_TIMEOUT),
    )
    .map(drop)
}

/// `git.branch_refresh_info`: Desk's `branchRefreshInfo` (`fetch`: the dialog opening; a
/// failed fetch falls back to the local `origin/<base>`).
pub fn branch_refresh_info(
    root: &Path,
    base: &str,
    fetch: bool,
) -> Result<BranchRefreshInfo, ProtoError> {
    let mut fetch_failed = false;
    if fetch {
        check_base(root, base)?;
        fetch_failed = fetch_base(root, base).is_err();
    }
    let branch = current_branch(root);
    let taken = taken_names(root);
    let (commits, more) = branch_commits(root, &base_ref_of(base), true);
    Ok(BranchRefreshInfo {
        suggested: suggest_name(&branch, &taken),
        branch,
        base: base.to_owned(),
        commits,
        more_commits: more,
        files: changed_files(root)?,
        fetch_failed,
    })
}

/// `git.branch_refresh_check`: Desk's `checkBranchRefresh` (local `origin/<base>`).
pub fn branch_refresh_check(
    root: &Path,
    base: &str,
    shas: &[String],
    move_files: bool,
) -> Result<BranchRefreshCheck, ProtoError> {
    check_base(root, base)?;
    let base_ref = base_ref_of(base);
    if !has_commit(root, &base_ref) {
        return Err(ProtoError::new("git-no-base").arg("base", format!("origin/{base}")));
    }
    let (commits, _) = branch_commits(root, &base_ref, false);
    let picks = resolve_picks(&commits, shas)?;
    match plan_picks(root, &base_ref, &picks)? {
        Err(conflict) => Ok(BranchRefreshCheck {
            conflict: Some(conflict),
            ..Default::default()
        }),
        Ok(plan) => Ok(BranchRefreshCheck {
            conflict: None,
            skipped: plan.skipped.into_iter().map(|c| c.full).collect(),
            files_conflict: if move_files {
                uncommitted_conflicts(root, &plan.tree)?
            } else {
                Vec::new()
            },
        }),
    }
}

/// `git.refresh_branch`: Desk's `refreshBranch`: fetch `origin/<base>`, re-make the picked
/// commits on it in memory (refused on a conflict before anything is touched), switch to a new
/// branch there (no upstream), uncommitted changes stashed first and popped when asked. The old
/// branch is kept.
pub fn refresh_branch(
    root: &Path,
    base: &str,
    opts: &BranchRefreshOptions,
) -> Result<BranchRefreshResult, ProtoError> {
    check_base(root, base)?;
    let old = current_branch(root);
    if old.is_empty() {
        return Err(err("git-detached"));
    }
    let taken = taken_names(root);
    let name = opts
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map_or_else(|| suggest_name(&old, &taken), str::to_owned);
    if !valid_branch_name(root, &name) {
        return Err(bad_branch(&name));
    }
    if taken.contains(&name) {
        return Err(ProtoError::new("git-branch-exists").arg("name", name));
    }
    let base_ref = base_ref_of(base);
    fetch_base(root, base)?;
    // Decided after the fetch, from the branch as it is now.
    let (commits, _) = branch_commits(root, &base_ref, false);
    let picks = resolve_picks(&commits, &opts.commits)?;
    let plan = match plan_picks(root, &base_ref, &picks)? {
        Ok(p) => p,
        Err(c) => {
            let short: String = c.sha.chars().take(7).collect();
            let shown: Vec<&str> = c.files.iter().take(3).map(String::as_str).collect();
            return Err(ProtoError::new("git-refresh-conflict")
                .arg("sha", short)
                .arg("subject", c.subject)
                .arg("files", shown.join(", "))
                .arg("more", c.files.len().saturating_sub(3).to_string()));
        }
    };
    let files = changed_files(root)?;
    let previous_head = git(root, &["rev-parse", "HEAD"])?.trim().to_owned();

    // The new history exists only as objects until the branch is created on it.
    let mut head = rev_parse(root, &format!("{base_ref}^{{commit}}"));
    let moved: Vec<&BranchCommit> = plan.planned.iter().map(|(c, _)| c).collect();
    let committer = committer_env(root, &moved);
    for (c, tree) in &plan.planned {
        head = recommit(root, c, tree, &head, &committer)?;
    }

    let mut stashed = false;
    if !files.is_empty() {
        let msg = format!("{STASH_TAG}{old}");
        git(root, &["stash", "push", "--include-untracked", "-m", &msg])?;
        stashed = true;
    }
    if let Err(e) = git(root, &["switch", "--no-track", "-c", &name, &head]) {
        if stashed {
            let _ = git(root, &["stash", "pop"]);
        }
        return Err(e);
    }
    let n = files.len() as u32;
    let (mut moved_files, mut stashed_files, mut left) = (0, 0, false);
    if stashed && opts.move_files {
        if git(root, &["stash", "pop"]).is_ok() {
            moved_files = n;
        } else {
            // A conflicting pop keeps the stash: back to the clean new branch (it was clean
            // before the pop, so only what the pop wrote is removed).
            let _ = git(root, &["reset", "--hard", "-q", "HEAD"]);
            let _ = git(root, &["clean", "-fdq"]);
            stashed_files = n;
            left = true;
        }
    } else if stashed {
        stashed_files = n;
    }
    let picked: HashSet<&str> = picks.iter().map(|c| c.full.as_str()).collect();
    Ok(BranchRefreshResult {
        branch: name,
        previous: old,
        previous_head,
        moved_commits: plan.planned.len() as u32,
        skipped_commits: plan.skipped.len() as u32,
        left_commits: commits
            .iter()
            .filter(|c| !picked.contains(c.full.as_str()))
            .count() as u32,
        moved_files,
        stashed_files,
        files_left_in_stash: left,
    })
}

// ---------------------------------------------------------------------------------------
// Dispatch

impl UserCtx {
    /// The local git action requests; each runs in the worktree of `repo` in `session`.
    /// Mutations are audited (`<session> <repo> <detail>`).
    pub(crate) fn handle_git_actions(&self, req: Request) -> Result<Response, ProtoError> {
        let audit = |action: &str, session: &str, repo: &str, detail: &str| {
            self.audit_action(action, &format!("{session} {repo} {detail}"));
        };
        match req {
            Request::GitDiscard {
                session,
                repo,
                file,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                discard(&j, &file)?;
                audit("git.discard", &session, &repo, &file.path);
                Ok(Response::Unit)
            }
            Request::GitStashList { session, repo } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                Ok(Response::GitStashes(stash_list(j.root())))
            }
            Request::GitStashPush {
                session,
                repo,
                message,
                paths,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                stash_push(&j, &message, paths.as_deref())?;
                let n = paths
                    .as_ref()
                    .map_or_else(|| "all".to_owned(), |p| p.len().to_string());
                audit("git.stash_push", &session, &repo, &n);
                Ok(Response::Unit)
            }
            Request::GitStashAction {
                session,
                repo,
                sha,
                action,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                let r = stash_action(j.root(), &sha, action)?;
                audit(
                    "git.stash_action",
                    &session,
                    &repo,
                    &format!("{action:?} {sha}"),
                );
                Ok(Response::GitStashResult(r))
            }
            Request::GitCommitAction {
                session,
                repo,
                sha,
                action,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                let r = commit_action(j.root(), &sha, &action);
                // Also after a failure: a conflict can leave a revert / cherry-pick in progress.
                audit(
                    "git.commit_action",
                    &session,
                    &repo,
                    &format!("{action:?} {sha}"),
                );
                r.map(|()| Response::Unit)
            }
            Request::GitSyncStatus {
                session,
                repo,
                fetch,
                merged_head,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                sync_status(j.root(), fetch, merged_head.as_deref()).map(Response::GitSyncStatus)
            }
            Request::GitPush {
                session,
                repo,
                force,
                expected,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                let b = push(j.root(), force, expected.as_ref())?;
                audit("git.push", &session, &repo, &format!("{b} force={force}"));
                Ok(Response::GitBranchName(b))
            }
            Request::GitPull {
                session,
                repo,
                mode,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                let b = pull(j.root(), mode)?;
                audit("git.pull", &session, &repo, &format!("{b} {mode:?}"));
                Ok(Response::GitBranchName(b))
            }
            Request::GitBranches { session, repo } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                branches(j.root()).map(Response::GitBranches)
            }
            Request::GitFetchRemotes { session, repo } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                Ok(Response::GitFetchStatus(fetch_remotes(j.root())))
            }
            Request::GitSwitchBranch {
                session,
                repo,
                branch,
                remote,
                mode,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                switch_branch(j.root(), &branch, remote.as_deref(), mode)?;
                let target = remote.map_or_else(|| branch.clone(), |r| format!("{r}/{branch}"));
                audit(
                    "git.switch_branch",
                    &session,
                    &repo,
                    &format!("{target} {mode:?}"),
                );
                Ok(Response::Unit)
            }
            Request::GitCreateBranch {
                session,
                repo,
                name,
                base,
                mode,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                create_branch(j.root(), &name, &base, mode)?;
                audit(
                    "git.create_branch",
                    &session,
                    &repo,
                    &format!("{name} {base} {mode:?}"),
                );
                Ok(Response::Unit)
            }
            Request::GitBranchRefreshInfo {
                session,
                repo,
                fetch,
            } => {
                let (s, wt, j) = self.worktree_root(&session, &repo)?;
                let base = self.session_base(&s, &wt);
                branch_refresh_info(j.root(), &base, fetch).map(Response::GitRefreshInfo)
            }
            Request::GitBranchRefreshCheck {
                session,
                repo,
                commits,
                move_files,
            } => {
                let (s, wt, j) = self.worktree_root(&session, &repo)?;
                let base = self.session_base(&s, &wt);
                branch_refresh_check(j.root(), &base, &commits, move_files)
                    .map(Response::GitRefreshCheck)
            }
            Request::GitRefreshBranch {
                session,
                repo,
                options,
            } => {
                let (s, wt, j) = self.worktree_root(&session, &repo)?;
                let base = self.session_base(&s, &wt);
                let r = refresh_branch(j.root(), &base, &options)?;
                audit(
                    "git.refresh_branch",
                    &session,
                    &repo,
                    &format!(
                        "{} -> {} ({} commits)",
                        r.previous, r.branch, r.moved_commits
                    ),
                );
                self.set_worktree_branch(&s, &wt, &r.branch)?;
                Ok(Response::GitRefreshResult(r))
            }
            other => Err(ProtoError::new("bad-request").arg("method", other.method())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jail::testutil::{git, init_repo, tmp};

    fn head(r: &Path) -> String {
        git(r, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    fn commit_file(r: &Path, f: &str, text: &str, msg: &str) -> String {
        std::fs::write(r.join(f), text).unwrap();
        git(r, &["add", "-A"]);
        git(r, &["commit", "-qm", msg]);
        head(r)
    }

    fn clone(origin: &Path, dir: &Path) {
        git(
            origin.parent().unwrap(),
            &[
                "clone",
                "-q",
                &origin.to_string_lossy(),
                &dir.to_string_lossy(),
            ],
        );
        git(dir, &["config", "user.email", "t@example.com"]);
        git(dir, &["config", "user.name", "t"]);
        git(dir, &["config", "commit.gpgsign", "false"]);
        git(dir, &["config", "core.autocrlf", "false"]);
    }

    /// A bare `origin` with `main` (one commit of `files`) and a clone `work`.
    fn with_origin(t: &Path, files: &[(&str, &str)]) -> (PathBuf, PathBuf) {
        let seed = t.join("seed");
        init_repo(&seed, files);
        let origin = t.join("origin.git");
        git(
            t,
            &[
                "clone",
                "-q",
                "--bare",
                &seed.to_string_lossy(),
                &origin.to_string_lossy(),
            ],
        );
        let work = t.join("work");
        clone(&origin, &work);
        (origin, work)
    }

    fn porcelain(r: &Path) -> String {
        git(r, &["status", "--porcelain", "--untracked-files=all"])
    }

    fn change(path: &str, orig: Option<&str>, kind: ChangeKind) -> GitChange {
        GitChange {
            path: path.into(),
            orig_path: orig.map(Into::into),
            kind,
        }
    }

    #[test]
    fn discard_each_kind() {
        let t = tmp("ga-discard");
        let repo = t.join("repo");
        init_repo(
            &repo,
            &[("a.txt", "a\n"), ("b.txt", "b\n"), ("old.txt", "old\n")],
        );
        let j = RootJail::new(&repo).unwrap();
        std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
        git(&repo, &["add", "a.txt"]);
        std::fs::write(repo.join("a.txt"), "changed twice\n").unwrap();
        std::fs::remove_file(repo.join("b.txt")).unwrap();
        std::fs::write(repo.join("n.txt"), "new\n").unwrap();
        git(&repo, &["add", "n.txt"]);
        git(&repo, &["mv", "old.txt", "새 이름.txt"]);
        std::fs::create_dir_all(repo.join("u")).unwrap();
        std::fs::write(repo.join("u/x.txt"), "x\n").unwrap();

        discard(&j, &change("a.txt", None, ChangeKind::Modified)).unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "a\n");
        discard(&j, &change("b.txt", None, ChangeKind::Deleted)).unwrap();
        assert!(repo.join("b.txt").exists());
        discard(&j, &change("n.txt", None, ChangeKind::Added)).unwrap();
        assert!(!repo.join("n.txt").exists());
        discard(
            &j,
            &change("새 이름.txt", Some("old.txt"), ChangeKind::Renamed),
        )
        .unwrap();
        assert!(repo.join("old.txt").exists() && !repo.join("새 이름.txt").exists());
        discard(&j, &change("u/x.txt", None, ChangeKind::Untracked)).unwrap();
        // Already gone is fine (rm --force).
        discard(&j, &change("u/x.txt", None, ChangeKind::Untracked)).unwrap();
        assert_eq!(porcelain(&repo), "");

        // Paths are jailed; `.git` is off limits.
        let e = discard(&j, &change("../x", None, ChangeKind::Untracked)).unwrap_err();
        assert_eq!(e.id, "path-denied", "{e}");
        let e = discard(&j, &change(".git/config", None, ChangeKind::Untracked)).unwrap_err();
        assert_eq!(e.id, "path-denied", "{e}");
        assert!(repo.join(".git/config").exists());
    }

    #[test]
    fn stash_list_push_actions() {
        let t = tmp("ga-stash");
        let repo = t.join("repo");
        init_repo(&repo, &[("a.txt", "a\n"), ("b.txt", "b\n")]);
        let j = RootJail::new(&repo).unwrap();
        assert!(stash_list(&repo).is_empty());
        assert_eq!(stash_push(&j, "", None).unwrap_err().id, "git-stash-empty");

        // Partial: only a.txt, with a message.
        std::fs::write(repo.join("a.txt"), "a2\n").unwrap();
        std::fs::write(repo.join("b.txt"), "b2\n").unwrap();
        stash_push(&j, "  메모 하나 ", Some(&["a.txt".to_owned()])).unwrap();
        assert_eq!(porcelain(&repo).trim(), "M b.txt");
        // Everything, untracked included, default message.
        std::fs::write(repo.join("새 파일.txt"), "n\n").unwrap();
        stash_push(&j, "", None).unwrap();
        assert_eq!(porcelain(&repo), "");
        let list = stash_list(&repo);
        assert_eq!(list.len(), 2);
        let (newest, oldest) = (&list[0], &list[1]);
        assert_eq!(
            (oldest.message.as_str(), oldest.branch.as_str(), oldest.auto),
            ("메모 하나", "main", false)
        );
        assert_eq!(oldest.files, [change("a.txt", None, ChangeKind::Modified)]);
        assert_eq!(
            (newest.message.as_str(), newest.branch.as_str()),
            ("", "main")
        );
        assert!(newest.date > 1_600_000_000 && is_full_sha(&newest.sha));
        let mut files: Vec<_> = newest
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.kind))
            .collect();
        files.sort_by_key(|f| f.0);
        assert_eq!(
            files,
            [
                ("b.txt", ChangeKind::Modified),
                ("새 파일.txt", ChangeKind::Untracked)
            ]
        );

        // Bad / unknown ids, path checks, message size.
        assert_eq!(
            stash_action(&repo, "abc", StashAction::Drop)
                .unwrap_err()
                .id,
            "git-bad-rev"
        );
        assert_eq!(
            stash_action(&repo, &"0".repeat(40), StashAction::Drop)
                .unwrap_err()
                .id,
            "git-stash-gone"
        );
        assert_eq!(
            stash_push(&j, "", Some(&["../a".to_owned()]))
                .unwrap_err()
                .id,
            "path-denied"
        );
        assert_eq!(stash_push(&j, "", Some(&[])).unwrap_err().id, "bad-request");
        assert_eq!(
            stash_push(&j, &"x".repeat(10_001), None).unwrap_err().id,
            "bad-request"
        );

        // Apply keeps the stash, pop removes it (addressed by sha, not index).
        let r = stash_action(&repo, &oldest.sha, StashAction::Apply).unwrap();
        assert!(r.conflicts.is_empty());
        assert_eq!(stash_list(&repo).len(), 2);
        git(&repo, &["checkout", "--", "a.txt"]);
        stash_action(&repo, &oldest.sha, StashAction::Pop).unwrap();
        assert_eq!(stash_list(&repo).len(), 1);
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "a2\n");
        git(&repo, &["checkout", "--", "a.txt"]);

        // A conflicting pop returns the paths and keeps the stash.
        commit_file(&repo, "b.txt", "b3\n", "b3");
        let r = stash_action(&repo, &newest.sha, StashAction::Pop).unwrap();
        assert_eq!(r.conflicts, ["b.txt"]);
        assert_eq!(stash_list(&repo).len(), 1);
        git(&repo, &["reset", "-q", "--hard"]);
        git(&repo, &["clean", "-fdq"]);
        stash_action(&repo, &newest.sha, StashAction::Drop).unwrap();
        assert!(stash_list(&repo).is_empty());
    }

    #[test]
    fn stash_subjects() {
        let p = |s: &str| parse_stash_subject(s);
        assert_eq!(p("On main: 메모"), ("main".into(), "메모".into()));
        assert_eq!(
            p("WIP on feat/x: abc123 subject"),
            ("feat/x".into(), String::new())
        );
        assert_eq!(
            p("On (no branch): hermes-desk:"),
            (String::new(), "hermes-desk:".into())
        );
        assert_eq!(p("odd"), (String::new(), "odd".into()));
    }

    #[test]
    fn commit_actions() {
        let t = tmp("ga-commit");
        let repo = t.join("repo");
        init_repo(&repo, &[("a.txt", "1\n")]);
        let c1 = head(&repo);
        let c2 = commit_file(&repo, "a.txt", "2\n", "two");
        git(&repo, &["checkout", "-q", "-b", "side", &c1]);
        let side = commit_file(&repo, "s.txt", "s\n", "side");
        git(&repo, &["checkout", "-q", "main"]);

        // Validation.
        let e = commit_action(&repo, &c2[..7], &CommitAction::Revert).unwrap_err();
        assert_eq!(e.id, "git-bad-rev");
        let e = commit_action(&repo, &"f".repeat(40), &CommitAction::Revert).unwrap_err();
        assert_eq!(e.id, "git-bad-rev");
        for bad in ["", "-x", " x", "a b", "a@{1}", "a..b"] {
            let e =
                commit_action(&repo, &c1, &CommitAction::Branch { name: bad.into() }).unwrap_err();
            assert_eq!(e.id, "git-bad-branch", "{bad}");
        }
        std::fs::write(repo.join("dirty.txt"), "d").unwrap();
        let e = commit_action(&repo, &c2, &CommitAction::Revert).unwrap_err();
        assert_eq!(e.id, "git-dirty");
        std::fs::remove_file(repo.join("dirty.txt")).unwrap();

        // Revert, cherry-pick, branch at a commit, detached checkout.
        commit_action(&repo, &c2, &CommitAction::Revert).unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "1\n");
        assert!(git(&repo, &["log", "-1", "--format=%s"]).starts_with("Revert"));
        commit_action(&repo, &side, &CommitAction::CherryPick).unwrap();
        assert!(repo.join("s.txt").exists());
        commit_action(
            &repo,
            &c1,
            &CommitAction::Branch {
                name: "at-c1".into(),
            },
        )
        .unwrap();
        assert_eq!(
            (current_branch(&repo), head(&repo)),
            ("at-c1".into(), c1.clone())
        );
        assert_eq!(
            commit_action(
                &repo,
                &c1,
                &CommitAction::Branch {
                    name: "at-c1".into()
                }
            )
            .unwrap_err()
            .id,
            "git"
        );
        commit_action(&repo, &c2, &CommitAction::Checkout).unwrap();
        assert_eq!(
            (current_branch(&repo), head(&repo)),
            (String::new(), c2.clone())
        );

        // Merge commits are refused for revert / cherry-pick.
        git(&repo, &["checkout", "-q", "main"]);
        git(
            &repo,
            &["merge", "-q", "--no-ff", "-m", "merge at-c1", "side"],
        );
        let m = head(&repo);
        let e = commit_action(&repo, &m, &CommitAction::Revert).unwrap_err();
        assert_eq!(e.id, "git-merge-commit");

        // An operation in progress.
        let git_dir = PathBuf::from(git(&repo, &["rev-parse", "--absolute-git-dir"]).trim());
        std::fs::write(git_dir.join("MERGE_HEAD"), &m).unwrap();
        let e = commit_action(&repo, &c1, &CommitAction::Checkout).unwrap_err();
        assert_eq!(e.id, "git-in-progress");
        std::fs::remove_file(git_dir.join("MERGE_HEAD")).unwrap();
        // The busy slot was released by every path above.
        assert!(busy_set().lock().unwrap().is_empty());
    }

    #[test]
    fn sync_push_pull() {
        let t = tmp("ga-sync");
        let local = t.join("local");
        init_repo(&local, &[("a.txt", "a\n")]);
        let s = sync_status(&local, true, None).unwrap();
        assert!(s.no_remote && s.ahead == 0);

        let (origin, work) = with_origin(&t, &[("a.txt", "a\n")]);
        let other = t.join("other");
        clone(&origin, &other);
        let s = sync_status(&work, false, None).unwrap();
        assert_eq!(
            (s.ahead, s.behind, s.upstream.as_deref(), s.unpublished),
            (0, 0, Some("origin/main"), false)
        );
        // Ahead locally, behind after the other clone pushes (seen with fetch).
        commit_file(&work, "w.txt", "w\n", "w");
        commit_file(&other, "o.txt", "o\n", "o");
        git(&other, &["push", "-q", "origin", "main"]);
        let s = sync_status(&work, false, None).unwrap();
        assert_eq!((s.ahead, s.behind), (1, 0));
        let s = sync_status(&work, true, None).unwrap();
        assert_eq!((s.ahead, s.behind), (1, 1));
        assert_eq!(pull(&work, PullMode::Merge).unwrap(), "main");
        assert!(work.join("o.txt").exists());
        assert_eq!(push(&work, false, None).unwrap(), "main");
        let s = sync_status(&work, false, None).unwrap();
        assert_eq!((s.ahead, s.behind), (0, 0));

        // Rebase pull with a local edit (autostash), then reset to the upstream.
        commit_file(&other, "o2.txt", "o2\n", "o2");
        git(&other, &["pull", "-q", "--no-rebase", "--no-edit"]);
        git(&other, &["push", "-q", "origin", "main"]);
        commit_file(&work, "w2.txt", "w2\n", "w2");
        std::fs::write(work.join("a.txt"), "edited\n").unwrap();
        pull(&work, PullMode::Rebase).unwrap();
        assert!(work.join("o2.txt").exists());
        assert_eq!(
            std::fs::read_to_string(work.join("a.txt")).unwrap(),
            "edited\n"
        );
        pull(&work, PullMode::Reset).unwrap();
        assert!(!work.join("w2.txt").exists());
        assert_eq!(porcelain(&work), "");

        // An unpublished branch: publish; its upstream deleted on the remote: gone.
        git(&work, &["switch", "-q", "-c", "feature"]);
        commit_file(&work, "f.txt", "f\n", "f");
        let f2 = commit_file(&work, "f2.txt", "f\n", "f2");
        let s = sync_status(&work, true, None).unwrap();
        assert_eq!(
            (s.ahead, s.unpublished, s.gone, s.upstream),
            (2, true, false, None)
        );
        // A merged PR's head leaves its commits out.
        let s = sync_status(&work, false, Some(&f2)).unwrap();
        assert_eq!(s.ahead, 0);
        assert_eq!(
            pull(&work, PullMode::Reset).unwrap_err().id,
            "git-no-upstream"
        );
        assert_eq!(push(&work, false, None).unwrap(), "feature");
        let s = sync_status(&work, false, None).unwrap();
        assert_eq!(
            (s.ahead, s.upstream.as_deref()),
            (0, Some("origin/feature"))
        );
        git(&other, &["push", "-q", "origin", "--delete", "feature"]);
        git(&work, &["fetch", "-q", "--prune"]);
        let s = sync_status(&work, false, None).unwrap();
        assert!(s.unpublished && s.gone, "{s:?}");

        // Verified push: refused when HEAD or the tree moved, else pushes exactly that commit.
        let h = head(&work);
        let exp = |sha: &str, b: &str| PushExpected {
            head_sha: sha.into(),
            branch: b.into(),
        };
        assert_eq!(
            push(&work, false, Some(&exp(&h, "main"))).unwrap_err().id,
            "git-push-changed"
        );
        assert_eq!(
            push(&work, true, Some(&exp(&h, "feature"))).unwrap_err().id,
            "git-push-changed"
        );
        std::fs::write(work.join("dirty.txt"), "d").unwrap();
        assert_eq!(
            push(&work, false, Some(&exp(&h, "feature")))
                .unwrap_err()
                .id,
            "git-push-changed"
        );
        std::fs::remove_file(work.join("dirty.txt")).unwrap();
        push(&work, false, Some(&exp(&h, "feature"))).unwrap();
        assert_eq!(git(&origin, &["rev-parse", "refs/heads/feature"]).trim(), h);

        // Force push with lease after rewriting history.
        git(&work, &["fetch", "-q"]);
        git(&work, &["branch", "-q", "--set-upstream-to=origin/feature"]);
        git(&work, &["reset", "-q", "--hard", "HEAD~1"]);
        assert_eq!(push(&work, false, None).unwrap_err().id, "git");
        push(&work, true, None).unwrap();

        // Detached HEAD.
        git(&work, &["switch", "-q", "--detach", "HEAD"]);
        assert_eq!(push(&work, false, None).unwrap_err().id, "git-detached");
        assert_eq!(pull(&work, PullMode::Merge).unwrap_err().id, "git-detached");
    }

    #[test]
    fn branches_switch_create_fetch() {
        let t = tmp("ga-branches");
        let (origin, main) = with_origin(&t, &[("a.txt", "a\n")]);
        let other = t.join("other");
        clone(&origin, &other);
        git(&other, &["switch", "-q", "-c", "remote-only"]);
        commit_file(&other, "r.txt", "r\n", "r");
        git(&other, &["push", "-q", "origin", "remote-only"]);
        let w1 = t.join("w1");
        let w2 = t.join("w2");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "meninx/s1",
                &w1.to_string_lossy(),
            ],
        );
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "meninx/s2",
                &w2.to_string_lossy(),
            ],
        );

        let b = branches(&w1).unwrap();
        assert_eq!(b.current.as_deref(), Some("meninx/s1"));
        let of = |n: &str| {
            b.local
                .iter()
                .find(|l| l.name == n)
                .unwrap()
                .checkout
                .clone()
        };
        assert_eq!(of("meninx/s1"), BranchCheckout::Here);
        assert!(
            matches!(of("meninx/s2"), BranchCheckout::Worktree(p) if meninx_core::same_path(Path::new(&p), &w2))
        );
        assert!(matches!(of("main"), BranchCheckout::MainRepo(_)));
        // Remote-tracking refs from the clone (before any fetch: no remote-only yet).
        assert!(
            b.remote
                .iter()
                .any(|r| r.short_ref == "origin/main" && r.has_local)
        );
        assert!(!b.remote.iter().any(|r| r.branch == "remote-only"));

        // Fetch (throttled per repository: the other worktree shares it).
        let f = fetch_remotes(&w1);
        assert!(f.ok && f.fetched, "{f:?}");
        let f = fetch_remotes(&w2);
        assert!(f.ok && !f.fetched, "{f:?}");
        let b = branches(&w1).unwrap();
        let r = b.remote.iter().find(|r| r.branch == "remote-only").unwrap();
        assert_eq!((r.remote.as_str(), r.has_local), ("origin", false));

        // Switch: another session's branch is refused, the main repo's is allowed.
        let e = switch_branch(&w1, "meninx/s2", None, SwitchMode::Carry).unwrap_err();
        assert_eq!(e.id, "git-branch-checked-out");
        assert_eq!(
            switch_branch(&w1, "nope", None, SwitchMode::Carry)
                .unwrap_err()
                .id,
            "git-bad-rev"
        );
        assert_eq!(
            switch_branch(&w1, "-x", None, SwitchMode::Carry)
                .unwrap_err()
                .id,
            "git-bad-branch"
        );
        // Stash mode leaves the changes tagged with the branch left.
        std::fs::write(w1.join("a.txt"), "dirty\n").unwrap();
        std::fs::write(w1.join("u.txt"), "u\n").unwrap();
        switch_branch(&w1, "main", None, SwitchMode::Stash).unwrap();
        assert_eq!(current_branch(&w1), "main");
        assert_eq!(porcelain(&w1), "");
        let st = stash_list(&w1);
        assert_eq!(
            (st[0].branch.as_str(), st[0].auto, st[0].message.as_str()),
            ("meninx/s1", true, "")
        );
        switch_branch(&w1, "meninx/s1", None, SwitchMode::Carry).unwrap();
        stash_action(&w1, &st[0].sha, StashAction::Pop).unwrap();
        // Carry mode brings them along.
        switch_branch(&w1, "main", None, SwitchMode::Carry).unwrap();
        assert!(w1.join("u.txt").exists());
        let j = RootJail::new(&w1).unwrap();
        stash_push(&j, "", None).unwrap();
        // A failed stash-mode switch puts the changes back (nothing else popped).
        std::fs::write(w1.join("a.txt"), "dirty2\n").unwrap();
        assert_eq!(
            switch_branch(&w1, "meninx/s2", None, SwitchMode::Stash)
                .unwrap_err()
                .id,
            "git-branch-checked-out"
        );
        assert_eq!(
            std::fs::read_to_string(w1.join("a.txt")).unwrap(),
            "dirty2\n"
        );
        assert_eq!(stash_list(&w1).len(), 1);
        git(&w1, &["checkout", "--", "a.txt"]);

        // Remote branch: created locally, tracking the remote.
        let e = switch_branch(&w1, "remote-only", Some("upstream"), SwitchMode::Carry).unwrap_err();
        assert_eq!(e.id, "git-unknown-remote");
        switch_branch(&w1, "remote-only", Some("origin"), SwitchMode::Carry).unwrap();
        assert_eq!(current_branch(&w1), "remote-only");
        assert!(w1.join("r.txt").exists());
        let s = sync_status(&w1, false, None).unwrap();
        assert_eq!(s.upstream.as_deref(), Some("origin/remote-only"));
        // A remote that has no such branch: nothing is left behind.
        assert!(switch_branch(&w1, "missing", Some("origin"), SwitchMode::Carry).is_err());
        assert!(!has_local_branch(&w1, "missing"));

        // Create from a remote-tracking base (no tracking) and from a local one.
        create_branch(&w1, "새-기능", "origin/main", SwitchMode::Carry).unwrap();
        assert_eq!(current_branch(&w1), "새-기능");
        assert!(try_git(&w1, &["rev-parse", "--abbrev-ref", "@{u}"]).is_none());
        assert_eq!(
            create_branch(&w1, "x", "nope", SwitchMode::Carry)
                .unwrap_err()
                .id,
            "git-bad-rev"
        );
        assert_eq!(
            create_branch(&w1, "x", "-p", SwitchMode::Carry)
                .unwrap_err()
                .id,
            "git-bad-rev"
        );
        assert_eq!(
            create_branch(&w1, "a..b", "main", SwitchMode::Carry)
                .unwrap_err()
                .id,
            "git-bad-branch"
        );
        std::fs::write(w1.join("a.txt"), "dirty3\n").unwrap();
        create_branch(&w1, "from-main", "main", SwitchMode::Stash).unwrap();
        assert_eq!(porcelain(&w1), "");
        assert!(stash_list(&w1)[0].auto);
    }

    #[test]
    fn suggest_names() {
        let taken: HashSet<String> = ["meninx/s1-2".to_owned(), "x-2".to_owned()].into();
        assert_eq!(suggest_name("meninx/s1", &taken), "meninx/s1-3");
        assert_eq!(suggest_name("x-2", &taken), "x-3");
        assert_eq!(suggest_name("v-", &taken), "v--2");
        assert_eq!(suggest_name("", &taken), "branch-2");
    }

    #[test]
    fn branch_refresh() {
        let t = tmp("ga-refresh");
        let (origin, work) = with_origin(&t, &[("a.txt", "a\n"), ("c.txt", "c\n")]);
        let other = t.join("other");
        clone(&origin, &other);
        git(&work, &["switch", "-q", "-c", "meninx/s1"]);
        let f1 = commit_file(&work, "f1.txt", "f1\n", "f1");
        let f2 = commit_file(&work, "f2.txt", "f2\n", "f2");
        let f3 = commit_file(&work, "c.txt", "mine\n", "f3 conflicts");
        // origin/main moves on: f1's content squash-merged, c.txt changed differently.
        std::fs::write(other.join("f1.txt"), "f1\n").unwrap();
        commit_file(&other, "c.txt", "theirs\n", "squash f1 + c");
        git(&other, &["push", "-q", "origin", "main"]);

        let info = branch_refresh_info(&work, "main", true).unwrap();
        assert!(!info.fetch_failed);
        assert_eq!(
            (info.branch.as_str(), info.base.as_str()),
            ("meninx/s1", "main")
        );
        assert_eq!(info.suggested, "meninx/s1-2");
        let listed: Vec<_> = info
            .commits
            .iter()
            .map(|c| (c.full.as_str(), c.in_base))
            .collect();
        assert_eq!(
            listed,
            [
                (f1.as_str(), true),
                (f2.as_str(), false),
                (f3.as_str(), false)
            ]
        );
        assert_eq!(info.more_commits, 0);
        assert!(info.files.is_empty());

        // Dry runs.
        let c = branch_refresh_check(&work, "main", &[f2.clone(), f3.clone()], false).unwrap();
        let conflict = c.conflict.unwrap();
        assert_eq!(
            (conflict.sha.as_str(), conflict.files.as_slice()),
            (f3.as_str(), &["c.txt".to_owned()][..])
        );
        let c = branch_refresh_check(&work, "main", &[f1.clone(), f2.clone()], false).unwrap();
        assert_eq!((c.conflict, c.skipped), (None, vec![f1.clone()]));
        std::fs::write(work.join("c.txt"), "local edit\n").unwrap();
        std::fs::write(work.join("f1.txt"), "untracked clash?\n").unwrap();
        let c = branch_refresh_check(&work, "main", std::slice::from_ref(&f2), true).unwrap();
        assert_eq!(c.files_conflict, ["c.txt"]);
        git(&work, &["checkout", "--", "c.txt", "f1.txt"]);
        assert_eq!(
            branch_refresh_check(&work, "main", &["0".repeat(40)], false)
                .unwrap_err()
                .id,
            "git-refresh-moved"
        );
        assert_eq!(
            branch_refresh_check(&work, "", &[], false).unwrap_err().id,
            "git-no-base"
        );
        assert_eq!(
            branch_refresh_check(&work, "gone", &[], false)
                .unwrap_err()
                .id,
            "git-no-base"
        );

        // Refused runs touch nothing.
        let e = refresh_branch(
            &work,
            "main",
            &BranchRefreshOptions {
                name: None,
                commits: vec![f3.clone()],
                move_files: false,
            },
        )
        .unwrap_err();
        assert_eq!(
            (e.id.as_str(), e.get("files")),
            ("git-refresh-conflict", Some("c.txt"))
        );
        let e = refresh_branch(
            &work,
            "main",
            &BranchRefreshOptions {
                name: Some("main".into()),
                commits: vec![],
                move_files: false,
            },
        )
        .unwrap_err();
        assert_eq!(e.id, "git-branch-exists");
        let e = refresh_branch(
            &work,
            "main",
            &BranchRefreshOptions {
                name: Some("a b".into()),
                commits: vec![],
                move_files: false,
            },
        )
        .unwrap_err();
        assert_eq!(e.id, "git-bad-branch");
        assert_eq!(current_branch(&work), "meninx/s1");

        // The run: f2 moved, f1 skipped as already in, f3 left; edits carried along.
        std::fs::write(work.join("a.txt"), "edited\n").unwrap();
        std::fs::write(work.join("new.txt"), "n\n").unwrap();
        let r = refresh_branch(
            &work,
            "main",
            &BranchRefreshOptions {
                name: None,
                commits: vec![f1.clone(), f2.clone()],
                move_files: true,
            },
        )
        .unwrap();
        assert_eq!(
            (
                r.branch.as_str(),
                r.previous.as_str(),
                r.previous_head.as_str()
            ),
            ("meninx/s1-2", "meninx/s1", f3.as_str())
        );
        assert_eq!(
            (
                r.moved_commits,
                r.skipped_commits,
                r.left_commits,
                r.moved_files,
                r.stashed_files,
                r.files_left_in_stash
            ),
            (1, 1, 1, 2, 0, false)
        );
        assert_eq!(current_branch(&work), "meninx/s1-2");
        assert_eq!(
            std::fs::read_to_string(work.join("c.txt")).unwrap(),
            "theirs\n"
        );
        assert!(work.join("f2.txt").exists() && work.join("new.txt").exists());
        assert_eq!(
            git(&work, &["log", "-1", "--format=%s%x09%an"]).trim(),
            "f2\tt"
        );
        assert_eq!(
            git(&work, &["rev-parse", "HEAD~1"]).trim(),
            git(&work, &["rev-parse", "origin/main"]).trim()
        );
        assert!(try_git(&work, &["rev-parse", "--abbrev-ref", "@{u}"]).is_none());
        assert!(has_local_branch(&work, "meninx/s1"));

        // Edits that conflict on the new branch stay stashed on the old one.
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-qm", "wip"]);
        std::fs::write(work.join("f2.txt"), "conflicting edit\n").unwrap();
        let r = refresh_branch(
            &work,
            "main",
            &BranchRefreshOptions {
                name: Some("meninx/s1-x".into()),
                commits: vec![],
                move_files: true,
            },
        )
        .unwrap();
        assert_eq!(
            (r.moved_files, r.stashed_files, r.files_left_in_stash),
            (0, 1, true)
        );
        assert_eq!(porcelain(&work), "");
        let st = stash_list(&work);
        assert_eq!((st[0].branch.as_str(), st[0].auto), ("meninx/s1-2", true));

        // Detached HEAD.
        git(&work, &["switch", "-q", "--detach"]);
        let e = refresh_branch(&work, "main", &BranchRefreshOptions::default()).unwrap_err();
        assert_eq!(e.id, "git-detached");
    }
}
