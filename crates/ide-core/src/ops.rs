//! Multi-step session operations used by the app: creating a session (repo -> worktree ->
//! launch spec) and deleting one (worktree + branch + store entry).

use std::path::{Path, PathBuf};

use crate::error::{Error, IoAction, Result};
use crate::launch::{LaunchEnv, LaunchSpec};
use crate::paths::Paths;
use crate::profile::Profile;
use crate::repo::{Repo, normalize_path};
use crate::session::{AgentKind, Session, SessionId, SessionStore};
use crate::worktree::{BRANCH_PREFIX, Worktree};

/// Input of [`create_session`] (what the new-session form collects).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSession {
    /// Any folder inside the repository.
    pub repo_path: PathBuf,
    /// Agent to run.
    pub agent: AgentKind,
    /// Display title; empty = derived from the repo folder and agent.
    pub title: String,
    /// Revision to branch from; empty = `HEAD` of the main checkout.
    pub base_ref: String,
}

/// Create a session without persisting it: open the repo, check the agent program can be
/// resolved (before touching git, so a missing program leaves nothing behind), create the
/// worktree on `ide/<id>` and build the launch spec. The caller stores the session
/// ([`SessionStore::upsert`]) and spawns the spec.
pub fn create_session(
    paths: &Paths,
    profile: &Profile,
    req: &NewSession,
) -> Result<(Session, LaunchSpec)> {
    create_session_with(paths, profile, req, &LaunchEnv::from_process())
}

/// Like [`create_session`] with an explicit launch environment (tests).
pub fn create_session_with(
    paths: &Paths,
    profile: &Profile,
    req: &NewSession,
    env: &LaunchEnv,
) -> Result<(Session, LaunchSpec)> {
    let repo = Repo::open(&req.repo_path)?;
    let base = req.base_ref.trim();
    let base = (!base.is_empty()).then_some(base);
    if let Some(b) = base {
        // Validate early for a clear error (create_worktree would fail the same way).
        repo.resolve_commit(b)?;
    }
    let id = SessionId::new();
    let title = match req.title.trim() {
        "" => default_title(&repo, req.agent),
        t => t.to_owned(),
    };
    // Dry run of the launch spec against the future worktree path.
    let planned = Worktree {
        path: repo.worktree_path(paths, &id),
        branch: id.branch(),
        base_commit: String::new(),
    };
    let probe = Session::new(id.clone(), &title, &profile.name, &repo, planned, req.agent);
    LaunchSpec::for_session_with(&probe, profile, env)?;

    let wt = repo.create_worktree(paths, &id, base)?;
    let session = Session::new(id, title, &profile.name, &repo, wt, req.agent);
    let spec = LaunchSpec::for_session_with(&session, profile, env)?;
    Ok((session, spec))
}

fn default_title(repo: &Repo, agent: AgentKind) -> String {
    let name = repo
        .root()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    format!("{} · {name}", agent.label())
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
    }
    store.remove(session)
}

/// The disk part of [`delete_session`] with `remove_worktree`: force-remove the session's
/// worktree and delete its `ide/` branch, without touching the store. Apps run this off the UI
/// thread (git can be slow) and then call [`SessionStore::remove`].
pub fn remove_session_files(paths: &Paths, s: &Session) -> Result<()> {
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
    use crate::profile::ProfileStore;
    use crate::repo::{same_path, test_repo};
    use crate::worktree::worktree_exists;

    fn shell_env(t: &TempDir) -> LaunchEnv {
        let bin = t.0.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let shell = bin.join(if cfg!(windows) { "cmd.exe" } else { "sh" });
        std::fs::write(&shell, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        LaunchEnv {
            path: Some(bin.into_os_string()),
            pathext: Some(".EXE".into()),
            comspec: Some(shell.clone().into()),
            shell: Some(shell.into()),
        }
    }

    #[test]
    fn create_and_delete() {
        let t = TempDir::new("ops");
        let repo_dir = t.0.join("my repo");
        test_repo::init(&repo_dir);
        let paths = Paths::new(t.0.join("data"));
        let profiles = ProfileStore::open(&paths).unwrap();
        let profile = profiles.get("default").unwrap();
        let mut store = SessionStore::open(&paths).unwrap();
        let env = shell_env(&t);

        let req = NewSession {
            repo_path: repo_dir.join("."),
            agent: AgentKind::Shell,
            title: String::new(),
            base_ref: String::new(),
        };
        let (a, spec) = create_session_with(&paths, profile, &req, &env).unwrap();
        assert_eq!(a.title, "Shell · my repo");
        assert!(worktree_exists(&a.worktree_path));
        assert_eq!(spec.cwd, a.worktree_path);
        assert!(same_path(&a.repo_path, &repo_dir));
        store.upsert(a.clone()).unwrap();

        let (b, _) = create_session_with(
            &paths,
            profile,
            &NewSession {
                title: " second ".into(),
                base_ref: "HEAD".into(),
                ..req.clone()
            },
            &env,
        )
        .unwrap();
        assert_eq!(b.title, "second");
        store.upsert(b.clone()).unwrap();

        // Errors leave nothing behind.
        let before = Repo::open(&repo_dir)
            .unwrap()
            .list_worktrees()
            .unwrap()
            .len();
        let err = create_session_with(
            &paths,
            profile,
            &NewSession {
                agent: AgentKind::Codex,
                ..req.clone()
            },
            &env,
        )
        .unwrap_err();
        assert!(matches!(err, Error::ProgramNotFound { .. }), "{err}");
        let err = create_session_with(
            &paths,
            profile,
            &NewSession {
                base_ref: "no-such-ref".into(),
                ..req.clone()
            },
            &env,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Git { .. }), "{err}");
        let err = create_session_with(
            &paths,
            profile,
            &NewSession {
                repo_path: t.0.join("data"),
                ..req.clone()
            },
            &env,
        )
        .unwrap_err();
        assert!(matches!(err, Error::NotAGitRepo { .. }), "{err}");
        let repo = Repo::open(&repo_dir).unwrap();
        assert_eq!(repo.list_worktrees().unwrap().len(), before);

        // Delete keeping the worktree.
        delete_session(&paths, &mut store, &b.id, false).unwrap();
        assert!(worktree_exists(&b.worktree_path));
        assert!(branch_exists(&repo, &b.branch));

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
    fn delete_when_repo_is_gone() {
        let t = TempDir::new("ops-gone");
        let repo_dir = t.0.join("r");
        test_repo::init(&repo_dir);
        let paths = Paths::new(t.0.join("data"));
        let profiles = ProfileStore::open(&paths).unwrap();
        let mut store = SessionStore::open(&paths).unwrap();
        let req = NewSession {
            repo_path: repo_dir.clone(),
            agent: AgentKind::Shell,
            title: "x".into(),
            base_ref: String::new(),
        };
        let (s, _) = create_session_with(
            &paths,
            profiles.get("default").unwrap(),
            &req,
            &shell_env(&t),
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
