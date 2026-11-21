//! Session workspaces, mirroring Hermes Desk's `Workspace` (`src/main/workspace.ts`):
//! a session folder `<data>/workspaces/<id>/` holding one worktree per attached repository
//! (`<folder>/<repo name>` on branch `ide/<id>`), the session guidance (Desk's `HERMES.md`
//! text), attach / detach, and deletion cleanup. Original-repo sessions (Desk's "워크트리 사용"
//! off) run in the registered repository itself and own no files there.

use std::path::{Path, PathBuf};

use crate::error::{Error, IoAction, Result};
use crate::paths::{Paths, ensure_dir};
use crate::registry::{RegisteredRepo, RepoId, RepoStore, detect_base};
use crate::repo::{Repo, normalize_path};
use crate::session::{Session, SessionId, SessionKind, SessionMode, SessionTarget, SessionWorktree};
use crate::worktree::BRANCH_PREFIX;

/// Input of [`create_session`] (what the new-session dialog collects).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSession {
    /// Provider, account and configuration.
    pub target: SessionTarget,
    /// Display title; empty = `"<provider_name> · <repo names>"`.
    pub title: String,
    /// Provider display name for the default title.
    pub provider_name: String,
    /// Repositories to give worktrees (workspace mode; may be empty: an empty workspace).
    pub repos: Vec<RepoId>,
    /// Original-repo mode: run in this registered repository's checkout instead.
    pub original_repo: Option<RepoId>,
}

/// A repository whose worktree could not be created (Desk reports these, the session stays).
#[derive(Debug)]
pub struct RepoFailure {
    /// The repository.
    pub repo: RepoId,
    /// Its name.
    pub name: String,
    /// Why.
    pub error: Error,
}

/// Create a session without persisting it. `precheck` sees the planned session first (e.g.
/// resolve the agent program) so a failure leaves nothing behind. Workspace mode: create the
/// folder, then one worktree per repo in registry order (Desk: `state.repos.filter(...)`);
/// a failing repo is returned in the failures, not fatal.
pub fn create_session(
    paths: &Paths,
    repos: &RepoStore,
    req: &NewSession,
    precheck: impl FnOnce(&Session) -> Result<()>,
) -> Result<(Session, Vec<RepoFailure>)> {
    let id = SessionId::new();
    if let Some(rid) = &req.original_repo {
        let r = repos.require(rid)?.clone();
        let repo = Repo::open(&r.path)?;
        let title = match req.title.trim() {
            "" => format!("{} · {}", req.provider_name, r.name),
            t => t.to_owned(),
        };
        let mut s = Session::new(
            id,
            title,
            req.target.clone(),
            SessionMode::OriginalRepo,
            r.path.clone(),
        );
        s.worktrees.push(SessionWorktree {
            repo: r.id.clone(),
            name: r.name.clone(),
            path: r.path.clone(),
            branch: repo.current_branch()?.unwrap_or_default(),
            base: String::new(),
        });
        precheck(&s)?;
        return Ok((s, Vec::new()));
    }
    let picked: Vec<RegisteredRepo> = repos
        .list()
        .iter()
        .filter(|r| req.repos.contains(&r.id))
        .cloned()
        .collect();
    if let Some(missing) = req.repos.iter().find(|r| repos.get(r).is_none()) {
        return Err(Error::RepoNotFound(missing.to_string()));
    }
    let title = match req.title.trim() {
        "" if picked.is_empty() => req.provider_name.clone(),
        "" => format!(
            "{} · {}",
            req.provider_name,
            picked
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        t => t.to_owned(),
    };
    let root = paths.workspace_dir(id.as_str());
    let mut s = Session::new(
        id,
        title,
        req.target.clone(),
        SessionMode::Workspace,
        root.clone(),
    );
    precheck(&s)?;
    if root.exists() {
        return Err(Error::WorktreeExists(root));
    }
    ensure_dir(&root)?;
    let mut failures = Vec::new();
    for r in picked {
        if let Err(error) = add_worktree(&mut s, &r) {
            failures.push(RepoFailure {
                repo: r.id.clone(),
                name: r.name.clone(),
                error,
            });
        }
    }
    Ok((s, failures))
}

/// Desk's `freeName`: the repo name, suffixed `-2`, `-3`... when the session already has a
/// worktree folder of that name (case-insensitive) or the folder exists.
fn free_name(s: &Session, wanted: &str) -> String {
    let taken = |n: &str| {
        s.worktrees.iter().any(|w| w.name.eq_ignore_ascii_case(n)) || s.root.join(n).exists()
    };
    let mut name = wanted.to_owned();
    let mut i = 2;
    while taken(&name) {
        name = format!("{wanted}-{i}");
        i += 1;
    }
    name
}

fn branch_exists(repo: &Repo, branch: &str) -> bool {
    repo.git([
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ])
    .is_ok()
}

/// Add a worktree of `r` to the workspace session `s` (Desk's `addWorktree` / `worktreeAdd`):
/// `<root>/<free name>` on `ide/<id>` from the repo's base, `--no-track`. An existing `ide/<id>`
/// branch (kept by an earlier detach) is checked out again instead of created. On failure what
/// this call created (folder, registration, new branch) is removed again.
pub fn add_worktree(s: &mut Session, r: &RegisteredRepo) -> Result<()> {
    if s.worktree(&r.id).is_some() {
        return Ok(());
    }
    let repo = Repo::open(&r.path)?;
    let base = detect_base(&repo, r.base_branch.as_deref())?;
    let name = free_name(s, &r.name);
    let path = s.root.join(&name);
    let branch = s.id.branch();
    let existed = branch_exists(&repo, &branch);
    let mut args: Vec<std::ffi::OsString> = vec!["worktree".into(), "add".into()];
    if existed {
        args.push(path.clone().into_os_string());
        args.push(branch.clone().into());
    } else if let Some(start) = &base.start {
        args.extend(["--no-track".into(), "-b".into(), branch.clone().into()]);
        args.push(path.clone().into_os_string());
        args.push(start.into());
    } else {
        args.extend(["--orphan".into(), "-b".into(), branch.clone().into()]);
        args.push(path.clone().into_os_string());
    }
    if let Err(e) = repo.git(&args) {
        let _ = repo.git([
            "worktree".as_ref(),
            "remove".as_ref(),
            "--force".as_ref(),
            path.as_os_str(),
        ]);
        let _ = std::fs::remove_dir_all(&path);
        let _ = repo.prune_worktrees();
        if !existed {
            let _ = repo.git(["branch", "-D", "--", &branch]);
        }
        return Err(e);
    }
    s.worktrees.push(SessionWorktree {
        repo: r.id.clone(),
        name,
        path,
        branch,
        base: base.name,
    });
    Ok(())
}

/// Attach a registered repository to a workspace session (Desk `addRepoToSessions`).
/// Already attached: no-op.
pub fn attach_repo(s: &mut Session, r: &RegisteredRepo) -> Result<()> {
    if !s.is_workspace() {
        return Err(Error::NotAWorkspaceSession(s.id.to_string()));
    }
    add_worktree(s, r)
}

/// Detach a repository from a workspace session: its worktree is removed (`--force`:
/// uncommitted changes are discarded), its branch `ide/<id>` is kept (no Desk equivalent:
/// Desk only adds repos to live sessions).
pub fn detach_repo(paths: &Paths, s: &mut Session, repo: &RepoId, r: Option<&RegisteredRepo>) -> Result<()> {
    if !s.is_workspace() {
        return Err(Error::NotAWorkspaceSession(s.id.to_string()));
    }
    let idx = s
        .worktrees
        .iter()
        .position(|w| &w.repo == repo)
        .ok_or_else(|| Error::RepoNotAttached {
            name: r.map_or_else(|| repo.to_string(), |r| r.name.clone()),
        })?;
    let wt = s.worktrees[idx].clone();
    remove_worktree(paths, &wt, r, false)?;
    s.worktrees.remove(idx);
    Ok(())
}

/// Desk's `isManagedPath`: folders this app may delete outright (inside `workspaces/` or the
/// legacy `worktrees/`).
pub fn is_managed_path(paths: &Paths, p: &Path) -> bool {
    is_inside(p, &paths.workspaces_dir()) || is_inside(p, &paths.worktrees_dir())
}

/// Desk's `removeWorktree`: `git worktree remove --force` (folder delete inside managed dirs
/// when git cannot), `worktree prune`, optionally `branch -D` of an `ide/` branch.
fn remove_worktree(
    paths: &Paths,
    wt: &SessionWorktree,
    r: Option<&RegisteredRepo>,
    delete_branch: bool,
) -> Result<()> {
    let repo = r.and_then(|r| Repo::open(&r.path).ok());
    if wt.path.exists() {
        let removed = repo
            .as_ref()
            .map(|repo| repo.remove_worktree(&wt.path, true));
        match removed {
            Some(Ok(())) => {}
            other => {
                if !is_managed_path(paths, &wt.path) {
                    return match other {
                        Some(Err(e)) => Err(e),
                        _ => Err(Error::NotAGitRepo {
                            path: r.map_or_else(|| wt.path.clone(), |r| r.path.clone()),
                            stderr: "repository is gone".into(),
                        }),
                    };
                }
                std::fs::remove_dir_all(&wt.path)
                    .map_err(|e| Error::io(IoAction::Removing, &wt.path, e))?;
            }
        }
    }
    if let Some(repo) = &repo {
        let _ = repo.prune_worktrees();
        if delete_branch && wt.branch.starts_with(BRANCH_PREFIX) && branch_exists(repo, &wt.branch)
        {
            repo.delete_branch(&wt.branch, true)?;
        }
    }
    Ok(())
}

/// The disk part of deleting a session (Desk's `removeSession`): every worktree removed (and
/// with `remove_worktrees` its `ide/` branch deleted), then the session folder (only inside
/// `workspaces/`) and `sessions/<id>/`. Original-repo and login sessions only lose
/// `sessions/<id>/`. Without `remove_worktrees` the worktrees and folder are kept.
pub fn remove_session_files(
    paths: &Paths,
    s: &Session,
    repos: &RepoStore,
    remove_worktrees: bool,
) -> Result<()> {
    if remove_worktrees && s.is_workspace() {
        for wt in &s.worktrees {
            remove_worktree(paths, wt, repos.get(&wt.repo), true)?;
        }
        if is_inside(&s.root, &paths.workspaces_dir()) && s.root.exists() {
            std::fs::remove_dir_all(&s.root)
                .map_err(|e| Error::io(IoAction::Removing, &s.root, e))?;
        }
    }
    crate::ops::remove_session_dir(paths, s)
}

fn is_inside(path: &Path, dir: &Path) -> bool {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let p = normalize_path(&abs(path));
    let d = normalize_path(&abs(dir));
    p.len() > d.len() && p.starts_with(&d) && p.as_bytes()[d.len()] == b'/'
}

/// The session guidance (Desk's `sessionInstructions()` / `HERMES.md`): what this folder is,
/// its worktrees (folder, repo, branch, base) and the working rules. Providers put it where
/// their CLI reads project context from the cwd. `None` for sessions without a workspace.
pub fn session_guidance(s: &Session, repos: &RepoStore) -> Option<String> {
    if !s.is_workspace() {
        return None;
    }
    let root = s.root.display().to_string().replace('\\', "/");
    let rows: Vec<String> = s
        .worktrees
        .iter()
        .map(|w| {
            let repo = repos
                .get(&w.repo)
                .map_or_else(|| "repository".to_owned(), |r| r.name.clone());
            // A worktree from before workspaces stays where it was: name it by absolute path.
            let folder = if w.path.parent().is_some_and(|p| crate::repo::same_path(p, &s.root)) {
                format!("{}/", w.name)
            } else {
                format!("{}/", w.path.display().to_string().replace('\\', "/"))
            };
            let base = if w.base.is_empty() {
                String::new()
            } else {
                format!(" (base `{}`)", w.base)
            };
            format!("- `{folder}` — {repo}, branch `{}`{base}", w.branch)
        })
        .collect();
    let list = if rows.is_empty() {
        "- (no repositories are currently connected to this workspace)".to_owned()
    } else {
        rows.join("\n")
    };
    let branches: Vec<&str> = {
        let mut b: Vec<&str> = s.worktrees.iter().map(|w| w.branch.as_str()).collect();
        b.dedup();
        b
    };
    let commit = if branches.len() > 1 {
        "listed above".to_owned()
    } else {
        format!("`{}`", branches.first().copied().map_or_else(|| s.id.branch(), str::to_owned))
    };
    Some(format!(
        "# Session workspace {id}\n\n\
         This folder is this session's private workspace. Each subfolder is a git worktree of one \
         repository:\n\n\
         {list}\n\n\
         Rules:\n\
         - Work only inside this folder: create and change files here only.\n\
         - Session output that is not part of a repository's change (analysis notes, reports, \
         plans, scratch files, e.g. `analysis/`) goes in this folder's root, beside the worktrees, \
         written by absolute path: `{root}/analysis/…`.\n\
         - The original repositories are never changed; use the worktrees here. To change several \
         repositories, edit their worktrees here; they can all be modified freely.\n\
         - Commit to each worktree's branch ({commit}). Do not check out branches that are already \
         checked out elsewhere (git will refuse).\n\
         - Before working in a repository, read its own AGENTS.md / CLAUDE.md / README if present.\n\
         - If the task needs another registered repository, ask the user to attach it in the app \
         (repository picker of the session's detail area); it then appears in this list.\n",
        id = s.id,
    ))
}

/// What [`migrate_workspaces`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkspaceMigration {
    /// Repositories registered (from sessions and browse roots).
    pub repos: usize,
    /// Single-repo sessions moved to the workspace layout (files not moved).
    pub sessions: usize,
}

/// Migrate to the registry/workspace model. While `repos.json` does not exist: register
/// every repository a session uses and every `browse_roots` folder that is itself a git top
/// level. Always (idempotent): every session without a `root` gets one: login sessions their
/// old cwd; agent sessions the folder `workspaces/<id>/` with their old worktree recorded (in
/// place) as the one attached repository. A session whose repository is gone keeps a
/// registration of its old path so it can still be deleted.
pub fn migrate_workspaces(
    paths: &Paths,
    repos: &mut RepoStore,
    sessions: &mut crate::session::SessionStore,
    browse_roots: &[PathBuf],
) -> Result<WorkspaceMigration> {
    let mut report = WorkspaceMigration::default();
    let first = !RepoStore::exists(paths);
    let before = repos.list().len();
    for s in sessions.all_mut() {
        if !s.root.as_os_str().is_empty() {
            continue;
        }
        let old_cwd = s.legacy_worktree_path.take().unwrap_or_default();
        let old_repo = s.legacy_repo_path.take();
        let old_branch = s.legacy_branch.take().unwrap_or_default();
        if s.kind == SessionKind::Login || old_repo.is_none() {
            s.root = old_cwd;
            continue;
        }
        let old_repo = old_repo.unwrap_or_default();
        let r = match repos.register(&old_repo) {
            Ok(r) => r,
            Err(Error::NotAGitRepo { .. }) => repos.adopt_missing(&old_repo)?,
            Err(e) => return Err(e),
        };
        let root = paths.workspace_dir(s.id.as_str());
        ensure_dir(&root)?;
        s.root = root;
        s.mode = SessionMode::Workspace;
        s.worktrees = vec![SessionWorktree {
            repo: r.id.clone(),
            name: r.name.clone(),
            path: old_cwd,
            branch: old_branch,
            base: String::new(),
        }];
        report.sessions += 1;
    }
    if first {
        for root in browse_roots {
            if let Ok(repo) = Repo::open(root)
                && crate::repo::same_path(repo.root(), root)
            {
                repos.register(root)?;
            }
        }
        repos.save()?;
    }
    report.repos = repos.list().len() - before;
    if report.sessions > 0 {
        sessions.persist()?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;
    use crate::repo::{same_path, test_repo};
    use crate::session::SessionStore;
    use crate::worktree::worktree_exists;

    fn target() -> SessionTarget {
        SessionTarget {
            provider: "shell".into(),
            account: None,
            configuration: "default".into(),
        }
    }

    fn req(repos: Vec<RepoId>) -> NewSession {
        NewSession {
            target: target(),
            title: String::new(),
            provider_name: "Shell".into(),
            repos,
            original_repo: None,
        }
    }

    #[test]
    fn multi_repo_create_attach_detach_delete() {
        let t = TempDir::new("ws");
        let paths = Paths::new(t.0.join("data"));
        let a = t.0.join("one").join("app");
        let b = t.0.join("two").join("app");
        let c = t.0.join("lib");
        for d in [&a, &b, &c] {
            test_repo::init(d);
        }
        let mut repos = RepoStore::open(&paths).unwrap();
        let ra = repos.register(&a).unwrap();
        let rb = repos.register(&b).unwrap();
        let rc = repos.register(&c).unwrap();

        // Two repos with the same folder name: app, app-2.
        let (mut s, failures) =
            create_session(&paths, &repos, &req(vec![rb.id.clone(), ra.id.clone()]), |_| Ok(()))
                .unwrap();
        assert!(failures.is_empty());
        assert_eq!(s.title, "Shell · app, app");
        assert!(same_path(&s.root, &paths.workspace_dir(s.id.as_str())));
        let names: Vec<&str> = s.worktrees.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["app", "app-2"]);
        for w in &s.worktrees {
            assert!(worktree_exists(&w.path));
            assert_eq!(w.branch, s.id.branch());
            assert_eq!(w.base, "main");
            assert!(w.path.starts_with(&s.root));
        }
        let g = session_guidance(&s, &repos).unwrap();
        assert!(g.contains("`app/`") && g.contains("`app-2/`"), "{g}");
        assert!(g.contains(&format!("`{}`", s.id.branch())), "{g}");

        // Attach, attach again (no-op), detach (branch kept), re-attach (branch reused).
        attach_repo(&mut s, &rc).unwrap();
        attach_repo(&mut s, &rc).unwrap();
        assert_eq!(s.worktrees.len(), 3);
        let lib = s.worktree(&rc.id).unwrap().clone();
        assert_eq!(lib.name, "lib");
        std::fs::write(lib.path.join("x.txt"), "dirty").unwrap();
        detach_repo(&paths, &mut s, &rc.id, Some(&rc)).unwrap();
        assert!(!lib.path.exists());
        assert!(branch_exists(&Repo::open(&c).unwrap(), &s.id.branch()));
        assert!(matches!(
            detach_repo(&paths, &mut s, &rc.id, Some(&rc)),
            Err(Error::RepoNotAttached { .. })
        ));
        attach_repo(&mut s, &rc).unwrap();
        assert!(worktree_exists(&s.worktree(&rc.id).unwrap().path));

        // Delete: worktrees, branches and the folder are gone; originals untouched.
        remove_session_files(&paths, &s, &repos, true).unwrap();
        assert!(!s.root.exists());
        for d in [&a, &b, &c] {
            let repo = Repo::open(d).unwrap();
            assert!(!branch_exists(&repo, &s.id.branch()));
            assert_eq!(repo.list_worktrees().unwrap().len(), 1);
            assert!(d.join("README.md").is_file());
        }
    }

    #[test]
    fn empty_workspace_failures_and_original_repo() {
        let t = TempDir::new("ws2");
        let paths = Paths::new(t.0.join("data"));
        let a = t.0.join("a");
        test_repo::init(&a);
        let mut repos = RepoStore::open(&paths).unwrap();
        let ra = repos.register(&a).unwrap();

        // Zero repos: an empty workspace (Desk allows it).
        let (s, f) = create_session(&paths, &repos, &req(vec![]), |_| Ok(())).unwrap();
        assert!(f.is_empty() && s.worktrees.is_empty() && s.root.is_dir());
        assert!(session_guidance(&s, &repos).unwrap().contains("no repositories"));

        // Precheck failure leaves nothing.
        let err = create_session(&paths, &repos, &req(vec![ra.id.clone()]), |_| {
            Err(Error::ProgramNotFound { program: "x".into() })
        })
        .unwrap_err();
        assert!(matches!(err, Error::ProgramNotFound { .. }));
        assert_eq!(Repo::open(&a).unwrap().list_worktrees().unwrap().len(), 1);

        // Unknown repo id.
        assert!(matches!(
            create_session(&paths, &repos, &req(vec![RepoId::new()]), |_| Ok(())),
            Err(Error::RepoNotFound(_))
        ));

        // A repo that fails (moved away after registering) is reported, the session stays.
        let gone = t.0.join("gone");
        test_repo::init(&gone);
        let rg = repos.register(&gone).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();
        let (s2, f) =
            create_session(&paths, &repos, &req(vec![ra.id.clone(), rg.id.clone()]), |_| Ok(()))
                .unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].repo, rg.id);
        assert_eq!(s2.worktrees.len(), 1);
        remove_session_files(&paths, &s2, &repos, true).unwrap();

        // Original-repo mode: runs in the repo, nothing created; deleting keeps the repo.
        let (o, _) = create_session(
            &paths,
            &repos,
            &NewSession {
                original_repo: Some(ra.id.clone()),
                ..req(vec![])
            },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(o.mode, SessionMode::OriginalRepo);
        assert!(same_path(&o.root, &a));
        assert_eq!(o.worktrees[0].branch, "main");
        assert!(session_guidance(&o, &repos).is_none());
        let mut o2 = o.clone();
        assert!(matches!(
            attach_repo(&mut o2, &ra),
            Err(Error::NotAWorkspaceSession(_))
        ));
        remove_session_files(&paths, &o, &repos, true).unwrap();
        assert!(a.join("README.md").is_file());
        assert_eq!(Repo::open(&a).unwrap().list_worktrees().unwrap().len(), 1);
    }

    #[test]
    fn migration_from_single_repo_sessions() {
        let t = TempDir::new("ws-mig");
        let paths = Paths::new(t.0.join("data"));
        let a = t.0.join("src").join("app");
        test_repo::init(&a);
        let other = t.0.join("src").join("other");
        test_repo::init(&other);
        // A legacy worktree + session file written by the old layout.
        let id = SessionId::new();
        let wt = Repo::open(&a)
            .unwrap()
            .create_worktree(&paths, &id, None)
            .unwrap();
        let login = SessionId::new();
        let login_cwd = paths.session_dir(login.as_str()).join("cwd");
        std::fs::create_dir_all(&login_cwd).unwrap();
        let json = serde_json::json!({ "sessions": [
            { "id": id.as_str(), "title": "old", "provider": "shell", "configuration": "default",
              "repo_path": a, "worktree_path": wt.path, "branch": wt.branch,
              "created_at": 1, "last_active": 1 },
            { "id": login.as_str(), "title": "login", "kind": "login", "provider": "claude",
              "account": "x", "configuration": "", "repo_path": login_cwd,
              "worktree_path": login_cwd, "branch": "", "created_at": 1, "last_active": 1 }
        ]});
        std::fs::create_dir_all(paths.root()).unwrap();
        std::fs::write(paths.sessions_file(), json.to_string()).unwrap();

        let mut repos = RepoStore::open(&paths).unwrap();
        let mut store = SessionStore::open(&paths).unwrap();
        let roots = vec![t.0.join("src"), other.clone()];
        let r = migrate_workspaces(&paths, &mut repos, &mut store, &roots).unwrap();
        // app (used by a session) + other (a browse root that is a repo); `src` is no repo.
        assert_eq!(r, WorkspaceMigration { repos: 2, sessions: 1 });
        let s = store.get(&id).unwrap().clone();
        assert!(same_path(&s.root, &paths.workspace_dir(id.as_str())));
        assert!(s.root.is_dir());
        assert_eq!(s.worktrees.len(), 1);
        assert!(same_path(&s.worktrees[0].path, &wt.path));
        assert_eq!(s.worktrees[0].branch, wt.branch);
        assert!(worktree_exists(&wt.path));
        let g = session_guidance(&s, &repos).unwrap();
        assert!(g.contains(&wt.path.display().to_string().replace('\\', "/")), "{g}");
        let l = store.get(&login).unwrap();
        assert!(same_path(&l.root, &login_cwd) && l.worktrees.is_empty());
        // Saved without the legacy fields; a second run changes nothing.
        let reopened = SessionStore::open(&paths).unwrap();
        assert_eq!(reopened.get(&id).unwrap().worktrees, s.worktrees);
        let text = std::fs::read_to_string(paths.sessions_file()).unwrap();
        assert!(!text.contains("worktree_path"), "{text}");
        let mut store = reopened;
        let mut repos = RepoStore::open(&paths).unwrap();
        assert_eq!(
            migrate_workspaces(&paths, &mut repos, &mut store, &roots).unwrap(),
            WorkspaceMigration::default()
        );
        // Deleting the migrated session removes the old worktree and branch.
        remove_session_files(&paths, &s, &repos, true).unwrap();
        assert!(!wt.path.exists() && !s.root.exists());
        assert!(!branch_exists(&Repo::open(&a).unwrap(), &wt.branch));
    }
}
