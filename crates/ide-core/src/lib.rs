//! Accounts, configurations, sessions, git worktrees and launch helpers. UI-free.
//!
//! Typical server flow:
//! ```no_run
//! use ide_core::*;
//! # fn main() -> Result<()> {
//! let paths = Paths::from_env()?;
//! migrate_profiles(&paths)?; // legacy profiles -> accounts + configurations
//! let configs = ConfigurationStore::open(&paths)?;
//! let mut sessions = SessionStore::open(&paths)?;
//!
//! let req = NewSession {
//!     repo_path: "C:/src/my-repo".into(),
//!     target: SessionTarget {
//!         provider: "shell".into(),
//!         account: None,
//!         configuration: configs.list()[0].slug.clone(),
//!     },
//!     title: String::new(),
//!     provider_name: "Shell".into(),
//!     base_ref: String::new(),
//! };
//! // The precheck resolves the agent program before the worktree is created.
//! let session = create_session(&paths, &req, |_planned| Ok(()))?;
//! sessions.upsert(session.clone())?;
//! // `ide-agents` turns (session, account, configuration) into a `LaunchSpec`.
//! // ... later: stop the process, then
//! delete_session(&paths, &mut sessions, &session.id, true)?;
//! # Ok(()) }
//! ```

pub mod account;
pub mod configuration;
pub mod error;
pub mod launch;
pub mod migrate;
pub mod ops;
pub mod paths;
pub mod repo;
pub mod session;
pub mod slug;
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
pub use ops::{
    NewSession, create_login_session, create_session, delete_session, remove_session_dir,
    remove_session_files,
};
pub use paths::{DATA_DIR_ENV, Paths, TempDir, ensure_dir, now_unix, read_json, write_json};
pub use repo::{GitOutput, Repo, git_raw, same_path};
pub use session::{Session, SessionId, SessionKind, SessionStatus, SessionStore, SessionTarget};
pub use slug::{slugify, validate_slug};
pub use worktree::{BRANCH_PREFIX, Worktree, WorktreeInfo, worktree_exists};
