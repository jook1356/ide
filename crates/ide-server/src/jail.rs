//! Path jail (phase 2 uses it for file/git/search RPCs): client paths must resolve inside the
//! user's allowed roots after canonicalization; `..` components and symlink escapes are
//! rejected.

use std::path::{Component, Path, PathBuf};

use ide_proto::ProtoError;

/// Allowed roots for one user.
#[derive(Debug, Clone, Default)]
pub struct PathJail {
    roots: Vec<PathBuf>,
}

impl PathJail {
    /// Jail with these roots (canonicalized; missing roots are skipped).
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        PathJail {
            roots: roots
                .into_iter()
                .filter_map(|r| std::fs::canonicalize(r).ok())
                .collect(),
        }
    }

    /// Add a root.
    pub fn add_root(&mut self, root: &Path) {
        if let Ok(r) = std::fs::canonicalize(root) {
            self.roots.push(r);
        }
    }

    /// Resolve a client path: relative paths are joined to the first root; the result (its
    /// longest existing ancestor, canonicalized, plus the missing tail) must lie inside a root.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, ProtoError> {
        let denied = || ProtoError::new("path-denied").arg("path", path);
        let p = Path::new(path);
        if p.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(denied());
        }
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.roots.first().ok_or_else(denied)?.join(p)
        };
        // Canonicalize the longest existing prefix so symlinks are resolved.
        let mut existing = joined.as_path();
        let mut tail = Vec::new();
        let canon = loop {
            match std::fs::canonicalize(existing) {
                Ok(c) => break c,
                Err(_) => {
                    tail.push(existing.file_name().ok_or_else(denied)?.to_owned());
                    existing = existing.parent().ok_or_else(denied)?;
                }
            }
        };
        let mut full = canon;
        for t in tail.iter().rev() {
            full.push(t);
        }
        if self.roots.iter().any(|r| full.starts_with(r)) {
            Ok(full)
        } else {
            Err(denied())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let base = std::env::var_os("IDE_TEST_TMP")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("ide-server-tests"));
        let d = base.join(format!("jail-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn inside_outside_and_dotdot() {
        let t = tmp("a");
        let root = t.join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(t.join("other")).unwrap();
        let jail = PathJail::new([root.clone()]);
        let canon_root = std::fs::canonicalize(&root).unwrap();
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
        assert!(PathJail::default().resolve("x").is_err());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape() {
        let t = tmp("b");
        let root = t.join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(t.join("secret")).unwrap();
        std::os::unix::fs::symlink(t.join("secret"), root.join("link")).unwrap();
        let jail = PathJail::new([root]);
        assert!(jail.resolve("link/x").is_err());
        let _ = std::fs::remove_dir_all(&t);
    }
}
