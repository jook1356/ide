//! Location of the app data directory and the files inside it.
//!
//! Layout (relative to the data root):
//! ```text
//! profiles.json
//! sessions.json
//! profiles/<slug>/{claude,codex}/
//! worktrees/<repo-slug>/<session-id>/
//! ```

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Environment variable overriding the app data root.
pub const DATA_DIR_ENV: &str = "IDE_DATA_DIR";

/// Resolved app data directory. Cheap to clone; all stores take one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Resolve the data root: `IDE_DATA_DIR` if set and non-empty, otherwise the platform
    /// data dir from `directories::ProjectDirs::from("dev", "ide", "ide")`.
    pub fn from_env() -> Result<Self> {
        if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
            return Ok(Self::new(PathBuf::from(dir)));
        }
        let dirs = directories::ProjectDirs::from("dev", "ide", "ide").ok_or(Error::NoDataDir)?;
        Ok(Self::new(dirs.data_dir().to_path_buf()))
    }

    /// Use an explicit data root (tests, custom setups). Nothing is created on disk.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The data root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `profiles.json`.
    pub fn profiles_file(&self) -> PathBuf {
        self.root.join("profiles.json")
    }

    /// `sessions.json`.
    pub fn sessions_file(&self) -> PathBuf {
        self.root.join("sessions.json")
    }

    /// `profiles/<slug>`.
    pub fn profile_dir(&self, slug: &str) -> PathBuf {
        self.root.join("profiles").join(slug)
    }

    /// `worktrees/`.
    pub fn worktrees_dir(&self) -> PathBuf {
        self.root.join("worktrees")
    }

    /// `worktrees/<repo-slug>/<session-id>`.
    pub fn worktree_dir(&self, repo_slug: &str, session_id: &str) -> PathBuf {
        self.worktrees_dir().join(repo_slug).join(session_id)
    }
}

/// `create_dir_all` with a contextual error.
pub(crate) fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| Error::io("creating directory", path, e))
}

/// Read a JSON file; `Ok(None)` if it does not exist.
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io("reading", path, e)),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| Error::Json {
            path: path.to_path_buf(),
            source,
        })
}

/// Write JSON atomically (temp file in the same dir, then rename over the target).
pub(crate) fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(|e| Error::io("writing", &tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| Error::io("replacing", path, e))
}

/// Current time as Unix seconds.
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh, empty directory for one test. Base dir: `IDE_TEST_TMP` if set, else the
    /// system temp dir. Removed on drop.
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let base = std::env::var_os("IDE_TEST_TMP")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let dir = base.join(format!(
                "ide-core-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let p = Paths::new("/data");
        assert_eq!(p.profiles_file(), Path::new("/data").join("profiles.json"));
        assert_eq!(p.sessions_file(), Path::new("/data").join("sessions.json"));
        assert_eq!(
            p.profile_dir("a"),
            Path::new("/data").join("profiles").join("a")
        );
        assert_eq!(
            p.worktree_dir("r", "s"),
            Path::new("/data").join("worktrees").join("r").join("s")
        );
    }

    #[test]
    fn json_round_trip_and_missing() {
        let t = test_util::TempDir::new("json");
        let f = t.0.join("sub").join("x.json");
        assert_eq!(read_json::<Vec<u32>>(&f).unwrap(), None);
        write_json(&f, &vec![1u32, 2]).unwrap();
        write_json(&f, &vec![3u32]).unwrap();
        assert_eq!(read_json::<Vec<u32>>(&f).unwrap(), Some(vec![3]));
    }
}
