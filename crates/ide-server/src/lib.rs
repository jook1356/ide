//! `ide-server`: the per-user backend daemon (see `docs/server-architecture.md`).
//!
//! - [`Server`] holds the services (profiles, sessions, terminals) and serves connections
//!   over any `AsyncRead + AsyncWrite` ([`Server::serve`]).
//! - [`daemon`]: local socket listener, `--stdio` relay, detached daemon start.
//! - Multi-user foundation: [`UserInfo`](ide_proto::UserInfo) from the transport,
//!   [`Server::authorize`] / [`UserCtx`] resolve every resource for the connection's user,
//!   all spawns go through [`Runner`], [`Audit`] records actions, [`PathJail`] (phase 2).

pub mod audit;
mod conn;
pub mod daemon;
pub mod jail;
pub mod pty;
pub mod ring;
pub mod runner;
pub mod service;
pub mod user;

pub use audit::{Audit, FileAudit, NoAudit};
pub use jail::PathJail;
pub use pty::TermHost;
pub use runner::{PtyChild, Runner, SameUserRunner, SpawnSpec};
pub use service::{Server, ServerConfig, UserCtx, core_error};
pub use user::{SOCKET_ENV, current_user, socket_name};
