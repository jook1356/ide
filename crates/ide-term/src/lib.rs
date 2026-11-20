//! PTY + terminal state (`TermSession`) and the iced terminal widget (`TerminalView`).
//!
//! ```ignore
//! let config = TermConfig::default();
//! let session = TermSession::spawn(&TermSpec::new("cmd.exe").cwd(dir), &config)?;
//! // subscription: session.subscription().map(Message::Term)
//! // view:         TerminalView::new(&session, &config).on_event(Message::View)
//! ```
mod config;
mod glyphs;
mod keys;
mod session;
mod view;

pub use config::{Palette, TermConfig};
pub use keys::key_bytes;
pub use session::{
    Counters, Diagnostics, EventProxy, GridSize, Injected, SessionId, TermEvent, TermEventKind, TermSession, TermSpec,
};
pub use view::{TerminalView, ViewEvent, terminal_view};

/// Re-exported so apps can inspect the grid through [`TermSession::lock_term`].
pub use alacritty_terminal;
