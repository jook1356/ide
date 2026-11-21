//! Accounts, configurations, sessions, git worktrees and launch helpers. UI-free.
//!
//! Typical server flow:
//! ```no_run
//! use ide_core::*;
//! # fn main() -> Result<()> {
//! let paths = Paths::from_env()?;
//! migrate_profiles(&paths)?; // legacy profiles -> accounts + configurations
//! let configs = ConfigurationStore::open(&paths)?;
//! let mut repos = RepoStore::open(&paths)?;
//! let mut sessions = SessionStore::open(&paths)?;
//! migrate_workspaces(&paths, &mut repos, &mut sessions, &[])?;
//! let repo = repos.register("C:/src/my-repo".as_ref())?;
//! let req = NewSession {
//!     target: SessionTarget {
//!         provider: "shell".into(),
//!         account: None,
//!         configuration: configs.list()[0].slug.clone(),
//!     },
//!     title: String::new(),
//!     provider_name: "Shell".into(),
//!     repos: vec![repo.id.clone()],
//!     original_repo: None,
//! };
//! // The precheck resolves the agent program before anything is created.
//! let (session, _failures) = create_session(&paths, &repos, &req, |_planned| Ok(()))?;
//! sessions.upsert(session.clone())?;
//! // `ide-agents` turns (session, account, configuration) into a `LaunchSpec`.
//! // ... later: stop the process, then
//! remove_session_files(&paths, &session, &repos, true)?;
//! sessions.remove(&session.id)?;
//! # Ok(()) }
//! ```

pub mod account;
pub mod configuration;
pub mod error;
pub mod launch;
pub mod migrate;
pub mod ops;
pub mod paths;
pub mod registry;
pub mod repo;
pub mod session;
pub mod slug;
pub mod workspace;
pub mod worktree;

pub use account::{Account, AccountStore};
pub use configuration::{
    Configuration, ConfigurationStore, DEFAULT_CONFIGURATION, McpServer, SkillRef,
    validate_mcp_name,
};
pub use error::{Error, ErrorMessage, IoAction, NameRule, Result};
pub use launch::{
    LaunchEnv, LaunchSpec, is_batch_file, resolve_program, shell_program, which, which_in,
    wrap_batch_file,
};
pub use migrate::{MigrationReport, migrate_profiles};
pub use ops::{create_login_session, remove_session_dir};
pub use registry::{BaseBranch, RegisteredRepo, RepoId, RepoStore, detect_base};
pub use workspace::{
    NewSession, RepoFailure, WorkspaceMigration, attach_repo, create_session, detach_repo,
    is_managed_path, migrate_workspaces, remove_session_files, session_guidance,
};
pub use paths::{DATA_DIR_ENV, Paths, TempDir, ensure_dir, now_unix, read_json, write_json};
pub use repo::{GitOutput, Repo, git_raw, same_path};
pub use session::{
    Session, SessionId, SessionKind, SessionMode, SessionStatus, SessionStore, SessionTarget,
    SessionWorktree,
};
pub use slug::{slugify, validate_slug};
pub use worktree::{BRANCH_PREFIX, Worktree, WorktreeInfo, worktree_exists};
