//! `ide-server`: the per-user backend daemon (see `docs/server-architecture.md`).
//!
//! - [`Server`] holds the services (providers, accounts, configurations, sessions, terminals) and serves connections
//!   over any `AsyncRead + AsyncWrite` ([`Server::serve`]).
//! - [`daemon`]: local socket listener, `--stdio` relay, detached daemon start.
//! - Multi-user foundation: [`UserInfo`](ide_proto::UserInfo) from the transport,
//!   [`Server::authorize`] / [`UserCtx`] resolve every resource for the connection's user,
//!   all spawns go through [`Runner`], [`Audit`] records actions, [`RootJail`] confines every
//!   client path to one repo worktree of one session (`fs.*`, `search.*`, `git.*`, `def.lookup`).

pub mod audit;
mod conn;
pub mod daemon;
pub mod def;
pub mod fsops;
pub mod git;
pub mod jail;
pub mod pty;
pub mod ring;
pub mod runner;
pub mod search;
pub mod service;
pub mod user;
pub mod watch;

pub use audit::{Audit, FileAudit, NoAudit};
pub use jail::RootJail;
pub use pty::TermHost;
pub use runner::{PtyChild, Runner, SameUserRunner, SpawnSpec};
pub use service::{SERVER_CONFIG_FILE, Server, ServerConfig, UserCtx, core_error};
pub use user::{SOCKET_ENV, current_user, socket_name};
