//! Profiles, sessions, git worktrees and agent launch specs. UI-free.
//!
//! Typical app flow:
//! ```no_run
//! use ide_core::*;
//! # fn main() -> Result<()> {
//! let paths = Paths::from_env()?;
//! let profiles = ProfileStore::open(&paths)?;
//! let mut sessions = SessionStore::open(&paths)?;
//!
//! let repo = Repo::open("C:/src/my-repo")?;
//! let id = SessionId::new();
//! let wt = repo.create_worktree(&paths, &id, None)?;
//! let session = Session::new(id, "my session", DEFAULT_PROFILE, &repo, wt, AgentKind::Claude);
//! sessions.upsert(session.clone())?;
//!
//! let spec = LaunchSpec::for_session(&session, profiles.get(DEFAULT_PROFILE).unwrap())?;
//! // hand `spec` to the PTY layer
//!
//! // Or, in one step (resolves the program before creating the worktree):
//! let req = NewSession {
//!     repo_path: "C:/src/my-repo".into(),
//!     agent: AgentKind::Shell,
//!     title: String::new(),
//!     base_ref: String::new(),
//! };
//! let (session, spec) = create_session(&paths, profiles.get(DEFAULT_PROFILE).unwrap(), &req)?;
//! sessions.upsert(session.clone())?;
//! // ... later: stop the process, then
//! delete_session(&paths, &mut sessions, &session.id, true)?;
//! # Ok(()) }
//! ```

pub mod error;
pub mod launch;
pub mod ops;
pub mod paths;
pub mod profile;
pub mod repo;
pub mod session;
pub mod slug;
pub mod worktree;

pub use error::{Error, ErrorMessage, IoAction, NameRule, Result};
pub use launch::{LaunchEnv, LaunchSpec, is_batch_file, which, which_in};
pub use ops::{
    NewSession, create_session, create_session_with, delete_session, remove_session_files,
};
pub use paths::{DATA_DIR_ENV, Paths};
pub use profile::{DEFAULT_PROFILE, Profile, ProfileStore};
pub use repo::{Repo, same_path};
pub use session::{AgentKind, Session, SessionId, SessionStatus, SessionStore};
pub use slug::{slugify, validate_slug};
pub use worktree::{BRANCH_PREFIX, Worktree, WorktreeInfo, worktree_exists};
