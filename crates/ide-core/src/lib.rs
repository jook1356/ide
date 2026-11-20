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
//! # Ok(()) }
//! ```

pub mod error;
pub mod launch;
pub mod paths;
pub mod profile;
pub mod repo;
pub mod session;
pub mod slug;
pub mod worktree;

pub use error::{Error, Result};
pub use launch::{LaunchEnv, LaunchSpec, which, which_in};
pub use paths::{DATA_DIR_ENV, Paths};
pub use profile::{DEFAULT_PROFILE, Profile, ProfileStore};
pub use repo::{Repo, same_path};
pub use session::{AgentKind, Session, SessionId, SessionStatus, SessionStore};
pub use slug::{slugify, validate_slug};
pub use worktree::{BRANCH_PREFIX, Worktree, WorktreeInfo, worktree_exists};
