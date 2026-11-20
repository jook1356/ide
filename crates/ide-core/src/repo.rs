//! Git repositories, accessed through the `git` CLI.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::slug::slugify;

/// Raw result of a `git` run ([`git_raw`]).
#[derive(Debug, Clone)]
pub struct GitOutput {
    /// Exit code (`None` if killed by a signal).
    pub code: Option<i32>,
    /// Raw stdout bytes (file contents, diffs: not necessarily UTF-8).
    pub stdout: Vec<u8>,
    /// Raw stderr bytes.
    pub stderr: Vec<u8>,
}

impl GitOutput {
    /// [`Error::Git`] for this output (`args` = the arguments, for the message).
    pub fn error<S: AsRef<OsStr>>(&self, args: &[S]) -> Error {
        Error::Git {
            args: args
                .iter()
                .map(|a| a.as_ref().to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            status: self
                .code
                .map_or_else(|| "signal".to_owned(), |c| format!("exit code: {c}")),
            stderr: String::from_utf8_lossy(&self.stderr).trim().to_owned(),
        }
    }
}

/// Run `git -C cwd -c core.quotepath=false <args>` without a terminal prompt (stdin closed,
/// `GIT_TERMINAL_PROMPT=0`, no console window on Windows) and return its raw output whatever
/// the exit code. `env` adds variables (e.g. `GIT_OPTIONAL_LOCKS=0`). Does not go through the
/// server's `Runner` (see docs/server-architecture.md).
pub fn git_raw<S: AsRef<OsStr>>(cwd: &Path, args: &[S], env: &[(&str, &str)]) -> Result<GitOutput> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(cwd)
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    tracing::debug!(
        cwd = %cwd.display(),
        args = ?args.iter().map(|a| a.as_ref().to_string_lossy()).collect::<Vec<_>>(),
        "git"
    );
    let out = cmd.output().map_err(Error::GitSpawn)?;
    Ok(GitOutput {
        code: out.status.code(),
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// Run `git` with `args` in `cwd`; returns trimmed stdout or [`Error::Git`] with stderr.
pub(crate) fn git<I, S>(cwd: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
    let out = git_raw(cwd, &args, &[])?;
    if out.code == Some(0) {
        Ok(String::from_utf8_lossy(&out.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_owned())
    } else {
        Err(out.error(&args))
    }
}

/// A git repository identified by its main checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// Open the repository containing `path`. If `path` is inside a linked worktree, the
    /// repository's main checkout is used as the root. Fails with [`Error::NotAGitRepo`]
    /// (carrying git's stderr) if `path` is not inside a git work tree, or is a bare repo.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let not_repo = |stderr: String| Error::NotAGitRepo {
            path: path.to_path_buf(),
            stderr,
        };
        let toplevel = git(path, ["rev-parse", "--show-toplevel"]).map_err(|e| match e {
            Error::Git { stderr, .. } => not_repo(stderr),
            other => other,
        })?;
        if toplevel.is_empty() {
            return Err(not_repo("no work tree (bare repository?)".into()));
        }
        let common = git(
            path,
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let common = PathBuf::from(common);
        let root = match (common.file_name(), common.parent()) {
            (Some(name), Some(parent)) if name == ".git" => parent.to_path_buf(),
            _ => PathBuf::from(toplevel),
        };
        Ok(Self { root })
    }

    /// The main checkout's directory (as reported by git, forward slashes on Windows).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory-safe identifier: slugified folder name plus a short hash of the full path,
    /// so two repos with the same folder name do not collide.
    pub fn slug(&self) -> String {
        let name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut slug = slugify(&name, "repo");
        slug.truncate(54);
        let slug = slug.trim_end_matches(['-', '_']);
        format!(
            "{slug}-{:08x}",
            fnv1a(normalize_path(&self.root).as_bytes()) as u32
        )
    }

    /// Commit id of `HEAD`.
    pub fn head(&self) -> Result<String> {
        self.resolve_commit("HEAD")
    }

    /// Name of the currently checked-out branch of the main checkout, or `None` if detached.
    pub fn current_branch(&self) -> Result<Option<String>> {
        let b = git(&self.root, ["branch", "--show-current"])?;
        Ok((!b.is_empty()).then_some(b))
    }

    /// Resolve any revision (branch, tag, sha, `HEAD~1`) to a full commit id.
    pub fn resolve_commit(&self, rev: &str) -> Result<String> {
        git(
            &self.root,
            [
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{rev}^{{commit}}"),
            ],
        )
    }

    pub(crate) fn git<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        git(&self.root, args)
    }
}

/// Compare-friendly form of a path: forward slashes, no trailing slash, lowercase on Windows.
pub(crate) fn normalize_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s
        .strip_prefix("//?/")
        .unwrap_or(&s)
        .trim_end_matches('/')
        .to_owned();
    if cfg!(windows) { s.to_lowercase() } else { s }
}

/// Whether two paths name the same location (string comparison after normalisation).
pub fn same_path(a: &Path, b: &Path) -> bool {
    normalize_path(a) == normalize_path(b)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
pub(crate) mod test_repo {
    use std::path::Path;
    use std::process::Command;

    /// `git init` + one commit in `dir`, without touching global config.
    pub fn init(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        run(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        run(dir, &["add", "README.md"]);
        run(dir, &["commit", "-q", "-m", "init"]);
    }

    pub fn run(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=ide-test",
                "-c",
                "user.email=ide-test@example.invalid",
            ])
            .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;

    #[test]
    fn open_repo_and_head() {
        let t = TempDir::new("repo-open");
        let dir = t.0.join("My Repo");
        test_repo::init(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let repo = Repo::open(dir.join("sub")).unwrap();
        assert!(
            same_path(repo.root(), &dir),
            "{:?} vs {:?}",
            repo.root(),
            dir
        );
        assert_eq!(repo.head().unwrap().len(), 40);
        assert_eq!(repo.current_branch().unwrap().as_deref(), Some("main"));
        let slug = repo.slug();
        assert!(slug.starts_with("my-repo-"), "{slug}");
        crate::slug::validate_slug(&slug).unwrap();
        assert!(matches!(
            repo.resolve_commit("nope"),
            Err(Error::Git { .. })
        ));
    }

    #[test]
    fn not_a_repo() {
        let t = TempDir::new("repo-none");
        let err = Repo::open(t.0.join("missing")).unwrap_err();
        match err {
            Error::NotAGitRepo { stderr, .. } => assert!(!stderr.is_empty()),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn slug_differs_by_path() {
        let a = Repo {
            root: PathBuf::from("C:/a/proj"),
        };
        let b = Repo {
            root: PathBuf::from("C:/b/proj"),
        };
        assert_ne!(a.slug(), b.slug());
        assert!(a.slug().starts_with("proj-"));
        assert_eq!(
            Repo {
                root: PathBuf::from("C:/한글")
            }
            .slug()
            .len(),
            "repo-".len() + 8
        );
    }
}
