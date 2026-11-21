//! Login sessions and the per-session server dir. Workspace sessions (create, attach, detach,
//! delete) live in [`crate::workspace`].

use crate::error::{Error, IoAction, Result};
use crate::paths::Paths;
use crate::session::{Session, SessionId, SessionKind, SessionMode, SessionTarget};

/// A login session for an account: no repository; its root (the process cwd) is an empty dir
/// `sessions/<id>/cwd`. Not persisted.
pub fn create_login_session(
    paths: &Paths,
    provider: &str,
    account: &str,
    title: &str,
) -> Result<Session> {
    let id = SessionId::new();
    let cwd = paths.session_dir(id.as_str()).join("cwd");
    crate::paths::ensure_dir(&cwd)?;
    let mut s = Session::new(
        id,
        title,
        SessionTarget {
            provider: provider.to_owned(),
            account: Some(account.to_owned()),
            configuration: String::new(),
        },
        SessionMode::Workspace,
        cwd,
    );
    s.kind = SessionKind::Login;
    Ok(s)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;
    use crate::registry::RepoStore;
    use crate::repo::test_repo;
    use crate::session::SessionStore;

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
        assert!(s.root.is_dir());
        assert!(s.root.starts_with(paths.session_dir(s.id.as_str())));
        assert!(!s.is_workspace());
        store.upsert(s.clone()).unwrap();
        let repos = RepoStore::open(&paths).unwrap();
        crate::workspace::remove_session_files(&paths, &s, &repos, true).unwrap();
        store.remove(&s.id).unwrap();
        assert!(!paths.session_dir(s.id.as_str()).exists());
        assert!(repo_dir.join("README.md").is_file());
    }
}
