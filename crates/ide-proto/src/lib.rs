//! Wire protocol shared by `ide-server` and `ide-client`: frame codec, MessagePack RPC
//! messages, the stream multiplexer with per-stream flow control, terminal stream records and
//! an in-memory transport for tests. Transport-agnostic (`AsyncRead + AsyncWrite`).
//!
//! See `docs/server-architecture.md`.

pub mod agents;
#[doc(hidden)]
pub mod bin;
pub mod error;
pub mod files;
pub mod frame;
pub mod mux;
pub mod rpc;
pub mod term;

pub use agents::{
    AccountInfo, AccountState, ConfigurationInfo, McpServerInfo, ProviderCaps, ProviderDetection,
    ProviderInfo, SessionKind, Support,
};
pub use error::ProtoError;
pub use files::{
    AheadBehind, BrowseConfig, BrowseEntry, BrowseListing, ByteRange, ChangeKind, DefResult,
    DefTarget, DiffAgainst, DiffFile, DiffHunk, DiffLine, EntryKind, FileContent, FileMatch,
    FsEntry, FsStat, GitChange, GitRev, GitStatus, LineKind, SearchMatch, SearchSummary,
    TextEncoding, TextSearch,
};
pub use frame::{Frame, FrameError, FrameKind, MAX_FRAME_LEN, MAX_PAYLOAD};
pub use mux::{
    DEFAULT_WINDOW, Mux, MuxConfig, MuxError, Role, RpcReceiver, Stream, StreamReceiver,
    StreamSender,
};
pub use rpc::{
    Attached, Hello, NewSession, Notify, PROTO_VERSION, Request, Response, ResumeToken, RpcMessage,
    SessionInfo, SessionState, UserInfo, Welcome,
};
pub use term::{ChunkDecoder, TermChunk, encode_chunk};

/// Build version of this crate (sent in `Hello`/`Welcome`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// In-memory transport: a connected pair of byte pipes (tests, in-process servers).
pub fn memory_pair() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    tokio::io::duplex(256 * 1024)
}
