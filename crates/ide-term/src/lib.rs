//! Client-side terminal: the terminal state fed from a byte stream (`TermModel`) and the iced
//! terminal widget (`TerminalView`). No PTY here: the server owns the process and the PTY, answers
//! terminal queries, and streams output chunks tagged with their byte offset.
//!
//! ```ignore
//! let config = TermConfig::default();
//! let model = TermModel::new(120, 30, &config);
//! // stream chunk from the server: model.feed(chunk.offset, &chunk.bytes) (after model.reset() on a reset flag)
//! // view: TerminalView::new(&model, &config)
//! //           .on_input(move |bytes| Message::TermWrite(id, bytes))           // -> term.write
//! //           .on_resize(move |cols, rows| Message::TermResize(id, cols, rows)) // -> term.resize
//! //           .on_event(Message::View)
//! // optional redraw/title/bell notifications: model.subscription().map(Message::Term)
//! ```
mod config;
mod glyphs;
mod keys;
mod model;
mod view;

pub use config::{Palette, TermConfig};
pub use keys::key_bytes;
pub use model::{Counters, Diagnostics, EventProxy, GridSize, Injected, ModelId, TermEvent, TermEventKind, TermModel};
pub use view::{TerminalView, ViewEvent, terminal_view};

/// Re-exported so apps can inspect the grid through [`TermModel::lock_term`].
pub use alacritty_terminal;
