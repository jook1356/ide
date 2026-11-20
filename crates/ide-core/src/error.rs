//! Error type shared by every `ide-core` module.

use std::path::PathBuf;

/// Result alias using [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong in `ide-core`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation failed.
    #[error("{action} {path}: {source}")]
    Io {
        /// What was being done (e.g. "reading", "creating directory").
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A persisted JSON file could not be parsed or written.
    #[error("invalid JSON in {path}: {source}")]
    Json {
        /// The file involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: serde_json::Error,
    },

    /// No home/app-data directory could be determined and `IDE_DATA_DIR` is unset.
    #[error("cannot determine the app data directory (set IDE_DATA_DIR)")]
    NoDataDir,

    /// A profile name is not a valid slug.
    #[error("invalid name {name:?}: {reason}")]
    InvalidName {
        /// The rejected name.
        name: String,
        /// Why it was rejected.
        reason: &'static str,
    },

    /// A profile with this name already exists.
    #[error("profile {0:?} already exists")]
    ProfileExists(String),

    /// No profile with this name exists.
    #[error("profile {0:?} not found")]
    ProfileNotFound(String),

    /// The last remaining profile cannot be deleted.
    #[error("cannot delete the last profile {0:?}")]
    LastProfile(String),

    /// No session with this id exists.
    #[error("session {0} not found")]
    SessionNotFound(String),

    /// A session id string is malformed.
    #[error("invalid session id {0:?}")]
    InvalidSessionId(String),

    /// The given path is not inside a git work tree.
    #[error("{path} is not a git repository: {stderr}")]
    NotAGitRepo {
        /// The path that was checked.
        path: PathBuf,
        /// git's stderr.
        stderr: String,
    },

    /// The `git` executable could not be started.
    #[error("failed to run git: {0}")]
    GitSpawn(#[source] std::io::Error),

    /// A git command exited unsuccessfully.
    #[error("git {args} failed ({status}): {stderr}")]
    Git {
        /// The arguments passed to git, space separated.
        args: String,
        /// Exit status description.
        status: String,
        /// git's stderr (trimmed).
        stderr: String,
    },

    /// The worktree target directory already exists.
    #[error("worktree path already exists: {0}")]
    WorktreeExists(PathBuf),

    /// The agent program could not be found on `PATH`.
    #[error("{program:?} not found on PATH; install it or add it to PATH")]
    ProgramNotFound {
        /// The program name that was searched for.
        program: String,
    },
}

impl Error {
    pub(crate) fn io(
        action: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Error::Io {
            action,
            path: path.into(),
            source,
        }
    }
}
