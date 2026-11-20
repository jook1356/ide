//! Multi-step session operations used by the app: creating a session (repo -> worktree ->
//! launch spec) and deleting one (worktree + branch + store entry).

use std::path::{Path, PathBuf};

use crate::error::{Error, IoAction, Result};
use crate::paths::Paths;
use crate::repo::{Repo, normalize_path};
use crate::session::{Session, SessionId, SessionKind, SessionStore, SessionTarget};
use crate::worktree::{BRANCH_PREFIX, Worktree};

/// Input of [`create_session`] (what the new-session form collects).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSession {
    /// Any folder inside the repository.
    pub repo_path: PathBuf,
    /// Provider, account and configuration.
    pub target: SessionTarget,
    /// Display title; empty = `"<provider_name> · <repo folder>"`.
    pub title: String,
    /// Provider display name for the default title.
    pub provider_name: String,
    /// Revision to branch from; empty = `HEAD` of the main checkout.
    pub base_ref: String,
}

/// Create a session without persisting it: open the repo, run `precheck` on the planned
/// session (e.g. resolve the agent program, before touching git, so a missing program leaves
/// nothing behind), then create the worktree on `ide/<id>`. The caller stores the session
/// ([`SessionStore::upsert`]) and builds/spawns its launch spec.
pub fn create_session(
    paths: &Paths,
    req: &NewSession,
    precheck: impl FnOnce(&Session) -> Result<()>,
) -> Result<Session> {
    let repo = Repo::open(&req.repo_path)?;
    let base = req.base_ref.trim();
    let base = (!base.is_empty()).then_some(base);
    if let Some(b) = base {
        // Validate early for a clear error (create_worktree would fail the same way).
        repo.resolve_commit(b)?;
    }
    let id = SessionId::new();
    let title = match req.title.trim() {
        "" => default_title(&repo, &req.provider_name),
        t => t.to_owned(),
    };
    let planned = Worktree {
        path: repo.worktree_path(paths, &id),
        branch: id.branch(),
        base_commit: String::new(),
    };
    precheck(&Session::new(
        id.clone(),
        &title,
        req.target.clone(),
        &repo,
        planned,
    ))?;
    let wt = repo.create_worktree(paths, &id, base)?;
    Ok(Session::new(id, title, req.target.clone(), &repo, wt))
}

/// A login session for an account: no repository; its "worktree" (the process cwd) is an
/// empty dir `sessions/<id>/cwd` and its branch is empty. Not persisted.
pub fn create_login_session(
    paths: &Paths,
    provider: &str,
    account: &str,
    title: &str,
) -> Result<Session> {
    let id = SessionId::new();
    let cwd = paths.session_dir(id.as_str()).join("cwd");
    crate::paths::ensure_dir(&cwd)?;
    let now = crate::paths::now_unix();
    Ok(Session {
        id,
        title: title.to_owned(),
        kind: SessionKind::Login,
        provider: provider.to_owned(),
        account: Some(account.to_owned()),
        configuration: String::new(),
        repo_path: cwd.clone(),
        worktree_path: cwd,
        branch: String::new(),
        created_at: now,
        last_active: now,
        legacy_profile: None,
        legacy_agent: None,
    })
}

fn default_title(repo: &Repo, provider_name: &str) -> String {
    let name = repo
        .root()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    format!("{provider_name} · {name}")
}

/// Delete a session. With `remove_worktree`, its worktree is removed (`--force`, local changes
/// are discarded) and its `ide/` branch deleted (`-D`) before the store entry is removed; if
/// that fails the session stays in the store so the user can retry. If the repository itself
/// is gone, a worktree directory inside the app's `worktrees/` dir is deleted directly.
/// The caller must stop the session's process first.
pub fn delete_session(
    paths: &Paths,
    store: &mut SessionStore,
    session: &SessionId,
    remove_worktree: bool,
) -> Result<Session> {
    let s = store
        .get(session)
        .cloned()
        .ok_or_else(|| Error::SessionNotFound(session.to_string()))?;
    if remove_worktree {
        remove_session_files(paths, &s)?;
    } else {
        remove_session_dir(paths, &s)?;
    }
    store.remove(session)
}

/// The disk part of [`delete_session`] with `remove_worktree`: force-remove the session's
/// worktree and delete its `ide/` branch, without touching the store. Apps run this off the UI
/// thread (git can be slow) and then call [`SessionStore::remove`].
pub fn remove_session_files(paths: &Paths, s: &Session) -> Result<()> {
    remove_session_dir(paths, s)?;
    if s.kind == SessionKind::Login {
        return Ok(());
    }
    match Repo::open(&s.repo_path) {
        Ok(repo) => {
            repo.remove_worktree(&s.worktree_path, true)?;
            if s.branch.starts_with(BRANCH_PREFIX) && branch_exists(&repo, &s.branch) {
                repo.delete_branch(&s.branch, true)?;
            }
            Ok(())
        }
        Err(Error::NotAGitRepo { .. }) => {
            // Repo deleted or moved: only clean up our own directory.
            if is_inside(&s.worktree_path, &paths.worktrees_dir()) && s.worktree_path.exists() {
                std::fs::remove_dir_all(&s.worktree_path)
                    .map_err(|e| Error::io(IoAction::Removing, &s.worktree_path, e))?;
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Remove `sessions/<id>/` (generated agent files, login cwd).
pub fn remove_session_dir(paths: &Paths, s: &Session) -> Result<()> {
    let dir = paths.session_dir(s.id.as_str());
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(IoAction::Removing, dir, e)),
    }
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

fn is_inside(path: &Path, dir: &Path) -> bool {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let p = normalize_path(&abs(path));
    let d = normalize_path(&abs(dir));
    p.len() > d.len() && p.starts_with(&d) && p.as_bytes()[d.len()] == b'/'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;
    use crate::repo::{same_path, test_repo};
    use crate::worktree::worktree_exists;

    fn req(repo: &Path) -> NewSession {
        NewSession {
            repo_path: repo.to_path_buf(),
            target: SessionTarget {
                provider: "shell".into(),
                account: None,
                configuration: "default".into(),
            },
            title: String::new(),
            provider_name: "Shell".into(),
            base_ref: String::new(),
        }
    }

    #[test]
    fn create_and_delete() {
        let t = TempDir::new("ops");
        let repo_dir = t.0.join("my repo");
        test_repo::init(&repo_dir);
        let paths = Paths::new(t.0.join("data"));
        let mut store = SessionStore::open(&paths).unwrap();
        let ok = |_: &Session| Ok(());

        let req = NewSession {
            repo_path: repo_dir.join("."),
            ..req(&repo_dir)
        };
        let a = create_session(&paths, &req, ok).unwrap();
        assert_eq!(a.title, "Shell · my repo");
        assert_eq!(a.provider, "shell");
        assert_eq!(a.configuration, "default");
        assert!(worktree_exists(&a.worktree_path));
        assert!(same_path(&a.repo_path, &repo_dir));
        store.upsert(a.clone()).unwrap();

        let b = create_session(
            &paths,
            &NewSession {
                title: " second ".into(),
                base_ref: "HEAD".into(),
                ..req.clone()
            },
            ok,
        )
        .unwrap();
        assert_eq!(b.title, "second");
        store.upsert(b.clone()).unwrap();

        // Errors leave nothing behind; the precheck sees the planned session.
        let before = Repo::open(&repo_dir)
            .unwrap()
            .list_worktrees()
            .unwrap()
            .len();
        let err = create_session(&paths, &req, |s: &Session| {
            assert!(!s.worktree_path.exists());
            Err(Error::ProgramNotFound {
                program: "codex".into(),
            })
        })
        .unwrap_err();
        assert!(matches!(err, Error::ProgramNotFound { .. }), "{err}");
        let err = create_session(
            &paths,
            &NewSession {
                base_ref: "no-such-ref".into(),
                ..req.clone()
            },
            ok,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Git { .. }), "{err}");
        let err = create_session(
            &paths,
            &NewSession {
                repo_path: t.0.join("data"),
                ..req.clone()
            },
            ok,
        )
        .unwrap_err();
        assert!(matches!(err, Error::NotAGitRepo { .. }), "{err}");
        let repo = Repo::open(&repo_dir).unwrap();
        assert_eq!(repo.list_worktrees().unwrap().len(), before);

        // Delete keeping the worktree (generated agent files go anyway).
        let agent_dir = paths.session_agent_dir(b.id.as_str());
        std::fs::create_dir_all(&agent_dir).unwrap();
        delete_session(&paths, &mut store, &b.id, false).unwrap();
        assert!(worktree_exists(&b.worktree_path));
        assert!(branch_exists(&repo, &b.branch));
        assert!(!agent_dir.exists());

        // Delete with worktree + branch removal (dirty worktree: forced).
        std::fs::write(a.worktree_path.join("dirty.txt"), "x").unwrap();
        delete_session(&paths, &mut store, &a.id, true).unwrap();
        assert!(!a.worktree_path.exists());
        assert!(!branch_exists(&repo, &a.branch));
        assert!(store.list().is_empty());
        assert!(SessionStore::open(&paths).unwrap().list().is_empty());
        assert!(matches!(
            delete_session(&paths, &mut store, &a.id, true),
            Err(Error::SessionNotFound(_))
        ));
        assert!(repo_dir.join("README.md").is_file());
        assert_eq!(repo.current_branch().unwrap().as_deref(), Some("main"));
    }

    #[test]
    fn login_session_has_no_repo() {
        let t = TempDir::new("ops-login");
        // Data dir inside a git repo: deleting a login session must not touch git.
        let repo_dir = t.0.join("r");
        test_repo::init(&repo_dir);
        let paths = Paths::new(repo_dir.join("data"));
        let mut store = SessionStore::open(&paths).unwrap();
        let s = create_login_session(&paths, "claude", "work", "login").unwrap();
        assert_eq!(s.kind, SessionKind::Login);
        assert!(s.worktree_path.is_dir());
        assert!(
            s.worktree_path
                .starts_with(paths.session_dir(s.id.as_str()))
        );
        store.upsert(s.clone()).unwrap();
        delete_session(&paths, &mut store, &s.id, true).unwrap();
        assert!(!paths.session_dir(s.id.as_str()).exists());
        assert!(repo_dir.join("README.md").is_file());
    }

    #[test]
    fn delete_when_repo_is_gone() {
        let t = TempDir::new("ops-gone");
        let repo_dir = t.0.join("r");
        test_repo::init(&repo_dir);
        let paths = Paths::new(t.0.join("data"));
        let mut store = SessionStore::open(&paths).unwrap();
        let s = create_session(
            &paths,
            &NewSession {
                title: "x".into(),
                ..req(&repo_dir)
            },
            |_| Ok(()),
        )
        .unwrap();
        store.upsert(s.clone()).unwrap();
        std::fs::remove_dir_all(&repo_dir).unwrap();
        delete_session(&paths, &mut store, &s.id, true).unwrap();
        assert!(!s.worktree_path.exists());
        assert!(store.list().is_empty());
    }

    #[test]
    fn inside_check() {
        assert!(is_inside(
            Path::new("C:/d/worktrees/r/1"),
            Path::new("C:/d/worktrees")
        ));
        assert!(!is_inside(
            Path::new("C:/d/worktrees"),
            Path::new("C:/d/worktrees")
        ));
        assert!(!is_inside(
            Path::new("C:/d/worktrees2/x"),
            Path::new("C:/d/worktrees")
        ));
    }
}
