//! Path jail: client paths must resolve inside the user's allowed roots after
//! canonicalization; `..` components, malformed names and symlink/junction escapes are
//! rejected. Every file/git/search RPC goes through it:
//! - session APIs take **root-relative** paths and use a [`RootJail`] for the session's
//!   worktree ([`RootJail::resolve`] follows symlinks, [`RootJail::resolve_entry`] does not
//!   follow the last component, for delete/rename/stat of a link itself);
//! - repository browsing takes absolute paths inside the browse roots ([`PathJail`]).

use std::path::{Component, Path, PathBuf};

use ide_proto::ProtoError;

/// `path-denied`: outside the allowed roots.
pub fn denied(path: &str) -> ProtoError {
    ProtoError::new("path-denied").arg("path", path)
}

/// `path-invalid`: not an acceptable root-relative path.
pub fn invalid(path: &str) -> ProtoError {
    ProtoError::new("path-invalid").arg("path", path)
}

/// `canonicalize` without the Windows verbatim prefix for drive paths (`\\?\C:\x` -> `C:\x`),
/// so paths stay usable by git and readable in messages. UNC paths keep `\\?\UNC\`.
pub fn canonicalize(p: &Path) -> std::io::Result<PathBuf> {
    let c = std::fs::canonicalize(p)?;
    Ok(simplify(c))
}

/// Strip `\\?\` from a verbatim drive path.
pub fn simplify(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.as_os_str().to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            let b = rest.as_bytes();
            if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
                return PathBuf::from(rest.to_owned());
            }
        }
    }
    p
}

const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Validate a root-relative path and split it into components. Accepts `/` and `\`
/// separators, skips empty and `.` segments; rejects absolute paths, drive/UNC prefixes,
/// `..`, NUL and control characters, `:` (Windows drive letters and alternate data streams),
/// Windows device names (`con`, `nul.txt`, ...) and names ending in a dot or space.
pub fn split_rel(path: &str) -> Result<Vec<&str>, ProtoError> {
    if path.starts_with(['/', '\\']) || path.contains(':') {
        return Err(invalid(path));
    }
    let mut out = Vec::new();
    for seg in path.split(['/', '\\']) {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(denied(path));
        }
        if seg
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
            || seg.ends_with(['.', ' '])
        {
            return Err(invalid(path));
        }
        let stem = seg.split('.').next().unwrap_or("").to_ascii_lowercase();
        if RESERVED.contains(&stem.trim_end()) {
            return Err(invalid(path));
        }
        out.push(seg);
    }
    Ok(out)
}

/// Normalized form of a root-relative path (`a/b`, `""` for the root).
pub fn normalize_rel(path: &str) -> Result<String, ProtoError> {
    Ok(split_rel(path)?.join("/"))
}

/// Canonicalize the longest existing prefix of `joined` and append the missing tail.
fn canonical_prefix(joined: &Path, err: impl Fn() -> ProtoError) -> Result<PathBuf, ProtoError> {
    let mut existing = joined;
    let mut tail = Vec::new();
    let canon = loop {
        match canonicalize(existing) {
            Ok(c) => break c,
            Err(_) => {
                tail.push(existing.file_name().ok_or_else(&err)?.to_owned());
                existing = existing.parent().ok_or_else(&err)?;
            }
        }
    };
    let mut full = canon;
    for t in tail.iter().rev() {
        full.push(t);
    }
    Ok(full)
}

/// Allowed roots for one user (absolute paths: repository browsing).
#[derive(Debug, Clone, Default)]
pub struct PathJail {
    roots: Vec<PathBuf>,
}

impl PathJail {
    /// Jail with these roots (canonicalized; missing roots are skipped).
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut j = PathJail::default();
        for r in roots {
            j.add_root(&r);
        }
        j
    }

    /// Add a root.
    pub fn add_root(&mut self, root: &Path) {
        if let Ok(r) = canonicalize(root)
            && !self.roots.contains(&r)
        {
            self.roots.push(r);
        }
    }

    /// The canonical roots.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Resolve a client path: relative paths are joined to the first root; the result (its
    /// longest existing ancestor, canonicalized, plus the missing tail) must lie inside a root.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, ProtoError> {
        let p = Path::new(path);
        if path.contains('\0') || p.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(denied(path));
        }
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.roots.first().ok_or_else(|| denied(path))?.join(p)
        };
        let full = canonical_prefix(&joined, || denied(path))?;
        if self.contains(&full) {
            Ok(full)
        } else {
            Err(denied(path))
        }
    }

    /// `p` (canonical) lies inside a root.
    pub fn contains(&self, p: &Path) -> bool {
        self.roots.iter().any(|r| p.starts_with(r))
    }
}

/// The jail of one root (a session worktree): root-relative paths only.
#[derive(Debug, Clone)]
pub struct RootJail {
    root: PathBuf,
}

impl RootJail {
    /// Jail for `root` (must exist; canonicalized).
    pub fn new(root: &Path) -> Result<Self, ProtoError> {
        let root = canonicalize(root)
            .map_err(|_| ProtoError::new("fs-not-found").arg("path", root.display().to_string()))?;
        Ok(RootJail { root })
    }

    /// The canonical root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn join(&self, parts: &[&str]) -> PathBuf {
        let mut p = self.root.clone();
        for s in parts {
            p.push(s);
        }
        p
    }

    /// Resolve `rel`, following symlinks: the canonical target must stay inside the root.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, ProtoError> {
        let parts = split_rel(rel)?;
        let full = canonical_prefix(&self.join(&parts), || denied(rel))?;
        if full.starts_with(&self.root) {
            Ok(full)
        } else {
            Err(denied(rel))
        }
    }

    /// Resolve `rel` without following its last component (the entry itself: a symlink is
    /// not followed). The parent must resolve inside the root; `rel` must not be the root.
    pub fn resolve_entry(&self, rel: &str) -> Result<PathBuf, ProtoError> {
        let parts = split_rel(rel)?;
        let Some((last, parent)) = parts.split_last() else {
            return Err(denied(rel));
        };
        let parent = canonical_prefix(&self.join(parent), || denied(rel))?;
        if !parent.starts_with(&self.root) {
            return Err(denied(rel));
        }
        Ok(parent.join(last))
    }

    /// Root-relative `/` form of an absolute path inside the root (`None` if outside).
    pub fn rel_of(&self, abs: &Path) -> Option<String> {
        let r = abs.strip_prefix(&self.root).ok()?;
        let parts: Vec<String> = r
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        Some(parts.join("/"))
    }
}

/// `rel` names `.git` or something inside it (protected from client writes).
pub fn is_git_internal(rel: &str) -> bool {
    split_rel(rel)
        .map(|p| p.iter().any(|s| s.eq_ignore_ascii_case(".git")))
        .unwrap_or(false)
}

/// Test helpers shared with other modules' tests.
#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};

    /// A fresh dir under `IDE_TEST_TMP` (or the system temp dir).
    pub fn tmp(tag: &str) -> PathBuf {
        let base = std::env::var_os("IDE_TEST_TMP")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("ide-server-tests"));
        let d = base.join(format!("{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A directory link (`mklink /J` junction on Windows: no privilege needed; symlink on
    /// Unix). False if it could not be created.
    pub fn make_dir_link(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            // mklink wants backslashes.
            let win = |p: &Path| p.display().to_string().replace('/', "\\");
            let out = std::process::Command::new("cmd")
                .arg("/c")
                .arg("mklink")
                .arg("/J")
                .arg(win(link))
                .arg(win(target))
                .output();
            matches!(out, Ok(o) if o.status.success()) && link.exists()
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }

    /// Remove a directory link without touching its target.
    pub fn remove_dir_link(link: &Path) {
        let _ = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link));
    }

    /// Run git in `dir` (test setup), panicking on failure.
    pub fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "core.quotepath=false", "-c", "core.autocrlf=false"])
            .args(args)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// `git init` with an identity, autocrlf off and one commit of `files`.
    pub fn init_repo(dir: &Path, files: &[(&str, &str)]) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        git(dir, &["config", "user.email", "t@example.com"]);
        git(dir, &["config", "user.name", "t"]);
        git(dir, &["config", "commit.gpgsign", "false"]);
        git(dir, &["config", "core.autocrlf", "false"]);
        for (p, c) in files {
            let f = dir.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, c).unwrap();
        }
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn inside_outside_and_dotdot() {
        let t = tmp("jail-a");
        let root = t.join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(t.join("other")).unwrap();
        let jail = PathJail::new([root.clone()]);
        let canon_root = canonicalize(&root).unwrap();
        assert_eq!(jail.resolve("sub").unwrap(), canon_root.join("sub"));
        assert_eq!(
            jail.resolve("sub/new/file.txt").unwrap(),
            canon_root.join("sub").join("new").join("file.txt")
        );
        assert!(
            jail.resolve(&root.join("sub").display().to_string())
                .is_ok()
        );
        assert_eq!(jail.resolve("../other").unwrap_err().id, "path-denied");
        assert!(jail.resolve("sub/../../other").is_err());
        assert!(
            jail.resolve(&t.join("other").display().to_string())
                .is_err()
        );
        // A sibling whose name extends the root's name is outside.
        std::fs::create_dir_all(t.join("root2")).unwrap();
        assert!(
            jail.resolve(&t.join("root2").display().to_string())
                .is_err()
        );
        assert!(PathJail::default().resolve("x").is_err());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn rel_paths() {
        assert_eq!(normalize_rel("a/./b//c").unwrap(), "a/b/c");
        assert_eq!(normalize_rel("a\\b").unwrap(), "a/b");
        assert_eq!(normalize_rel("").unwrap(), "");
        assert_eq!(normalize_rel("한글/파일.txt").unwrap(), "한글/파일.txt");
        for bad in [
            "/etc/passwd",
            "\\x",
            "C:/x",
            "c:x",
            "\\\\server\\share\\x",
            "//server/share/x",
            "a/file.txt:stream",
            "a\0b",
            "con",
            "a/NUL.txt",
            "lpt1.log",
            "trail.",
            "trail ",
            "a/b?",
        ] {
            assert!(split_rel(bad).is_err(), "{bad:?} accepted");
        }
        assert_eq!(split_rel("a/../b").unwrap_err().id, "path-denied");
        assert_eq!(split_rel("C:/x").unwrap_err().id, "path-invalid");
        assert!(split_rel("console.txt").is_ok());
        assert!(is_git_internal(".git/config"));
        assert!(is_git_internal("sub/.GIT"));
        assert!(!is_git_internal(".github/x"));
    }

    #[test]
    fn root_jail_korean_and_entries() {
        let t = tmp("jail-k");
        let root = t.join("작업 폴더");
        std::fs::create_dir_all(root.join("하위")).unwrap();
        std::fs::write(root.join("하위").join("파일.txt"), "x").unwrap();
        let j = RootJail::new(&root).unwrap();
        let f = j.resolve("하위/파일.txt").unwrap();
        assert!(f.is_file());
        assert_eq!(j.rel_of(&f).unwrap(), "하위/파일.txt");
        assert_eq!(j.resolve("").unwrap(), j.root());
        assert_eq!(j.resolve("하위/../../x").unwrap_err().id, "path-denied");
        // The root itself is not an entry (cannot be deleted/renamed).
        assert!(j.resolve_entry("").is_err());
        assert!(j.resolve_entry("하위/새 파일.txt").is_ok());
        #[cfg(windows)]
        assert!(!j.root().display().to_string().starts_with(r"\\?\"));
        let _ = std::fs::remove_dir_all(&t);
    }

    /// A link inside the root pointing outside: following it is denied, the link entry
    /// itself is reachable (for delete).
    #[test]
    fn link_escape() {
        let t = tmp("jail-b");
        let root = t.join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(t.join("secret")).unwrap();
        std::fs::write(t.join("secret").join("s.txt"), "s").unwrap();
        assert!(
            make_dir_link(&t.join("secret"), &root.join("link")),
            "cannot create a directory link"
        );
        let j = RootJail::new(&root).unwrap();
        assert_eq!(j.resolve("link/s.txt").unwrap_err().id, "path-denied");
        assert_eq!(j.resolve("link").unwrap_err().id, "path-denied");
        assert_eq!(j.resolve("link/new.txt").unwrap_err().id, "path-denied");
        assert_eq!(j.resolve_entry("link").unwrap(), j.root().join("link"));
        assert!(j.resolve_entry("link/s.txt").is_err());
        let pj = PathJail::new([root.clone()]);
        assert!(
            pj.resolve(&root.join("link").join("s.txt").display().to_string())
                .is_err()
        );
        remove_dir_link(&root.join("link"));
        assert!(t.join("secret").join("s.txt").exists());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[cfg(windows)]
    #[test]
    fn unc_paths() {
        let t = tmp("jail-u");
        let jail = PathJail::new([t.clone()]);
        assert!(jail.resolve(r"\\localhost\c$\Windows").is_err());
        assert!(jail.resolve(r"\\?\C:\Windows").is_err());
        assert!(jail.resolve(r"\\nonexistent-host-x\share\x").is_err());
        let s = jail.roots()[0].display().to_string();
        // An admin-share alias of a folder inside the root canonicalizes to a UNC path,
        // which is not inside the (drive-letter) root: denied rather than aliased.
        if let Some(rest) = s.strip_prefix("C:\\") {
            let unc = format!(r"\\localhost\c$\{rest}");
            if let Ok(r) = jail.resolve(&unc) {
                assert!(r.starts_with(&jail.roots()[0]));
            }
        }
        // The verbatim form of a path inside the root is fine.
        assert!(jail.resolve(&format!(r"\\?\{s}")).is_ok());
        let _ = std::fs::remove_dir_all(&t);
    }
}
