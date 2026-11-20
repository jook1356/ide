//! Per-session git worktrees under `worktrees/<repo-slug>/<session-id>`.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::paths::{Paths, ensure_dir};
use crate::repo::{Repo, same_path};
use crate::session::SessionId;

/// Prefix of every branch created for a session.
pub const BRANCH_PREFIX: &str = "ide/";

/// A worktree created for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Worktree directory (absolute).
    pub path: PathBuf,
    /// New branch checked out in it (`ide/<session-id>`).
    pub branch: String,
    /// Commit the branch was created from.
    pub base_commit: String,
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeInfo {
    /// Worktree directory as reported by git.
    pub path: PathBuf,
    /// Checked-out commit (`None` for bare entries).
    pub head: Option<String>,
    /// Checked-out branch without `refs/heads/` (`None` if detached or bare).
    pub branch: Option<String>,
    /// `true` for the repository's main checkout (first entry).
    pub is_main: bool,
    /// `true` if the worktree is locked.
    pub locked: bool,
    /// `true` if git considers the worktree prunable (its directory is gone).
    pub prunable: bool,
}

/// Whether `path` still looks like a linked worktree (directory with a `.git` file).
pub fn worktree_exists(path: &Path) -> bool {
    path.join(".git").is_file()
}

impl Repo {
    /// Where the worktree for `id` lives: `<data>/worktrees/<repo-slug>/<id>` (absolute).
    pub fn worktree_path(&self, paths: &Paths, id: &SessionId) -> PathBuf {
        let p = paths.worktree_dir(&self.slug(), id.as_str());
        std::path::absolute(&p).unwrap_or(p)
    }

    /// Create a worktree for session `id` on a new branch `ide/<id>`, starting from `base`
    /// (any revision) or the main checkout's `HEAD`. The main checkout is not modified.
    pub fn create_worktree(
        &self,
        paths: &Paths,
        id: &SessionId,
        base: Option<&str>,
    ) -> Result<Worktree> {
        let path = self.worktree_path(paths, id);
        if path.exists() {
            return Err(Error::WorktreeExists(path));
        }
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        let base_commit = self.resolve_commit(base.unwrap_or("HEAD"))?;
        let branch = id.branch();
        self.git([
            "worktree".as_ref(),
            "add".as_ref(),
            "-b".as_ref(),
            branch.as_ref(),
            path.as_os_str(),
            base_commit.as_ref(),
        ])?;
        Ok(Worktree {
            path,
            branch,
            base_commit,
        })
    }

    /// All worktrees of this repository, main checkout first.
    pub fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>> {
        Ok(parse_porcelain(&self.git([
            "worktree",
            "list",
            "--porcelain",
        ])?))
    }

    /// The registered worktree at `path`, if any.
    pub fn find_worktree(&self, path: &Path) -> Result<Option<WorktreeInfo>> {
        Ok(self
            .list_worktrees()?
            .into_iter()
            .find(|w| same_path(&w.path, path)))
    }

    /// Remove the linked worktree at `path` (`git worktree remove`, `--force` discards local
    /// changes). If its directory is already gone, the stale registration is pruned instead.
    /// Refuses to touch the main checkout. The branch is kept; see [`Repo::delete_branch`].
    pub fn remove_worktree(&self, path: &Path, force: bool) -> Result<()> {
        if same_path(path, self.root()) {
            return Err(Error::Git {
                args: format!("worktree remove {}", path.display()),
                status: "refused".into(),
                stderr: "refusing to remove the main checkout".into(),
            });
        }
        if path.exists() {
            let mut args = vec!["worktree".as_ref(), "remove".as_ref()];
            if force {
                args.push("--force".as_ref());
            }
            args.push(path.as_os_str());
            self.git(args)?;
        }
        self.prune_worktrees()
    }

    /// `git worktree prune`: forget worktrees whose directories were deleted.
    pub fn prune_worktrees(&self) -> Result<()> {
        self.git(["worktree", "prune"]).map(drop)
    }

    /// Delete a session branch (`-D` when `force`). Only `ide/...` branches may be deleted.
    pub fn delete_branch(&self, branch: &str, force: bool) -> Result<()> {
        if !branch.starts_with(BRANCH_PREFIX) {
            return Err(Error::Git {
                args: format!("branch -d {branch}"),
                status: "refused".into(),
                stderr: format!("only {BRANCH_PREFIX}* branches can be deleted"),
            });
        }
        self.git(["branch", if force { "-D" } else { "-d" }, "--", branch])
            .map(drop)
    }
}

fn parse_porcelain(out: &str) -> Vec<WorktreeInfo> {
    let mut list = Vec::new();
    let mut cur: Option<WorktreeInfo> = None;
    for line in out.lines().map(|l| l.trim_end_matches('\r')) {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "worktree" => {
                list.extend(cur.take());
                cur = Some(WorktreeInfo {
                    path: PathBuf::from(value),
                    ..Default::default()
                });
            }
            _ => {
                let Some(w) = cur.as_mut() else { continue };
                match key {
                    "HEAD" => w.head = Some(value.to_owned()),
                    "branch" => {
                        w.branch = Some(
                            value
                                .strip_prefix("refs/heads/")
                                .unwrap_or(value)
                                .to_owned(),
                        )
                    }
                    "locked" => w.locked = true,
                    "prunable" => w.prunable = true,
                    _ => {}
                }
            }
        }
    }
    list.extend(cur);
    if let Some(first) = list.first_mut() {
        first.is_main = true;
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;
    use crate::repo::test_repo;

    #[test]
    fn porcelain_parsing() {
        let out = "worktree C:/r\nHEAD aaa\nbranch refs/heads/main\n\n\
                   worktree C:/w/1\nHEAD bbb\ndetached\nlocked why\n\n\
                   worktree C:/w/2\nHEAD ccc\nbranch refs/heads/ide/2\nprunable gitdir file points to non-existent location\n";
        let l = parse_porcelain(out);
        assert_eq!(l.len(), 3);
        assert!(l[0].is_main && l[0].branch.as_deref() == Some("main"));
        assert!(!l[1].is_main && l[1].branch.is_none() && l[1].locked);
        assert_eq!(l[2].branch.as_deref(), Some("ide/2"));
        assert!(l[2].prunable);
    }

    #[test]
    fn create_list_remove() {
        let t = TempDir::new("wt");
        let repo_dir = t.0.join("repo 저장소");
        test_repo::init(&repo_dir);
        let paths = Paths::new(t.0.join("data"));
        let repo = Repo::open(&repo_dir).unwrap();
        let head = repo.head().unwrap();

        let id = SessionId::new();
        let wt = repo.create_worktree(&paths, &id, None).unwrap();
        assert_eq!(wt.branch, format!("ide/{id}"));
        assert_eq!(wt.base_commit, head);
        assert!(wt.path.ends_with(Path::new(&repo.slug()).join(id.as_str())));
        assert!(worktree_exists(&wt.path));
        assert!(wt.path.join("README.md").is_file());
        assert!(matches!(
            repo.create_worktree(&paths, &id, None),
            Err(Error::WorktreeExists(_))
        ));

        // A second worktree from an explicit base ref.
        test_repo::run(
            &repo_dir,
            &["commit", "-q", "--allow-empty", "-m", "second"],
        );
        let id2 = SessionId::new();
        let wt2 = repo.create_worktree(&paths, &id2, Some("HEAD~1")).unwrap();
        assert_eq!(wt2.base_commit, head);

        // Main checkout untouched.
        assert_eq!(repo.current_branch().unwrap().as_deref(), Some("main"));
        assert_eq!(test_repo::run(&repo_dir, &["status", "--porcelain"]), "");

        // Opening from inside a worktree resolves to the main checkout.
        assert!(same_path(Repo::open(&wt.path).unwrap().root(), repo.root()));

        let list = repo.list_worktrees().unwrap();
        assert_eq!(list.len(), 3);
        assert!(list[0].is_main && same_path(&list[0].path, repo.root()));
        let found = repo.find_worktree(&wt.path).unwrap().expect("listed");
        assert_eq!(found.branch.as_deref(), Some(wt.branch.as_str()));

        // Dirty worktree needs force.
        std::fs::write(wt.path.join("new.txt"), "x").unwrap();
        assert!(repo.remove_worktree(&wt.path, false).is_err());
        repo.remove_worktree(&wt.path, true).unwrap();
        assert!(!wt.path.exists() && !worktree_exists(&wt.path));
        repo.delete_branch(&wt.branch, true).unwrap();

        // Directory deleted behind git's back -> remove prunes the registration.
        std::fs::remove_dir_all(&wt2.path).unwrap();
        assert!(!worktree_exists(&wt2.path));
        repo.remove_worktree(&wt2.path, false).unwrap();
        assert_eq!(repo.list_worktrees().unwrap().len(), 1);

        assert!(repo.remove_worktree(repo.root(), true).is_err());
        assert!(repo.delete_branch("main", true).is_err());
        assert!(repo_dir.join("README.md").is_file());
    }
}
