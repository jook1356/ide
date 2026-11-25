//! Session workspaces, mirroring Hermes Desk's `Workspace` (`src/main/workspace.ts`):
//! a session folder `<data>/workspaces/<id>/` holding one worktree per attached repository
//! (`<folder>/<repo name>` on branch `meninx/<id>`), the session guidance (Desk's `HERMES.md`
//! text), attach / detach, and deletion cleanup. Original-repo sessions (Desk's "워크트리 사용"
//! off) run in the registered repository itself and own no files there.

use std::path::{Path, PathBuf};

use crate::error::{Error, IoAction, Result};
use crate::paths::{Paths, ensure_dir};
use crate::base::{BasePlan, plan_base, repo_default_base, short_branch};
use crate::registry::{RegisteredRepo, RepoId, RepoStore, detect_base};
use crate::repo::{Repo, normalize_path};
use crate::session::{AutoTitle, Session, SessionId, SessionKind, SessionMode, SessionTarget, SessionWorktree};
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
    /// 기준 브랜치 최신화: fetch each base from its remote first (Desk default: on).
    pub refresh_base: bool,
    /// Base branch picked per repository (origin's branch name); others use the repo default.
    pub bases: std::collections::BTreeMap<RepoId, String>,
    /// Create only the folder now and record the worktrees as [`Session::pending`] (with
    /// [`PENDING_MARKER`] in the folder); the caller creates them afterwards with
    /// [`add_worktree`], so the session (and its agent) can start right away.
    pub background: bool,
}

/// File in a session folder while its worktrees are still being created in the background
/// (lists their folders, one per line). The session guidance tells the agent to wait for it to
/// disappear before it works in those repositories.
pub const PENDING_MARKER: &str = ".worktrees-pending";

/// Write [`PENDING_MARKER`] in a workspace session's folder while it has pending worktrees,
/// remove it once it has none.
pub fn write_pending_marker(s: &Session) -> Result<()> {
    if !s.is_workspace() || s.shared {
        return Ok(());
    }
    let path = s.root.join(PENDING_MARKER);
    if s.pending.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(Error::io(IoAction::Removing, &path, e))
            }
            _ => Ok(()),
        };
    }
    let body: String = s.pending.iter().map(|p| format!("{}/\n", p.name)).collect();
    std::fs::write(&path, body).map_err(|e| Error::io(IoAction::Writing, &path, e))
}

/// What [`create_session`] returns: the session, the repos whose worktree failed, and each
/// created worktree's base plan.
pub type Created = (Session, Vec<RepoFailure>, Vec<(RepoId, BasePlan)>);

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
/// Also returns each created worktree's [`BasePlan`] (the caller records rule picks with
/// [`RepoStore::record_plan`]).
pub fn create_session(
    paths: &Paths,
    repos: &RepoStore,
    req: &NewSession,
    precheck: impl FnOnce(&Session) -> Result<()>,
) -> Result<Created> {
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
        if req.title.trim().is_empty() {
            s.auto_title = AutoTitle::Pending;
        }
        s.worktrees.push(SessionWorktree {
            repo: r.id.clone(),
            name: r.name.clone(),
            path: r.path.clone(),
            branch: repo.current_branch()?.unwrap_or_default(),
            base: String::new(),
        });
        precheck(&s)?;
        return Ok((s, Vec::new(), Vec::new()));
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
    if req.title.trim().is_empty() {
        s.auto_title = AutoTitle::Pending;
    }
    precheck(&s)?;
    if root.exists() {
        return Err(Error::WorktreeExists(root));
    }
    ensure_dir(&root)?;
    let mut failures = Vec::new();
    let mut plans = Vec::new();
    if req.background {
        for r in picked {
            let name = free_name(&s, &r.name);
            s.pending.push(crate::session::PendingWorktree {
                repo: r.id.clone(),
                name,
                pick: req.bases.get(&r.id).cloned(),
                refresh: req.refresh_base,
            });
        }
        write_pending_marker(&s)?;
        return Ok((s, failures, plans));
    }
    for r in picked {
        let pick = req.bases.get(&r.id).map(String::as_str);
        match add_worktree(&mut s, &r, req.refresh_base, pick) {
            Ok(plan) => plans.push((r.id.clone(), plan)),
            Err(error) => failures.push(RepoFailure {
                repo: r.id.clone(),
                name: r.name.clone(),
                error,
            }),
        }
    }
    Ok((s, failures, plans))
}

/// Desk's `freeName`: the repo name, suffixed `-2`, `-3`... when the session already has a
/// worktree folder of that name (case-insensitive) or the folder exists.
fn free_name(s: &Session, wanted: &str) -> String {
    let taken = |n: &str| {
        s.worktrees.iter().any(|w| w.name.eq_ignore_ascii_case(n))
            || s.pending.iter().any(|p| p.name.eq_ignore_ascii_case(n))
            || s.root.join(n).exists()
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
/// `<root>/<free name>` on `meninx/<id>` from the base chosen by [`plan_base`] (`refresh` = 기준
/// 브랜치 최신화, `pick` = a base picked for this session), `--no-track`; the session keeps that
/// base (`Session::bases`). An existing `meninx/<id>` branch (kept by an earlier detach) is checked
/// out again instead of created (no fetch). On failure what this call created (folder,
/// registration, new branch) is removed again.
pub fn add_worktree(
    s: &mut Session,
    r: &RegisteredRepo,
    refresh: bool,
    pick: Option<&str>,
) -> Result<BasePlan> {
    if let Some(w) = s.worktree(&r.id) {
        return Ok(BasePlan {
            name: w.base.clone(),
            start: None,
            rule_resolved: None,
            fixed_upgrade: None,
        });
    }
    let repo = Repo::open(&r.path)?;
    let existed = branch_exists(&repo, &s.id.branch());
    let base = if existed {
        let name = match pick.filter(|p| !p.trim().is_empty()) {
            Some(p) => short_branch(p.trim()),
            None => match repo_default_base(r) {
                Some(b) => b,
                None => detect_base(&repo, None)?.name,
            },
        };
        BasePlan {
            name,
            start: None,
            rule_resolved: None,
            fixed_upgrade: None,
        }
    } else {
        plan_base(&repo, r, refresh, pick)?
    };
    let name = free_name(s, &r.name);
    let path = s.root.join(&name);
    let branch = s.id.branch();
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
        base: base.name.clone(),
    });
    s.bases.insert(r.id.clone(), base.name.clone());
    Ok(base)
}

/// Add a worktree of `r` to `s` (a separate child session) on its branch `meninx/<id>` started at
/// the parent's branch `parent_branch`, which becomes this repo's base (a local branch).
pub fn add_child_worktree(s: &mut Session, r: &RegisteredRepo, parent_branch: &str) -> Result<()> {
    if s.worktree(&r.id).is_some() {
        return Ok(());
    }
    let repo = Repo::open(&r.path)?;
    let branch = s.id.branch();
    let existed = branch_exists(&repo, &branch);
    let name = free_name(s, &r.name);
    let path = s.root.join(&name);
    let mut args: Vec<std::ffi::OsString> = vec!["worktree".into(), "add".into()];
    if existed {
        args.push(path.clone().into_os_string());
        args.push(branch.clone().into());
    } else {
        args.extend(["--no-track".into(), "-b".into(), branch.clone().into()]);
        args.push(path.clone().into_os_string());
        args.push(parent_branch.into());
    }
    if let Err(e) = repo.git(&args) {
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
        base: parent_branch.to_owned(),
    });
    s.bases.insert(r.id.clone(), parent_branch.to_owned());
    Ok(())
}

/// Attach a registered repository to a workspace session (Desk `addRepoToSessions`: no
/// refresh, `base` = the base picked in the session settings, else the repo's). Already
/// attached: no-op.
pub fn attach_repo(s: &mut Session, r: &RegisteredRepo, base: Option<&str>) -> Result<BasePlan> {
    if !s.is_workspace() {
        return Err(Error::NotAWorkspaceSession(s.id.to_string()));
    }
    add_worktree(s, r, false, base)
}

/// Desk `setSessionBase`: the session's base for one repository (`None` = back to the
/// repository's default). Must be one of origin's branches as of the last fetch. Only what the
/// session is measured against changes; its branch is not rebased.
pub fn set_session_base(s: &mut Session, r: &RegisteredRepo, base: Option<&str>) -> Result<()> {
    if s.worktree(&r.id).is_none() {
        return Err(Error::RepoNotAttached {
            name: r.name.clone(),
        });
    }
    match base.map(str::trim).filter(|b| !b.is_empty()) {
        None => {
            s.bases.remove(&r.id);
        }
        Some(b) => {
            let name = short_branch(b);
            let repo = Repo::open(&r.path)?;
            repo.git([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/origin/{name}^{{commit}}"),
            ])
            .map_err(|_| Error::OriginBranchUnknown(name.clone()))?;
            s.bases.insert(r.id.clone(), name);
        }
    }
    Ok(())
}

/// Desk's `Workspace` constructor: sessions made before per-session bases get today's base
/// pinned, so a later change of the repo default does not move them. Returns whether any
/// session changed.
pub fn pin_session_bases(sessions: &mut crate::session::SessionStore, repos: &RepoStore) -> bool {
    let mut changed = false;
    for s in sessions.all_mut() {
        if !s.is_workspace() {
            continue;
        }
        let missing: Vec<SessionWorktree> = s
            .worktrees
            .iter()
            .filter(|w| !s.bases.contains_key(&w.repo))
            .cloned()
            .collect();
        for w in missing {
            let mut b = s.base_of(repos.get(&w.repo), &w);
            if b.is_empty() {
                b = "main".into();
            }
            s.bases.insert(w.repo.clone(), b);
            changed = true;
        }
    }
    changed
}

/// Detach a repository from a workspace session: its worktree is removed (`--force`:
/// uncommitted changes are discarded), its branch `meninx/<id>` is kept (no Desk equivalent:
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
    s.bases.remove(repo);
    Ok(())
}

/// Desk's `isManagedPath`: folders this app may delete outright (inside `workspaces/` or the
/// legacy `worktrees/`).
pub fn is_managed_path(paths: &Paths, p: &Path) -> bool {
    is_inside(p, &paths.workspaces_dir()) || is_inside(p, &paths.worktrees_dir())
}

/// Desk's `removeWorktree`: `git worktree remove --force` (folder delete inside managed dirs
/// when git cannot), `worktree prune`, optionally `branch -D` of an `meninx/` branch.
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
/// with `remove_worktrees` its `meninx/` branch deleted), then the session folder (only inside
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
            let b = s.base_of(repos.get(&w.repo), w);
            let base = if b.is_empty() {
                String::new()
            } else {
                format!(" (base `{b}`)")
            };
            format!("- `{folder}` — {repo}, branch `{}`{base}", w.branch)
        })
        .collect();
    let mut rows = rows;
    rows.extend(s.pending.iter().map(|p| {
        let repo = repos
            .get(&p.repo)
            .map_or_else(|| "repository".to_owned(), |r| r.name.clone());
        format!(
            "- `{}/` — {repo}, branch `{}` (**being created** — see \"Worktrees being created\" below)",
            p.name,
            s.id.branch()
        )
    }));
    let list = if rows.is_empty() {
        "- (no repositories are currently connected to this workspace)".to_owned()
    } else {
        rows.join("\n")
    };
    let pending = if s.pending.is_empty() {
        String::new()
    } else {
        format!(
            "\n## Worktrees being created\n\n\
             Meninx is still creating the worktrees marked *being created* in the background \
             (it started this session first so you could begin right away). While the file \
             `{root}/{marker}` exists, that is still going on and those folders may be missing or \
             incomplete.\n\n\
             - A request that does not need those repositories (questions, planning, other \
             folders): answer it right away.\n\
             - A request that needs one of them (reading, searching, changing or running anything \
             in it): first tell the user in one short line that you are waiting for the worktree \
             to be created, then wait until `{marker}` is gone — check every few seconds with a \
             shell loop, e.g. bash `while [ -e '{root}/{marker}' ]; do sleep 3; done` or \
             PowerShell `while (Test-Path '{root}/{marker}') {{ Start-Sleep 3 }}` (give up after \
             about 10 minutes and tell the user) — and then do the request.\n\
             - After the file is gone, a listed folder that still does not exist could not be \
             created: tell the user (they can attach the repository again from the app).\n",
            marker = PENDING_MARKER,
        )
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
         (repository picker of the session's detail area); it then appears in this list.\n\
         {pending}",
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
        let r = match repos.register(&old_repo, false) {
            Ok(r) => r,
            Err(Error::NotAGitRepo { .. } | Error::FolderNotFound(_) | Error::NotAGitFolder(_)) => {
                repos.adopt_missing(&old_repo)?
            }
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
                repos.register(root, false)?;
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
            refresh_base: false,
            background: false,
            bases: Default::default(),
        }
    }

    #[test]
    fn background_create_leaves_worktrees_pending() {
        let t = TempDir::new("ws-bg");
        let paths = Paths::new(t.0.join("data"));
        let a = t.0.join("one").join("app");
        let b = t.0.join("two").join("app");
        for d in [&a, &b] {
            test_repo::init(d);
        }
        let mut repos = RepoStore::open(&paths).unwrap();
        let ra = repos.register(&a, false).unwrap().clone();
        let rb = repos.register(&b, false).unwrap().clone();
        let (mut s, failures, plans) = create_session(
            &paths,
            &repos,
            &NewSession { background: true, ..req(vec![ra.id.clone(), rb.id.clone()]) },
            |_| Ok(()),
        )
        .unwrap();
        assert!(failures.is_empty() && plans.is_empty() && s.worktrees.is_empty());
        // Planned names already avoid each other; nothing is created yet but the marker.
        let names: Vec<&str> = s.pending.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["app", "app-2"]);
        assert!(s.preparing() && s.root.is_dir() && !s.root.join("app").exists());
        let marker = s.root.join(PENDING_MARKER);
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "app/\napp-2/\n");
        let g = session_guidance(&s, &repos).unwrap();
        assert!(g.contains("`app-2/` — app, branch") && g.contains("being created"), "{g}");
        assert!(g.contains(PENDING_MARKER), "{g}");
        // The caller creates them one by one (as the server's job does).
        for p in s.pending.clone() {
            s.pending.retain(|x| x.repo != p.repo);
            let r = repos.get(&p.repo).unwrap().clone();
            add_worktree(&mut s, &r, p.refresh, p.pick.as_deref()).unwrap();
            assert_eq!(s.worktree(&p.repo).unwrap().name, p.name);
        }
        write_pending_marker(&s).unwrap();
        assert!(!marker.exists());
        let g = session_guidance(&s, &repos).unwrap();
        assert!(!g.contains("being created") && !g.contains(PENDING_MARKER), "{g}");
        let mut store = SessionStore::open(&paths).unwrap();
        store.upsert(s.clone()).unwrap();
        assert_eq!(store.count_using_repo(&ra.id), 1);
        remove_session_files(&paths, &s, &repos, true).unwrap();
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
        let ra = repos.register(&a, false).unwrap();
        let rb = repos.register(&b, false).unwrap();
        let rc = repos.register(&c, false).unwrap();

        // Two repos with the same folder name: app, app-2.
        let (mut s, failures, _) =
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
        attach_repo(&mut s, &rc, None).unwrap();
        attach_repo(&mut s, &rc, None).unwrap();
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
        attach_repo(&mut s, &rc, None).unwrap();
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
        let ra = repos.register(&a, false).unwrap();

        // Zero repos: an empty workspace (Desk allows it).
        let (s, f, _) = create_session(&paths, &repos, &req(vec![]), |_| Ok(())).unwrap();
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
        let rg = repos.register(&gone, false).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();
        let (s2, f, _) =
            create_session(&paths, &repos, &req(vec![ra.id.clone(), rg.id.clone()]), |_| Ok(()))
                .unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].repo, rg.id);
        assert_eq!(s2.worktrees.len(), 1);
        remove_session_files(&paths, &s2, &repos, true).unwrap();

        // Original-repo mode: runs in the repo, nothing created; deleting keeps the repo.
        let (o, _, _) = create_session(
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
            attach_repo(&mut o2, &ra, None),
            Err(Error::NotAWorkspaceSession(_))
        ));
        remove_session_files(&paths, &o, &repos, true).unwrap();
        assert!(a.join("README.md").is_file());
        assert_eq!(Repo::open(&a).unwrap().list_worktrees().unwrap().len(), 1);
    }

    #[test]
    fn bases_refresh_rules_and_offline() {
        let t = TempDir::new("ws-base");
        let paths = Paths::new(t.0.join("data"));
        let seed = t.0.join("seed");
        test_repo::init(&seed);
        test_repo::run(&t.0, &["clone", "-q", "--bare", &seed.display().to_string(), "origin.git"]);
        let origin = t.0.join("origin.git").display().to_string();
        test_repo::run(&t.0, &["clone", "-q", &origin, "app"]);
        test_repo::run(&t.0, &["clone", "-q", &origin, "pusher"]);
        let a = t.0.join("app");
        let pusher = t.0.join("pusher");
        let rev = |d: &Path, r: &str| test_repo::run(d, &["rev-parse", r]);
        let old = rev(&a, "main");
        // origin moves on (main + a new release branch) after the clone.
        test_repo::run(&pusher, &["commit", "-q", "--allow-empty", "-m", "remote"]);
        test_repo::run(&pusher, &["branch", "release/2026.2.0"]);
        test_repo::run(&pusher, &["branch", "release/2026.10.0"]);
        test_repo::run(&pusher, &["push", "-q", "origin", "main", "release/2026.2.0", "release/2026.10.0"]);
        let new = rev(&pusher, "main");
        let mut repos = RepoStore::open(&paths).unwrap();
        let ra = repos.register(&a, false).unwrap();
        let mk = |refresh: bool, pick: Option<&str>| NewSession {
            refresh_base: refresh,
            bases: pick
                .map(|p| [(ra.id.clone(), p.to_owned())].into_iter().collect())
                .unwrap_or_default(),
            ..req(vec![ra.id.clone()])
        };
        let head = |s: &Session| rev(&s.worktrees[0].path, "HEAD");

        // Without 최신화: the last-fetched state (old). With it: origin's latest; local main unmoved.
        let (s1, f, _) = create_session(&paths, &repos, &mk(false, None), |_| Ok(())).unwrap();
        assert!(f.is_empty());
        assert_eq!(head(&s1), old);
        let (s2, f, _) = create_session(&paths, &repos, &mk(true, None), |_| Ok(())).unwrap();
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(head(&s2), new);
        assert_eq!(rev(&a, "main"), old);
        assert_eq!(s2.bases.get(&ra.id).map(String::as_str), Some("main"));

        // A picked base: unknown to the last fetch without 최신화; fetched with it.
        let (s3, f, _) =
            create_session(&paths, &repos, &mk(false, Some("release/2026.10.0")), |_| Ok(())).unwrap();
        assert!(matches!(f[0].error, Error::OriginBranchMissing(_)), "{f:?}");
        assert!(s3.worktrees.is_empty());
        let (s4, f, _) =
            create_session(&paths, &repos, &mk(true, Some("release/2026.10.0")), |_| Ok(())).unwrap();
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(s4.worktrees[0].base, "release/2026.10.0");

        // Rule: highest version, re-resolved and fetched at every creation.
        let rule = crate::base::BaseBranchRule {
            remote: "origin".into(),
            pattern: "release/*".into(),
            matches: None,
        };
        let r = repos.set_base_rule(&ra.id, Some(&rule)).unwrap();
        assert_eq!(r.rule_resolved.as_deref(), Some("refs/remotes/origin/release/2026.10.0"));
        let (s5, f, plans) = create_session(&paths, &repos, &mk(false, None), |_| Ok(())).unwrap();
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(s5.worktrees[0].base, "release/2026.10.0");
        assert_eq!(plans[0].1.rule_resolved.as_deref(), Some("refs/remotes/origin/release/2026.10.0"));
        // Session base override and resolution order.
        let mut s5 = s5;
        let r = repos.get(&ra.id).unwrap().clone();
        assert!(matches!(
            set_session_base(&mut s5, &r, Some("nope")),
            Err(Error::OriginBranchUnknown(_))
        ));
        set_session_base(&mut s5, &r, Some("origin/main")).unwrap();
        assert_eq!(s5.base_of(Some(&r), &s5.worktrees[0]), "main");
        set_session_base(&mut s5, &r, None).unwrap();
        assert_eq!(s5.base_of(Some(&r), &s5.worktrees[0]), "release/2026.10.0");
        assert!(session_guidance(&s5, &repos).unwrap().contains("(base `release/2026.10.0`)"));
        repos.set_base_rule(&ra.id, None).unwrap();

        // Offline origin: 최신화 fails that repo (no fallback, as Desk); without it, local state.
        test_repo::run(&a, &["remote", "set-url", "origin", &t.0.join("gone.git").display().to_string()]);
        let (s6, f, _) = create_session(&paths, &repos, &mk(true, None), |_| Ok(())).unwrap();
        assert!(matches!(f[0].error, Error::BaseRefreshFailed(_)), "{f:?}");
        assert!(s6.worktrees.is_empty());
        let (s7, f, _) = create_session(&paths, &repos, &mk(false, None), |_| Ok(())).unwrap();
        assert!(f.is_empty());
        assert_eq!(head(&s7), new); // origin/main as fetched by the earlier refresh
        // A repo without any remote: 최신화 is refused with base-no-remote.
        let lone = t.0.join("lone");
        test_repo::init(&lone);
        let rl = repos.register(&lone, false).unwrap();
        let (_, f, _) = create_session(
            &paths,
            &repos,
            &NewSession { refresh_base: true, ..req(vec![rl.id.clone()]) },
            |_| Ok(()),
        )
        .unwrap();
        assert!(matches!(f[0].error, Error::BaseNoRemote), "{f:?}");
        for s in [&s1, &s2, &s3, &s4, &s5, &s6, &s7] {
            remove_session_files(&paths, s, &repos, true).unwrap();
        }
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
