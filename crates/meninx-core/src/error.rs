//! Error type shared by every `meninx-core` module.
//!
//! `Display` is English and meant for logs. UIs translate errors through [`Error::i18n`], which
//! returns a language-agnostic [`ErrorMessage`] (stable id + arguments); `meninx-core` itself never
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

/// Everything that can go wrong in `meninx-core`.
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

    /// No home/app-data directory could be determined and `MENINX_DATA_DIR` is unset.
    #[error("cannot determine the app data directory (set MENINX_DATA_DIR)")]
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

    /// The configuration has no skill with this folder name.
    #[error("skill {0:?} not found")]
    SkillNotFound(String),

    /// A skill with this folder name exists already.
    #[error("skill {0:?} exists already")]
    SkillExists(String),

    /// The folder is not a skill (no `SKILL.md`).
    #[error("{0:?} is not a skill folder (no SKILL.md)")]
    InvalidSkill(PathBuf),

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

    /// An agent CLI's one-time sandbox setup failed or was refused (e.g. the UAC prompt).
    #[error("the sandbox setup failed: {0}")]
    SandboxSetup(String),

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

    /// No registered repository with this id exists.
    #[error("repository {0:?} is not registered")]
    RepoNotFound(String),

    /// The repository is attached to sessions and cannot be unregistered.
    #[error("repository {name:?} is used by {count} session(s)")]
    RepoInUse {
        /// Repository name.
        name: String,
        /// Sessions using it.
        count: usize,
    },

    /// The session has no worktree of this repository.
    #[error("repository {name:?} is not attached to the session")]
    RepoNotAttached {
        /// Repository name (or id).
        name: String,
    },

    /// The operation needs a session workspace (not an original-repo or login session).
    #[error("session {0} has no workspace")]
    NotAWorkspaceSession(String),

    /// Children need a workspace parent (`docs/session-family.md`).
    #[error("session {0} has no workspace: it cannot have child sessions")]
    FamilyNeedsWorkspace(String),

    /// A child would be deeper than two levels below its root session.
    #[error("child sessions go at most two levels deep")]
    FamilyTooDeep,

    /// A separate child picked a repository its parent does not have.
    #[error("the parent session has no repository {name:?}")]
    RepoNotInParent {
        /// Repository name (or id).
        name: String,
    },

    /// A session with child sessions cannot be deleted.
    #[error("the session has {count} child session(s)")]
    SessionHasChildren {
        /// Children.
        count: usize,
    },

    /// A parent cannot detach a repository its separate children work in.
    #[error("{count} child session(s) use repository {name:?}")]
    RepoUsedByChildren {
        /// Repository name.
        name: String,
        /// Children using it.
        count: usize,
    },

    /// A shared child works in its parent's workspace: change the parent's repositories.
    #[error("a shared child session uses its parent's repositories")]
    SharedChildRepos,

    /// A plain folder (no git repository): registering needs `init` (Desk `NotGitRepoError`).
    #[error("{0} is not a git repository (register with init to git-initialise it)")]
    NotAGitFolder(PathBuf),

    /// The folder does not exist.
    #[error("folder not found: {0}")]
    FolderNotFound(PathBuf),

    /// `git init` refused: a broken `.git` is in the way.
    #[error("a broken .git exists, not initialising: {0}")]
    CorruptGitDir(PathBuf),

    /// A network git command was stopped after its time limit.
    #[error("git {command} stopped after {secs} s")]
    GitTimeout {
        /// git subcommand (`fetch`, `ls-remote`).
        command: String,
        /// Time limit in seconds.
        secs: u64,
    },

    /// 기준 브랜치 최신화 (fetch of the base before creating a worktree) failed.
    #[error("refreshing the base branch failed: {0}")]
    BaseRefreshFailed(String),

    /// 기준 브랜치 최신화 needs a remote, and the base has none.
    #[error("the base branch has no remote; turn off refreshing or set an upstream")]
    BaseNoRemote,

    /// The base start point is not a branch (nothing to refresh).
    #[error("choose a base branch to refresh")]
    BaseNotBranch,

    /// Fetching a rule's / session's base branch failed.
    #[error("could not fetch the base branch: {0}")]
    BaseFetchFailed(String),

    /// No remote branch matches the repository's base branch rule.
    #[error("no remote branch matches the base branch rule")]
    BaseRuleNoMatch,

    /// The base branch rule is invalid; `field` = `remote`, `pattern` or `match`.
    #[error("invalid base branch rule ({0})")]
    InvalidBaseRule(&'static str),

    /// The repository has no remote of this name.
    #[error("no such remote: {0}")]
    RemoteNotFound(String),

    /// `git ls-remote` failed.
    #[error("could not list the remote branches: {0}")]
    RemoteListFailed(String),

    /// A fixed base branch is invalid or does not exist locally.
    #[error("base branch is invalid or not present locally: {0}")]
    InvalidBaseBranch(String),

    /// A base picked for a new session is not in origin's last-fetched branches.
    #[error("origin has no branch {0} (turn on refreshing the base branch)")]
    OriginBranchMissing(String),

    /// A session base must be one of origin's branches.
    #[error("origin has no branch {0} (fetch and try again)")]
    OriginBranchUnknown(String),
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
    SkillNotFound {
        name: &'a str,
    },
    SkillExists {
        name: &'a str,
    },
    InvalidSkill {
        path: &'a Path,
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
    SandboxSetup {
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
    RepoNotFound {
        id: &'a str,
    },
    RepoInUse {
        name: &'a str,
        count: usize,
    },
    RepoNotAttached {
        name: &'a str,
    },
    NotAWorkspaceSession {
        id: &'a str,
    },
    FamilyNeedsWorkspace {
        id: &'a str,
    },
    FamilyTooDeep,
    RepoNotInParent {
        name: &'a str,
    },
    SessionHasChildren {
        count: usize,
    },
    RepoUsedByChildren {
        name: &'a str,
        count: usize,
    },
    SharedChildRepos,
    NotAGitFolder {
        path: &'a Path,
    },
    FolderNotFound {
        path: &'a Path,
    },
    CorruptGitDir {
        path: &'a Path,
    },
    GitTimeout {
        command: &'a str,
        secs: u64,
    },
    BaseRefreshFailed {
        detail: &'a str,
    },
    BaseNoRemote,
    BaseNotBranch,
    BaseFetchFailed {
        detail: &'a str,
    },
    BaseRuleNoMatch,
    InvalidBaseRule {
        field: &'static str,
    },
    RemoteNotFound {
        remote: &'a str,
    },
    RemoteListFailed {
        detail: &'a str,
    },
    InvalidBaseBranch {
        branch: &'a str,
    },
    OriginBranchMissing {
        branch: &'a str,
    },
    OriginBranchUnknown {
        branch: &'a str,
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
            ErrorMessage::SkillNotFound { .. } => "skill-not-found",
            ErrorMessage::SkillExists { .. } => "skill-exists",
            ErrorMessage::InvalidSkill { .. } => "skill-invalid",
            ErrorMessage::ProviderNotFound { .. } => "provider-not-found",
            ErrorMessage::ProviderNoLogin { .. } => "provider-no-login",
            ErrorMessage::InvalidMcpName { .. } => "invalid-mcp-name",
            ErrorMessage::SessionNotFound { .. } => "session-not-found",
            ErrorMessage::InvalidSessionId { .. } => "invalid-session-id",
            ErrorMessage::NotAGitRepo { .. } => "not-a-git-repo",
            ErrorMessage::GitSpawn { .. } => "git-spawn",
            ErrorMessage::SandboxSetup { .. } => "sandbox-setup",
            ErrorMessage::Git { .. } => "git",
            ErrorMessage::WorktreeExists { .. } => "worktree-exists",
            ErrorMessage::ProgramNotFound { .. } => "program-not-found",
            ErrorMessage::RepoNotFound { .. } => "repo-not-found",
            ErrorMessage::RepoInUse { .. } => "repo-in-use",
            ErrorMessage::RepoNotAttached { .. } => "repo-not-attached",
            ErrorMessage::NotAWorkspaceSession { .. } => "not-a-workspace-session",
            ErrorMessage::FamilyNeedsWorkspace { .. } => "family-needs-workspace",
            ErrorMessage::FamilyTooDeep => "family-too-deep",
            ErrorMessage::RepoNotInParent { .. } => "repo-not-in-parent",
            ErrorMessage::SessionHasChildren { .. } => "session-has-children",
            ErrorMessage::RepoUsedByChildren { .. } => "repo-used-by-children",
            ErrorMessage::SharedChildRepos => "shared-child-repos",
            ErrorMessage::NotAGitFolder { .. } => "not-a-git-folder",
            ErrorMessage::FolderNotFound { .. } => "folder-not-found",
            ErrorMessage::CorruptGitDir { .. } => "corrupt-git-dir",
            ErrorMessage::GitTimeout { .. } => "git-timeout",
            ErrorMessage::BaseRefreshFailed { .. } => "base-refresh-failed",
            ErrorMessage::BaseNoRemote => "base-no-remote",
            ErrorMessage::BaseNotBranch => "base-not-branch",
            ErrorMessage::BaseFetchFailed { .. } => "base-fetch-failed",
            ErrorMessage::BaseRuleNoMatch => "base-rule-no-match",
            ErrorMessage::InvalidBaseRule { .. } => "invalid-base-rule",
            ErrorMessage::RemoteNotFound { .. } => "remote-not-found",
            ErrorMessage::RemoteListFailed { .. } => "remote-list-failed",
            ErrorMessage::InvalidBaseBranch { .. } => "invalid-base-branch",
            ErrorMessage::OriginBranchMissing { .. } => "origin-branch-missing",
            ErrorMessage::OriginBranchUnknown { .. } => "origin-branch-unknown",
        }
    }
}

impl Error {
    /// An [`Error::Io`] (for crates building on `meninx-core`).
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
            Error::SkillNotFound(name) => ErrorMessage::SkillNotFound { name },
            Error::SkillExists(name) => ErrorMessage::SkillExists { name },
            Error::InvalidSkill(path) => ErrorMessage::InvalidSkill { path },
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
            Error::SandboxSetup(d) => ErrorMessage::SandboxSetup { detail: d.clone() },
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
            Error::RepoNotFound(id) => ErrorMessage::RepoNotFound { id },
            Error::RepoInUse { name, count } => ErrorMessage::RepoInUse {
                name,
                count: *count,
            },
            Error::RepoNotAttached { name } => ErrorMessage::RepoNotAttached { name },
            Error::NotAWorkspaceSession(id) => ErrorMessage::NotAWorkspaceSession { id },
            Error::FamilyNeedsWorkspace(id) => ErrorMessage::FamilyNeedsWorkspace { id },
            Error::FamilyTooDeep => ErrorMessage::FamilyTooDeep,
            Error::RepoNotInParent { name } => ErrorMessage::RepoNotInParent { name },
            Error::SessionHasChildren { count } => {
                ErrorMessage::SessionHasChildren { count: *count }
            }
            Error::RepoUsedByChildren { name, count } => ErrorMessage::RepoUsedByChildren {
                name,
                count: *count,
            },
            Error::SharedChildRepos => ErrorMessage::SharedChildRepos,
            Error::NotAGitFolder(path) => ErrorMessage::NotAGitFolder { path },
            Error::FolderNotFound(path) => ErrorMessage::FolderNotFound { path },
            Error::CorruptGitDir(path) => ErrorMessage::CorruptGitDir { path },
            Error::GitTimeout { command, secs } => ErrorMessage::GitTimeout {
                command,
                secs: *secs,
            },
            Error::BaseRefreshFailed(detail) => ErrorMessage::BaseRefreshFailed { detail },
            Error::BaseNoRemote => ErrorMessage::BaseNoRemote,
            Error::BaseNotBranch => ErrorMessage::BaseNotBranch,
            Error::BaseFetchFailed(detail) => ErrorMessage::BaseFetchFailed { detail },
            Error::BaseRuleNoMatch => ErrorMessage::BaseRuleNoMatch,
            Error::InvalidBaseRule(field) => ErrorMessage::InvalidBaseRule { field },
            Error::RemoteNotFound(remote) => ErrorMessage::RemoteNotFound { remote },
            Error::RemoteListFailed(detail) => ErrorMessage::RemoteListFailed { detail },
            Error::InvalidBaseBranch(branch) => ErrorMessage::InvalidBaseBranch { branch },
            Error::OriginBranchMissing(branch) => ErrorMessage::OriginBranchMissing { branch },
            Error::OriginBranchUnknown(branch) => ErrorMessage::OriginBranchUnknown { branch },
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
