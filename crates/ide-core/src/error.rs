//! Error type shared by every `ide-core` module.
//!
//! `Display` is English and meant for logs. UIs translate errors through [`Error::i18n`], which
//! returns a language-agnostic [`ErrorMessage`] (stable id + arguments); `ide-core` itself never
//! depends on a translation crate.

use std::fmt;
use std::path::{Path, PathBuf};

/// Result alias using [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The filesystem operation behind an [`Error::Io`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoAction {
    /// Reading a file.
    Reading,
    /// Writing a file.
    Writing,
    /// Replacing a file with a freshly written one.
    Replacing,
    /// Creating a directory.
    CreatingDirectory,
    /// Renaming a file or directory.
    Renaming,
    /// Removing a file or directory.
    Removing,
}

impl IoAction {
    /// Stable identifier (`"reading"`, `"creating-directory"`, ...).
    pub fn id(self) -> &'static str {
        match self {
            IoAction::Reading => "reading",
            IoAction::Writing => "writing",
            IoAction::Replacing => "replacing",
            IoAction::CreatingDirectory => "creating-directory",
            IoAction::Renaming => "renaming",
            IoAction::Removing => "removing",
        }
    }
}

impl fmt::Display for IoAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            IoAction::Reading => "reading",
            IoAction::Writing => "writing",
            IoAction::Replacing => "replacing",
            IoAction::CreatingDirectory => "creating directory",
            IoAction::Renaming => "renaming",
            IoAction::Removing => "removing",
        })
    }
}

/// Why a name was rejected by [`crate::validate_slug`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRule {
    /// The name is empty.
    Empty,
    /// The name is longer than [`crate::slug::MAX_SLUG_LEN`].
    TooLong,
    /// The name contains characters other than `[a-z0-9_-]`.
    InvalidChars,
    /// The name does not start with `[a-z0-9]`.
    BadStart,
}

impl NameRule {
    /// Stable identifier (`"empty"`, `"too-long"`, ...).
    pub fn id(self) -> &'static str {
        match self {
            NameRule::Empty => "empty",
            NameRule::TooLong => "too-long",
            NameRule::InvalidChars => "invalid-chars",
            NameRule::BadStart => "bad-start",
        }
    }
}

impl fmt::Display for NameRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NameRule::Empty => "must not be empty",
            NameRule::TooLong => "must be at most 64 characters",
            NameRule::InvalidChars => "only lowercase letters, digits, '-' and '_' are allowed",
            NameRule::BadStart => "must start with a letter or digit",
        })
    }
}

/// Everything that can go wrong in `ide-core`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation failed.
    #[error("{action} {path}: {source}")]
    Io {
        /// What was being done.
        action: IoAction,
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

    /// A name (account, configuration, provider id) is not a valid slug.
    #[error("invalid name {name:?}: {reason}")]
    InvalidName {
        /// The rejected name.
        name: String,
        /// Why it was rejected.
        reason: NameRule,
    },

    /// No account with this provider and slug exists.
    #[error("account {provider}/{slug} not found")]
    AccountNotFound {
        /// Provider id.
        provider: String,
        /// Account slug.
        slug: String,
    },

    /// The account is used by sessions and cannot be deleted.
    #[error("account {name:?} is used by {count} session(s)")]
    AccountInUse {
        /// Account display name.
        name: String,
        /// Sessions using it.
        count: usize,
    },

    /// No configuration with this slug exists.
    #[error("configuration {0:?} not found")]
    ConfigurationNotFound(String),

    /// The configuration is used by sessions and cannot be deleted.
    #[error("configuration {name:?} is used by {count} session(s)")]
    ConfigurationInUse {
        /// Configuration display name.
        name: String,
        /// Sessions using it.
        count: usize,
    },

    /// The last remaining configuration cannot be deleted.
    #[error("cannot delete the last configuration {0:?}")]
    LastConfiguration(String),

    /// No agent provider with this id is registered.
    #[error("agent provider {0:?} not found")]
    ProviderNotFound(String),

    /// The provider has no login command (or no accounts at all).
    #[error("agent provider {0:?} has no login command")]
    ProviderNoLogin(String),

    /// An MCP server name is empty, too long, has characters other than `[A-Za-z0-9_-]` or
    /// is used twice in one configuration.
    #[error("invalid or duplicate MCP server name {0:?}")]
    InvalidMcpName(String),

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

/// A language-agnostic description of an [`Error`] for UIs: one variant per message, holding
/// the message arguments. Untranslatable details (OS error text, git's stderr) are passed
/// through as `detail`. UIs match on it exhaustively, so a new error variant is a compile error
/// in every UI until it gets a translation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum ErrorMessage<'a> {
    Io {
        action: IoAction,
        path: &'a Path,
        detail: String,
    },
    Json {
        path: &'a Path,
        detail: String,
    },
    NoDataDir,
    InvalidName {
        name: &'a str,
        reason: NameRule,
    },
    AccountNotFound {
        provider: &'a str,
        slug: &'a str,
    },
    AccountInUse {
        name: &'a str,
        count: usize,
    },
    ConfigurationNotFound {
        name: &'a str,
    },
    ConfigurationInUse {
        name: &'a str,
        count: usize,
    },
    LastConfiguration {
        name: &'a str,
    },
    ProviderNotFound {
        provider: &'a str,
    },
    ProviderNoLogin {
        provider: &'a str,
    },
    InvalidMcpName {
        name: &'a str,
    },
    SessionNotFound {
        id: &'a str,
    },
    InvalidSessionId {
        id: &'a str,
    },
    NotAGitRepo {
        path: &'a Path,
        detail: &'a str,
    },
    GitSpawn {
        detail: String,
    },
    Git {
        args: &'a str,
        status: &'a str,
        detail: &'a str,
    },
    WorktreeExists {
        path: &'a Path,
    },
    ProgramNotFound {
        program: &'a str,
    },
}

impl ErrorMessage<'_> {
    /// Stable message id (kebab-case), e.g. `"not-a-git-repo"`.
    pub fn id(&self) -> &'static str {
        match self {
            ErrorMessage::Io { .. } => "io",
            ErrorMessage::Json { .. } => "json",
            ErrorMessage::NoDataDir => "no-data-dir",
            ErrorMessage::InvalidName { .. } => "invalid-name",
            ErrorMessage::AccountNotFound { .. } => "account-not-found",
            ErrorMessage::AccountInUse { .. } => "account-in-use",
            ErrorMessage::ConfigurationNotFound { .. } => "configuration-not-found",
            ErrorMessage::ConfigurationInUse { .. } => "configuration-in-use",
            ErrorMessage::LastConfiguration { .. } => "last-configuration",
            ErrorMessage::ProviderNotFound { .. } => "provider-not-found",
            ErrorMessage::ProviderNoLogin { .. } => "provider-no-login",
            ErrorMessage::InvalidMcpName { .. } => "invalid-mcp-name",
            ErrorMessage::SessionNotFound { .. } => "session-not-found",
            ErrorMessage::InvalidSessionId { .. } => "invalid-session-id",
            ErrorMessage::NotAGitRepo { .. } => "not-a-git-repo",
            ErrorMessage::GitSpawn { .. } => "git-spawn",
            ErrorMessage::Git { .. } => "git",
            ErrorMessage::WorktreeExists { .. } => "worktree-exists",
            ErrorMessage::ProgramNotFound { .. } => "program-not-found",
        }
    }
}

impl Error {
    /// An [`Error::Io`] (for crates building on `ide-core`).
    pub fn io(action: IoAction, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            action,
            path: path.into(),
            source,
        }
    }

    /// The message id and arguments for showing this error in a UI language.
    pub fn i18n(&self) -> ErrorMessage<'_> {
        match self {
            Error::Io {
                action,
                path,
                source,
            } => ErrorMessage::Io {
                action: *action,
                path,
                detail: source.to_string(),
            },
            Error::Json { path, source } => ErrorMessage::Json {
                path,
                detail: source.to_string(),
            },
            Error::NoDataDir => ErrorMessage::NoDataDir,
            Error::InvalidName { name, reason } => ErrorMessage::InvalidName {
                name,
                reason: *reason,
            },
            Error::AccountNotFound { provider, slug } => {
                ErrorMessage::AccountNotFound { provider, slug }
            }
            Error::AccountInUse { name, count } => ErrorMessage::AccountInUse {
                name,
                count: *count,
            },
            Error::ConfigurationNotFound(name) => ErrorMessage::ConfigurationNotFound { name },
            Error::ConfigurationInUse { name, count } => ErrorMessage::ConfigurationInUse {
                name,
                count: *count,
            },
            Error::LastConfiguration(name) => ErrorMessage::LastConfiguration { name },
            Error::ProviderNotFound(provider) => ErrorMessage::ProviderNotFound { provider },
            Error::ProviderNoLogin(provider) => ErrorMessage::ProviderNoLogin { provider },
            Error::InvalidMcpName(name) => ErrorMessage::InvalidMcpName { name },
            Error::SessionNotFound(id) => ErrorMessage::SessionNotFound { id },
            Error::InvalidSessionId(id) => ErrorMessage::InvalidSessionId { id },
            Error::NotAGitRepo { path, stderr } => ErrorMessage::NotAGitRepo {
                path,
                detail: stderr,
            },
            Error::GitSpawn(e) => ErrorMessage::GitSpawn {
                detail: e.to_string(),
            },
            Error::Git {
                args,
                status,
                stderr,
            } => ErrorMessage::Git {
                args,
                status,
                detail: stderr,
            },
            Error::WorktreeExists(path) => ErrorMessage::WorktreeExists { path },
            Error::ProgramNotFound { program } => ErrorMessage::ProgramNotFound { program },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i18n_ids_and_args() {
        let e = Error::NotAGitRepo {
            path: "C:/x".into(),
            stderr: "fatal: not a git repository".into(),
        };
        let m = e.i18n();
        assert_eq!(m.id(), "not-a-git-repo");
        assert_eq!(
            m,
            ErrorMessage::NotAGitRepo {
                path: Path::new("C:/x"),
                detail: "fatal: not a git repository"
            }
        );
        let e = Error::io(
            IoAction::CreatingDirectory,
            "C:/y",
            std::io::Error::other("boom"),
        );
        // Display stays English for logs.
        assert_eq!(e.to_string(), "creating directory C:/y: boom");
        assert!(matches!(
            e.i18n(),
            ErrorMessage::Io { action: IoAction::CreatingDirectory, detail, .. } if detail == "boom"
        ));
        let e = Error::InvalidName {
            name: "A".into(),
            reason: NameRule::InvalidChars,
        };
        assert_eq!(e.i18n().id(), "invalid-name");
        assert!(e.to_string().contains("only lowercase letters"));
    }
}
