//! Session families (`docs/session-family.md`): a workspace session's child sessions either
//! share its folder and worktrees or get their own worktrees branched from its branches; the
//! parent merges a child back, a child syncs from the parent. At most two levels below a root.

use std::path::Path;

use crate::error::{Error, Result};
use crate::paths::{Paths, ensure_dir};
use crate::registry::{RepoId, RepoStore};
use crate::session::{Session, SessionId, SessionMode, SessionTarget};
use crate::workspace::{RepoFailure, add_child_worktree};

/// Levels below a root session (root -> child -> grandchild).
pub const MAX_DEPTH: usize = 2;

/// How many ancestors `s` has (0 = a root).
pub fn depth(sessions: &[Session], s: &Session) -> usize {
    let mut n = 0;
    let mut cur = s.parent.clone();
    while let Some(p) = cur {
        n += 1;
        if n > MAX_DEPTH + 1 {
            break;
        }
        cur = sessions.iter().find(|x| x.id == p).and_then(|x| x.parent.clone());
    }
    n
}

/// The children of `id`.
pub fn children<'a>(sessions: &'a [Session], id: &SessionId) -> Vec<&'a Session> {
    sessions
        .iter()
        .filter(|s| s.parent.as_ref() == Some(id))
        .collect()
}

/// Related sessions of `s`: its parent, its siblings and its children (in list order).
pub fn related<'a>(sessions: &'a [Session], s: &Session) -> Vec<&'a Session> {
    sessions
        .iter()
        .filter(|x| x.id != s.id)
        .filter(|x| {
            s.parent.as_ref() == Some(&x.id)
                || x.parent.as_ref() == Some(&s.id)
                || (s.parent.is_some() && x.parent == s.parent)
        })
        .collect()
}

/// Can `parent` get a child?
pub fn check_parent(sessions: &[Session], parent: &Session) -> Result<()> {
    if parent.mode != SessionMode::Workspace || !parent.root.is_absolute() {
        return Err(Error::FamilyNeedsWorkspace(parent.id.to_string()));
    }
    if depth(sessions, parent) + 1 > MAX_DEPTH {
        return Err(Error::FamilyTooDeep);
    }
    Ok(())
}

/// What a new child needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChild {
    /// Provider, account and configuration (the parent's unless chosen otherwise).
    pub target: SessionTarget,
    /// Title; empty = `"<parent title> · <n>"`.
    pub title: String,
    /// Work in the parent's folder and worktrees.
    pub shared: bool,
    /// Separate children: the parent's repositories to give worktrees (empty = all of them).
    pub repos: Vec<RepoId>,
}

/// Create a child of `parent` without persisting it. `precheck` sees the planned session first.
/// Shared: the parent's folder and worktrees as they are. Separate: its own folder, one
/// worktree per picked repo of the parent, started at the parent's branch (a failing repo is
/// returned, not fatal).
pub fn create_child(
    paths: &Paths,
    repos: &RepoStore,
    sessions: &[Session],
    parent: &Session,
    req: &NewChild,
    precheck: impl FnOnce(&Session) -> Result<()>,
) -> Result<(Session, Vec<RepoFailure>)> {
    check_parent(sessions, parent)?;
    for r in &req.repos {
        if parent.worktree(r).is_none() {
            let name = repos.get(r).map_or_else(|| r.to_string(), |x| x.name.clone());
            return Err(Error::RepoNotInParent { name });
        }
    }
    let id = SessionId::new();
    let title = match req.title.trim() {
        "" => format!(
            "{} · {}",
            parent.title,
            children(sessions, &parent.id).len() + 1
        ),
        t => t.to_owned(),
    };
    let root = if req.shared {
        parent.root.clone()
    } else {
        paths.workspace_dir(id.as_str())
    };
    let mut s = Session::new(
        id,
        title,
        req.target.clone(),
        SessionMode::Workspace,
        root.clone(),
    );
    s.parent = Some(parent.id.clone());
    s.shared = req.shared;
    precheck(&s)?;
    if req.shared {
        s.worktrees = parent.worktrees.clone();
        s.bases = parent.bases.clone();
        return Ok((s, Vec::new()));
    }
    if root.exists() {
        return Err(Error::WorktreeExists(root));
    }
    ensure_dir(&root)?;
    let mut failures = Vec::new();
    for w in &parent.worktrees {
        if !req.repos.is_empty() && !req.repos.contains(&w.repo) {
            continue;
        }
        let Some(r) = repos.get(&w.repo) else {
            continue;
        };
        if let Err(error) = add_child_worktree(&mut s, r, &w.branch) {
            failures.push(RepoFailure {
                repo: r.id.clone(),
                name: r.name.clone(),
                error,
            });
        }
    }
    Ok((s, failures))
}

/// Attach one of the parent's repositories to a separate child (its base: the parent's
/// branch). Shared children cannot attach repositories.
pub fn attach_child_repo(
    child: &mut Session,
    parent: &Session,
    repos: &RepoStore,
    repo: &RepoId,
) -> Result<()> {
    if child.shared {
        return Err(Error::SharedChildRepos);
    }
    let r = repos.require(repo)?;
    let w = parent.worktree(repo).ok_or_else(|| Error::RepoNotInParent {
        name: r.name.clone(),
    })?;
    add_child_worktree(child, r, &w.branch)
}

/// Separate children of `parent` that work in `repo`.
pub fn children_using(sessions: &[Session], parent: &SessionId, repo: &RepoId) -> usize {
    children(sessions, parent)
        .iter()
        .filter(|c| !c.shared && c.worktree(repo).is_some())
        .count()
}

/// What merging a branch into a worktree did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Merged (a merge or squash commit was made).
    Merged,
    /// Nothing to merge.
    UpToDate,
    /// The merge stopped at conflicts in these files (left in progress to resolve).
    Conflicts(Vec<String>),
    /// Git refused or failed (e.g. local changes in the way); nothing changed.
    Failed(String),
}

fn run(dir: &Path, args: &[&str]) -> crate::repo::GitOutput {
    match crate::repo::git_raw(dir, args, &[]) {
        Ok(o) => o,
        Err(e) => crate::repo::GitOutput {
            code: None,
            stdout: Vec::new(),
            stderr: e.to_string().into_bytes(),
        },
    }
}

fn conflicted(dir: &Path) -> Vec<String> {
    let out = run(dir, &["diff", "--name-only", "--diff-filter=U"]);
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Merge `branch` into the worktree at `dir`: `--no-ff` with `message`, or `--squash` and a
/// commit with `message`. Uncommitted work is never merged (only commits on `branch`).
pub fn merge_branch(dir: &Path, branch: &str, squash: bool, message: &str) -> MergeOutcome {
    let ahead = run(dir, &["rev-list", "--count", &format!("HEAD..{branch}")]);
    if ahead.code != Some(0) {
        return MergeOutcome::Failed(String::from_utf8_lossy(&ahead.stderr).trim().to_owned());
    }
    if String::from_utf8_lossy(&ahead.stdout).trim() == "0" {
        return MergeOutcome::UpToDate;
    }
    let out = if squash {
        run(dir, &["merge", "--squash", branch])
    } else {
        run(dir, &["merge", "--no-ff", "-m", message, branch])
    };
    if out.code != Some(0) {
        let files = conflicted(dir);
        if !files.is_empty() {
            return MergeOutcome::Conflicts(files);
        }
        let mut why = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        if why.is_empty() {
            why = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        }
        return MergeOutcome::Failed(why);
    }
    if squash {
        let staged = run(dir, &["diff", "--cached", "--quiet"]);
        if staged.code == Some(0) {
            return MergeOutcome::UpToDate;
        }
        let c = run(dir, &["commit", "-m", message]);
        if c.code != Some(0) {
            return MergeOutcome::Failed(String::from_utf8_lossy(&c.stderr).trim().to_owned());
        }
    }
    MergeOutcome::Merged
}

/// The family part of a session's guidance: who its parent / children are and how work flows
/// between them (`None` for a session without family).
pub fn family_guidance(s: &Session, parent: Option<&Session>, kids: &[&Session]) -> Option<String> {
    if parent.is_none() && kids.is_empty() {
        return None;
    }
    let mut t = String::from("## Session family\n");
    if let Some(p) = parent {
        if s.shared {
            t.push_str(&format!(
                "You are a child session of \"{}\" (`{}`) and share its folder and worktrees. \
                 Only the session holding the folder's write access may edit files (the \
                 parent, unless it was handed over): ask for it with \
                 `ide_request_write_access`, give it back with `ide_release_write_access`, and \
                 prefer reading, reviewing and planning otherwise.\n",
                p.title, p.id
            ));
        } else {
            t.push_str(&format!(
                "You are a child session of \"{}\" (`{}`) with your own worktrees: each branch \
                 started at the parent's branch, which is your base. Commit your work here; the \
                 parent merges it (`ide_merge_child`). Get the parent's newer commits with \
                 `ide_sync_from_parent`. Tell the parent when you are done \
                 (`ide_send_to_session`).\n",
                p.title, p.id
            ));
        }
    }
    if !kids.is_empty() {
        t.push_str("Your child sessions (see `ide_list_related_sessions`):\n");
        for c in kids {
            t.push_str(&format!(
                "- \"{}\" (`{}`), {}\n",
                c.title,
                c.id,
                if c.shared {
                    "shares your folder"
                } else {
                    "own worktrees branched from yours"
                }
            ));
        }
        t.push_str(
            "Merge a finished child with `ide_merge_child`; on conflicts, read both sides \
             (`git diff`, the child's commits) and ask the child before resolving.\n",
        );
        if kids.iter().any(|c| c.shared) {
            t.push_str(
                "You hold this folder's write access by default; a shared child that needs to \
                 edit asks for it when you are idle. Take it back with \
                 `ide_request_write_access`.\n",
            );
        }
    }
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let o = crate::repo::git_raw(dir, args, &[]).unwrap();
        assert_eq!(o.code, Some(0), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    }

    fn repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        git(dir, &["config", "user.email", "t@example.com"]);
        git(dir, &["config", "user.name", "t"]);
        git(dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);
    }

    fn target() -> SessionTarget {
        SessionTarget {
            provider: "shell".into(),
            account: None,
            configuration: "default".into(),
        }
    }

    #[test]
    fn family_lifecycle() {
        let t = TempDir::new("family");
        let paths = Paths::new(t.0.join("data"));
        let r1 = t.0.join("r1");
        let r2 = t.0.join("r2");
        repo(&r1);
        repo(&r2);
        let mut repos = RepoStore::open(&paths).unwrap();
        let id1 = repos.register(&r1, false).unwrap().id.clone();
        let id2 = repos.register(&r2, false).unwrap().id.clone();
        let (parent, f, _) = crate::workspace::create_session(
            &paths,
            &repos,
            &crate::workspace::NewSession {
                target: target(),
                title: "P".into(),
                provider_name: "Shell".into(),
                repos: vec![id1.clone()],
                original_repo: None,
                refresh_base: false,
                background: false,
                bases: Default::default(),
            },
            |_| Ok(()),
        )
        .unwrap();
        assert!(f.is_empty());
        let pw = parent.worktree(&id1).unwrap().clone();
        // The parent commits first: the child starts from it.
        std::fs::write(pw.path.join("p.txt"), "p\n").unwrap();
        git(&pw.path, &["add", "."]);
        git(&pw.path, &["commit", "-q", "-m", "parent work"]);
        let mut all = vec![parent.clone()];

        // Only the parent's repos.
        let bad = NewChild {
            target: target(),
            title: String::new(),
            shared: false,
            repos: vec![id2.clone()],
        };
        assert!(matches!(
            create_child(&paths, &repos, &all, &parent, &bad, |_| Ok(())),
            Err(Error::RepoNotInParent { .. })
        ));
        let req = NewChild {
            repos: Vec::new(),
            ..bad
        };
        let (child, f) = create_child(&paths, &repos, &all, &parent, &req, |_| Ok(())).unwrap();
        assert!(f.is_empty());
        assert_eq!(child.title, "P · 1");
        assert_eq!(child.parent.as_ref(), Some(&parent.id));
        let cw = child.worktree(&id1).unwrap().clone();
        assert_eq!(cw.base, pw.branch, "base = the parent's branch");
        assert!(cw.path.join("p.txt").is_file(), "started at the parent's tip");
        assert!(cw.path.starts_with(&child.root) && child.root != parent.root);
        all.push(child.clone());

        // Shared: the parent's folder and worktrees, nothing on disk.
        let (shared, _) = create_child(
            &paths,
            &repos,
            &all,
            &parent,
            &NewChild {
                target: target(),
                title: "S".into(),
                shared: true,
                repos: Vec::new(),
            },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!((shared.root.clone(), shared.worktrees.clone()), (parent.root.clone(), parent.worktrees.clone()));
        all.push(shared.clone());
        assert_eq!(children(&all, &parent.id).len(), 2);
        assert_eq!(children_using(&all, &parent.id, &id1), 1, "shared children do not count");
        let mut r: Vec<&str> = related(&all, &child).iter().map(|s| s.title.as_str()).collect();
        r.sort();
        assert_eq!(r, ["P", "S"]);

        // Depth: root -> child -> grandchild; no deeper.
        let (grand, _) = create_child(&paths, &repos, &all, &child, &req, |_| Ok(())).unwrap();
        all.push(grand.clone());
        assert_eq!(depth(&all, &grand), 2);
        assert!(matches!(
            create_child(&paths, &repos, &all, &grand, &req, |_| Ok(())),
            Err(Error::FamilyTooDeep)
        ));

        // The child commits; the parent merges it.
        std::fs::write(cw.path.join("c.txt"), "c\n").unwrap();
        git(&cw.path, &["add", "."]);
        git(&cw.path, &["commit", "-q", "-m", "child work"]);
        assert_eq!(merge_branch(&pw.path, &cw.branch, false, "Merge child"), MergeOutcome::Merged);
        assert!(pw.path.join("c.txt").is_file());
        assert_eq!(merge_branch(&pw.path, &cw.branch, false, "Merge child"), MergeOutcome::UpToDate);

        // Both edit the same line: conflicts are reported and left to resolve.
        std::fs::write(cw.path.join("a.txt"), "child\n").unwrap();
        git(&cw.path, &["commit", "-qam", "child edit"]);
        std::fs::write(pw.path.join("a.txt"), "parent\n").unwrap();
        git(&pw.path, &["commit", "-qam", "parent edit"]);
        assert_eq!(
            merge_branch(&pw.path, &cw.branch, true, "Squash child"),
            MergeOutcome::Conflicts(vec!["a.txt".into()])
        );
        git(&pw.path, &["reset", "-q", "--hard", "HEAD"]);
        // The child syncs from the parent the same way (conflict again here).
        assert!(matches!(
            merge_branch(&cw.path, &pw.branch, false, "Sync"),
            MergeOutcome::Conflicts(_)
        ));
        git(&cw.path, &["merge", "--abort"]);
        // Local changes in the way: refused, nothing changed.
        std::fs::write(pw.path.join("a.txt"), "dirty\n").unwrap();
        assert!(matches!(
            merge_branch(&pw.path, &cw.branch, false, "x"),
            MergeOutcome::Failed(_)
        ));

        // Guidance.
        let g = family_guidance(&child, Some(&parent), &[&grand]).unwrap();
        assert!(g.contains("child session of \"P\"") && g.contains("ide_merge_child"), "{g}");
        assert!(family_guidance(&shared, Some(&parent), &[]).unwrap().contains("share its folder"));
        let mut lone = parent.clone();
        lone.id = SessionId::new();
        assert!(family_guidance(&lone, None, &[]).is_none());

        // Shared children cannot attach repos; separate ones only the parent's.
        let mut s2 = shared.clone();
        assert!(matches!(
            attach_child_repo(&mut s2, &parent, &repos, &id1),
            Err(Error::SharedChildRepos)
        ));
        let mut c2 = child.clone();
        assert!(matches!(
            attach_child_repo(&mut c2, &parent, &repos, &id2),
            Err(Error::RepoNotInParent { .. })
        ));
    }

    #[test]
    fn needs_a_workspace_parent() {
        let mut p = Session::new(
            SessionId::new(),
            "p",
            target(),
            SessionMode::OriginalRepo,
            std::env::temp_dir(),
        );
        assert!(matches!(check_parent(&[], &p), Err(Error::FamilyNeedsWorkspace(_))));
        p.mode = SessionMode::Workspace;
        assert!(check_parent(&[], &p).is_ok());
    }
}
