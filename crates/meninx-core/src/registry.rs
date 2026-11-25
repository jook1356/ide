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
    /// Rule-based base (Desk `baseBranchRule`); wins over `base_branch` while set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch_rule: Option<crate::base::BaseBranchRule>,
    /// The rule's last pick (`refs/remotes/<remote>/<branch>`, Desk `ruleResolved`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_resolved: Option<String>,
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

/// The base new worktrees of `repo` start from without network access: the configured fixed
/// base ([`crate::base::configured_base`]) when set, else automatic
/// ([`crate::base::main_branch`]). Rules and 기준 브랜치 최신화 are in [`crate::base::plan_base`].
pub fn detect_base(repo: &Repo, configured: Option<&str>) -> Result<BaseBranch> {
    match configured.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => crate::base::configured_base(repo, b),
        None => crate::base::main_branch(repo),
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ReposFile {
    #[serde(default)]
    repos: Vec<RegisteredRepo>,
}

/// Persistent registry backed by `repos.json`. Every mutation saves immediately.
#[derive(Debug, Clone)]
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

    /// Register the repository containing `path` (its main checkout); an already registered
    /// repository returns the existing entry (Desk `addRepo`). A missing folder ->
    /// [`Error::FolderNotFound`]; a plain folder -> [`Error::NotAGitFolder`] unless `init`, which
    /// git-initialises it first ([`crate::base::init_repository`]).
    pub fn register(&mut self, path: &Path, init: bool) -> Result<RegisteredRepo> {
        let repo = match Repo::open(path) {
            Ok(r) => r,
            Err(Error::NotAGitRepo { stderr, .. }) => {
                if !path.is_dir() {
                    return Err(Error::FolderNotFound(path.to_path_buf()));
                }
                if !stderr.to_lowercase().contains("not a git repository") {
                    return Err(Error::NotAGitRepo {
                        path: path.to_path_buf(),
                        stderr,
                    });
                }
                if !init {
                    return Err(Error::NotAGitFolder(path.to_path_buf()));
                }
                crate::base::init_repository(path)?;
                Repo::open(path)?
            }
            Err(e) => return Err(e),
        };
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
            base_branch_rule: None,
            rule_resolved: None,
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
            base_branch_rule: None,
            rule_resolved: None,
            created_at: now_unix(),
        };
        self.repos.push(r.clone());
        self.save()?;
        Ok(r)
    }

    fn require_mut(&mut self, id: &RepoId) -> Result<&mut RegisteredRepo> {
        self.repos
            .iter_mut()
            .find(|r| &r.id == id)
            .ok_or_else(|| Error::RepoNotFound(id.to_string()))
    }

    /// Desk `setRepoBaseBranch`: a fixed base (validated, stored as its full ref) or `None` =
    /// automatic. Replaces a rule. Only future worktrees are affected.
    pub fn set_base_branch(&mut self, id: &RepoId, base: Option<&str>) -> Result<RegisteredRepo> {
        let path = self.require(id)?.path.clone();
        let start = match base {
            Some(b) => crate::base::configured_base(&Repo::open(&path)?, b)?.start,
            None => None,
        };
        let r = self.require_mut(id)?;
        r.base_branch = start;
        r.base_branch_rule = None;
        r.rule_resolved = None;
        let r = r.clone();
        self.save()?;
        Ok(r)
    }

    /// Desk `setRepoBaseBranchRule`: save (checked against the remote's live branches: no match
    /// fails) or remove (`None`) a rule. Saving drops the fixed base.
    pub fn set_base_rule(
        &mut self,
        id: &RepoId,
        rule: Option<&crate::base::BaseBranchRule>,
    ) -> Result<RegisteredRepo> {
        let path = self.require(id)?.path.clone();
        let resolved = match rule {
            Some(rule) => {
                let clean = crate::base::clean_rule(rule)?;
                let (res, _) = crate::base::resolve_rule(&Repo::open(&path)?, &clean)?;
                let pick = res.pick.ok_or(Error::BaseRuleNoMatch)?;
                let tracking = format!("refs/remotes/{}/{pick}", clean.remote);
                Some((clean, tracking))
            }
            None => None,
        };
        let r = self.require_mut(id)?;
        match resolved {
            Some((clean, tracking)) => {
                r.base_branch_rule = Some(clean);
                r.rule_resolved = Some(tracking);
                r.base_branch = None;
            }
            None => {
                r.base_branch_rule = None;
                r.rule_resolved = None;
            }
        }
        let r = r.clone();
        self.save()?;
        Ok(r)
    }

    /// Record what creating a worktree learned: a rule's new pick, a legacy short fixed base's
    /// full ref. Saves when something changed.
    pub fn record_plan(
        &mut self,
        id: &RepoId,
        rule_resolved: Option<&str>,
        fixed_upgrade: Option<&str>,
    ) -> Result<()> {
        let Ok(r) = self.require_mut(id) else {
            return Ok(());
        };
        let mut changed = false;
        if let Some(t) = rule_resolved
            && r.base_branch_rule.is_some()
            && r.rule_resolved.as_deref() != Some(t)
        {
            r.rule_resolved = Some(t.to_owned());
            changed = true;
        }
        if let Some(f) = fixed_upgrade
            && r.base_branch.is_some()
            && r.base_branch.as_deref() != Some(f)
        {
            r.base_branch = Some(f.to_owned());
            changed = true;
        }
        if changed { self.save() } else { Ok(()) }
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
        let r = store.register(&a.join("sub"), false).unwrap();
        assert_eq!(r.name, "레포 a");
        assert!(same_path(&r.path, &a));
        // Same repo again (also from a linked worktree): the existing entry.
        assert_eq!(store.register(&a, false).unwrap().id, r.id);
        let id = crate::session::SessionId::new();
        let wt = Repo::open(&a)
            .unwrap()
            .create_worktree(&paths, &id, None)
            .unwrap();
        assert_eq!(store.register(&wt.path, false).unwrap().id, r.id);
        assert!(matches!(
            store.register(&t.0.join("missing"), false),
            Err(Error::FolderNotFound(_))
        ));
        // A plain folder: refused without init, git-initialised (branch main) with it.
        let plain = t.0.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("f.txt"), "x").unwrap();
        assert!(matches!(
            store.register(&plain, false),
            Err(Error::NotAGitFolder(_))
        ));
        assert!(!plain.join(".git").exists());
        let p = store.register(&plain, true).unwrap();
        assert_eq!(p.name, "plain");
        assert_eq!(detect_base(&Repo::open(&plain).unwrap(), None).unwrap().name, "main");
        store.unregister(&p.id).unwrap();
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
        // Fixed base saved as the full ref; a rule needs a remote.
        let paths = Paths::new(t.0.join("data"));
        let mut store = RepoStore::open(&paths).unwrap();
        let ra = store.register(&a, false).unwrap();
        let r = store.set_base_branch(&ra.id, Some("release")).unwrap();
        assert_eq!(r.base_branch.as_deref(), Some("refs/heads/release"));
        assert!(matches!(
            store.set_base_branch(&ra.id, Some("nope")),
            Err(Error::InvalidBaseBranch(_))
        ));
        let rule = crate::base::BaseBranchRule {
            remote: "origin".into(),
            pattern: "release/*".into(),
            matches: None,
        };
        assert!(matches!(
            store.set_base_rule(&ra.id, Some(&rule)),
            Err(Error::RemoteNotFound(_))
        ));
        assert!(store.set_base_branch(&ra.id, None).unwrap().base_branch.is_none());
        // Unborn repository: no start.
        let u = t.0.join("u");
        std::fs::create_dir_all(&u).unwrap();
        test_repo::run(&u, &["init", "-q", "-b", "trunk"]);
        let b = detect_base(&Repo::open(&u).unwrap(), None).unwrap();
        assert_eq!((b.name.as_str(), b.start), ("trunk", None));
    }
}
