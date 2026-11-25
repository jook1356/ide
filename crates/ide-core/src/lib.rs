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
//! let repo = repos.register("C:/src/my-repo".as_ref(), false)?;
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
//!     refresh_base: true, // 기준 브랜치 최신화
//!     background: false,
//!     bases: Default::default(),
//! };
//! // The precheck resolves the agent program before anything is created.
//! let (session, _failures, _plans) = create_session(&paths, &repos, &req, |_planned| Ok(()))?;
//! sessions.upsert(session.clone())?;
//! // `ide-agents` turns (session, account, configuration) into a `LaunchSpec`.
//! // ... later: stop the process, then
//! remove_session_files(&paths, &session, &repos, true)?;
//! sessions.remove(&session.id)?;
//! # Ok(()) }
//! ```

pub mod account;
pub mod approval;
pub mod auto_answer;
pub mod base;
pub mod configuration;
pub mod error;
pub mod family;
pub mod launch;
pub mod migrate;
pub mod ops;
pub mod paths;
pub mod preview;
pub mod profile_files;
pub mod profile_history;
pub mod prompts;
pub mod registry;
pub mod repo;
pub mod session;
pub mod skill;
pub mod slug;
pub mod widget;
pub mod workspace;
pub mod worktree;
pub mod write_guard;

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
pub use base::{BasePlan, BaseBranchRule, BaseRuleResult, repo_default_base, short_branch};
pub use registry::{BaseBranch, RegisteredRepo, RepoId, RepoStore, detect_base};
pub use workspace::{
    NewSession, PENDING_MARKER, RepoFailure, WorkspaceMigration, add_worktree, attach_repo,
    create_session, detach_repo, is_managed_path, migrate_workspaces, pin_session_bases,
    remove_session_files, session_guidance, set_session_base, write_pending_marker,
};
pub use profile_history::{ChangeNote, ChangeSource, HistoryEntry};
pub use paths::{DATA_DIR_ENV, Paths, TempDir, ensure_dir, now_unix, read_json, write_json};
pub use repo::{GitOutput, Repo, git_raw, same_path};
pub use session::{
    AutoTitle,
    PendingWorktree, Session, SessionId, SessionKind, SessionMode, SessionStatus, SessionStore,
    SessionTarget, SessionWorktree,
};
pub use skill::{SKILL_FILE, SkillEntry};
pub use slug::{slugify, validate_slug};
// --- pull requests and agent prompts
pub use prompts::{commit_prompt, pr_file_path, pr_prompt};
pub use worktree::{BRANCH_PREFIX, Worktree, WorktreeInfo, worktree_exists};
