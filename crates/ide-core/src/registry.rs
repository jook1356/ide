//! Registered repositories (Hermes Desk's `Workspace.repos`, `addRepo` / `removeRepo`): the
//! repositories sessions get worktrees of. Persisted in `repos.json`.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::paths::{Paths, now_unix, read_json, write_json};
use crate::repo::{Repo, same_path};

/// Id of a registered repository: 8 lowercase hex digits (as Desk's `newId()`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoId(String);

impl RepoId {
    /// A new random id.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_u32(std::process::id());
        RepoId(format!("{:08x}", h.finish() as u32))
    }

    /// Parse an id (8 hex digits).
    pub fn parse(s: &str) -> Result<Self> {
        if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            Ok(RepoId(s.to_owned()))
        } else {
            Err(Error::RepoNotFound(s.to_owned()))
        }
    }

    /// The id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for RepoId {
    type Error = Error;
    fn try_from(s: String) -> Result<Self> {
        Self::parse(&s)
    }
}

impl From<RepoId> for String {
    fn from(id: RepoId) -> String {
        id.0
    }
}

/// A registered repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredRepo {
    /// Id.
    pub id: RepoId,
    /// Display name: the main checkout's folder name.
    pub name: String,
    /// Main checkout (top level).
    pub path: PathBuf,
    /// Fixed base for new worktrees (`refs/heads/x` / `refs/remotes/o/x` / short name);
    /// `None` = automatic ([`detect_base`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    /// Registration time, Unix seconds.
    #[serde(default)]
    pub created_at: u64,
}

/// The branch new worktrees start from: its short name (shown, recorded as the worktree's
/// base) and the revision to branch from (`None` = an unborn branch: no commit yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseBranch {
    /// Branch name (`main`).
    pub name: String,
    /// Start revision (`main`, `origin/main`, `refs/heads/x`); `None` for an unborn branch.
    pub start: Option<String>,
}

fn has_commit(repo: &Repo, rev: &str) -> bool {
    repo.git([
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &format!("{rev}^{{commit}}"),
    ])
    .is_ok()
}

/// Desk's `mainBranch()`: `main`, else `master`, else origin's default branch; start from
/// `origin/<name>` when the local branch is missing or only behind it. Else the current branch
/// (unborn: no start). A configured `base_branch` (Desk's `configuredBase`) wins when it exists.
pub fn detect_base(repo: &Repo, configured: Option<&str>) -> Result<BaseBranch> {
    if let Some(b) = configured.map(str::trim).filter(|b| !b.is_empty() && !b.starts_with('-')) {
        let refs: Vec<String> = if b.starts_with("refs/") {
            vec![b.to_owned()]
        } else {
            vec![format!("refs/heads/{b}"), format!("refs/remotes/{b}")]
        };
        for r in refs {
            if (r.starts_with("refs/heads/") || r.starts_with("refs/remotes/"))
                && repo.git(["show-ref", "--verify", "--quiet", &r]).is_ok()
            {
                let name = match r.strip_prefix("refs/heads/") {
                    Some(n) => n.to_owned(),
                    None => {
                        let rest = r.trim_start_matches("refs/remotes/");
                        rest.split_once('/').map_or(rest, |(_, n)| n).to_owned()
                    }
                };
                return Ok(BaseBranch {
                    name,
                    start: Some(r),
                });
            }
        }
        return Err(Error::Git {
            args: format!("show-ref {b}"),
            status: "missing".into(),
            stderr: format!("base branch {b} does not exist locally"),
        });
    }
    let origin_default = repo
        .git(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .unwrap_or_default();
    let origin_default = origin_default
        .trim()
        .trim_start_matches("origin/")
        .to_owned();
    for name in ["main", "master", origin_default.as_str()] {
        if name.is_empty() {
            continue;
        }
        let local = has_commit(repo, &format!("refs/heads/{name}"));
        let remote = has_commit(repo, &format!("refs/remotes/origin/{name}"));
        match (local, remote) {
            (false, false) => continue,
            (true, false) => {
                return Ok(BaseBranch {
                    name: name.to_owned(),
                    start: Some(name.to_owned()),
                });
            }
            (false, true) => {
                return Ok(BaseBranch {
                    name: name.to_owned(),
                    start: Some(format!("origin/{name}")),
                });
            }
            (true, true) => {
                let behind = repo
                    .git([
                        "merge-base",
                        "--is-ancestor",
                        name,
                        &format!("origin/{name}"),
                    ])
                    .is_ok();
                return Ok(BaseBranch {
                    name: name.to_owned(),
                    start: Some(if behind {
                        format!("origin/{name}")
                    } else {
                        name.to_owned()
                    }),
                });
            }
        }
    }
    let current = repo.current_branch()?.unwrap_or_default();
    if !current.is_empty() && !has_commit(repo, "HEAD") {
        return Ok(BaseBranch {
            name: current,
            start: None,
        });
    }
    let name = if current.is_empty() {
        "HEAD".to_owned()
    } else {
        current
    };
    Ok(BaseBranch {
        start: Some(name.clone()),
        name,
    })
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ReposFile {
    #[serde(default)]
    repos: Vec<RegisteredRepo>,
}

/// Persistent registry backed by `repos.json`. Every mutation saves immediately.
#[derive(Debug)]
pub struct RepoStore {
    paths: Paths,
    repos: Vec<RegisteredRepo>,
}

impl RepoStore {
    /// Load `repos.json` (empty if missing).
    pub fn open(paths: &Paths) -> Result<Self> {
        let file: ReposFile = read_json(&paths.repos_file())?.unwrap_or_default();
        Ok(RepoStore {
            paths: paths.clone(),
            repos: file.repos,
        })
    }

    /// Whether `repos.json` exists (the browse-roots migration ran or a repo was registered).
    pub fn exists(paths: &Paths) -> bool {
        paths.repos_file().exists()
    }

    /// All registered repositories in registration order.
    pub fn list(&self) -> &[RegisteredRepo] {
        &self.repos
    }

    /// Look up a repository.
    pub fn get(&self, id: &RepoId) -> Option<&RegisteredRepo> {
        self.repos.iter().find(|r| &r.id == id)
    }

    /// Look up a repository or fail with [`Error::RepoNotFound`].
    pub fn require(&self, id: &RepoId) -> Result<&RegisteredRepo> {
        self.get(id)
            .ok_or_else(|| Error::RepoNotFound(id.to_string()))
    }

    /// The registered repository whose main checkout is `path`.
    pub fn find_path(&self, path: &Path) -> Option<&RegisteredRepo> {
        self.repos.iter().find(|r| same_path(&r.path, path))
    }

    /// Register the repository containing `path` (its main checkout). Not a git repository ->
    /// [`Error::NotAGitRepo`]; an already registered repository returns the existing entry.
    pub fn register(&mut self, path: &Path) -> Result<RegisteredRepo> {
        let repo = Repo::open(path)?;
        if let Some(r) = self.find_path(repo.root()) {
            return Ok(r.clone());
        }
        let mut id = RepoId::new();
        while self.get(&id).is_some() {
            id = RepoId::new();
        }
        let name = repo
            .root()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".into());
        let r = RegisteredRepo {
            id,
            name,
            path: repo.root().to_path_buf(),
            base_branch: None,
            created_at: now_unix(),
        };
        self.repos.push(r.clone());
        self.save()?;
        Ok(r)
    }

    /// Keep a session's repository whose folder is gone (migration): registered by path without
    /// opening it, so the session can still be listed and deleted.
    pub fn adopt_missing(&mut self, path: &Path) -> Result<RegisteredRepo> {
        if let Some(r) = self.find_path(path) {
            return Ok(r.clone());
        }
        let r = RegisteredRepo {
            id: RepoId::new(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "repo".into()),
            path: path.to_path_buf(),
            base_branch: None,
            created_at: now_unix(),
        };
        self.repos.push(r.clone());
        self.save()?;
        Ok(r)
    }

    /// Remove a registration (files untouched). The caller checks that no session uses it.
    pub fn unregister(&mut self, id: &RepoId) -> Result<RegisteredRepo> {
        let idx = self
            .repos
            .iter()
            .position(|r| &r.id == id)
            .ok_or_else(|| Error::RepoNotFound(id.to_string()))?;
        let r = self.repos.remove(idx);
        self.save()?;
        Ok(r)
    }

    /// Write `repos.json` (also when empty: marks the migration as done).
    pub fn save(&self) -> Result<()> {
        #[derive(Serialize)]
        struct Out<'a> {
            repos: &'a [RegisteredRepo],
        }
        write_json(&self.paths.repos_file(), &Out { repos: &self.repos })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;
    use crate::repo::test_repo;

    #[test]
    fn ids() {
        let a = RepoId::new();
        assert_eq!(a.as_str().len(), 8);
        assert_eq!(RepoId::parse(a.as_str()).unwrap(), a);
        for bad in ["", "ABCDEF12", "1234567", "../x/abc", "123456789"] {
            assert!(RepoId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn register_unregister() {
        let t = TempDir::new("reg");
        let paths = Paths::new(t.0.join("data"));
        let a = t.0.join("레포 a");
        test_repo::init(&a);
        std::fs::create_dir_all(a.join("sub")).unwrap();
        let mut store = RepoStore::open(&paths).unwrap();
        assert!(!RepoStore::exists(&paths));
        let r = store.register(&a.join("sub")).unwrap();
        assert_eq!(r.name, "레포 a");
        assert!(same_path(&r.path, &a));
        // Same repo again (also from a linked worktree): the existing entry.
        assert_eq!(store.register(&a).unwrap().id, r.id);
        let id = crate::session::SessionId::new();
        let wt = Repo::open(&a)
            .unwrap()
            .create_worktree(&paths, &id, None)
            .unwrap();
        assert_eq!(store.register(&wt.path).unwrap().id, r.id);
        assert!(matches!(
            store.register(&t.0.join("missing")),
            Err(Error::NotAGitRepo { .. })
        ));
        let reopened = RepoStore::open(&paths).unwrap();
        assert_eq!(reopened.list(), store.list());
        store.unregister(&r.id).unwrap();
        assert!(matches!(
            store.unregister(&r.id),
            Err(Error::RepoNotFound(_))
        ));
        assert!(RepoStore::open(&paths).unwrap().list().is_empty());
        assert!(a.join("README.md").is_file());
    }

    #[test]
    fn base_detection() {
        let t = TempDir::new("base");
        let a = t.0.join("a");
        test_repo::init(&a);
        let repo = Repo::open(&a).unwrap();
        let b = detect_base(&repo, None).unwrap();
        assert_eq!(b.name, "main");
        assert_eq!(b.start.as_deref(), Some("main"));
        // master when there is no main.
        let m = t.0.join("m");
        std::fs::create_dir_all(&m).unwrap();
        test_repo::run(&m, &["init", "-q", "-b", "master"]);
        test_repo::run(&m, &["commit", "-q", "--allow-empty", "-m", "x"]);
        test_repo::run(&m, &["checkout", "-q", "-b", "feature"]);
        let b = detect_base(&Repo::open(&m).unwrap(), None).unwrap();
        assert_eq!(b.name, "master");
        // Clone: origin/main ahead of a local main that is only behind -> start origin/main.
        let c = t.0.join("c");
        test_repo::run(&t.0, &["clone", "-q", &a.display().to_string(), "c"]);
        test_repo::run(&a, &["commit", "-q", "--allow-empty", "-m", "next"]);
        test_repo::run(&c, &["fetch", "-q"]);
        let b = detect_base(&Repo::open(&c).unwrap(), None).unwrap();
        assert_eq!(b.start.as_deref(), Some("origin/main"));
        // Configured base.
        test_repo::run(&a, &["branch", "release"]);
        let b = detect_base(&repo, Some("release")).unwrap();
        assert_eq!((b.name.as_str(), b.start.as_deref()), ("release", Some("refs/heads/release")));
        assert!(detect_base(&repo, Some("nope")).is_err());
        // Unborn repository: no start.
        let u = t.0.join("u");
        std::fs::create_dir_all(&u).unwrap();
        test_repo::run(&u, &["init", "-q", "-b", "trunk"]);
        let b = detect_base(&Repo::open(&u).unwrap(), None).unwrap();
        assert_eq!((b.name.as_str(), b.start), ("trunk", None));
    }
}
