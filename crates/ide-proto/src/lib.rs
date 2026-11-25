//! Wire protocol shared by `ide-server` and `ide-client`: frame codec, MessagePack RPC
//! messages, the stream multiplexer with per-stream flow control, terminal stream records and
//! an in-memory transport for tests. Transport-agnostic (`AsyncRead + AsyncWrite`).
//!
//! See `docs/server-architecture.md`.

pub mod agents;
#[doc(hidden)]
pub mod bin;
pub mod browser;
pub mod error;
pub mod files;
pub mod frame;
pub mod git_actions;
pub mod mux;
pub mod pr;
pub mod rpc;
pub mod term;

pub use agents::{
    AccountInfo, AccountState, ConfigurationChange, ConfigurationInfo, IntegrationAction, IntegrationInfo, IntegrationState,
    McpServerInfo, ModeCaps, ProviderCaps, SkillInfo,
    ProviderDetection, ProviderInfo, AutoAnswerInfo, RepoInfo, BaseRuleInfo, BaseRulePreview, SessionKind, SessionMode, SessionRepo, Support,
};
pub use error::ProtoError;
pub use files::{
    AheadBehind, BrowseEntry, BrowseListing, ByteRange, ChangeKind, CommitInfo, DefResult,
    DefTarget, DiffAgainst, DiffFile, DiffHunk, DiffLine, EntryKind, FileContent, FileMatch, PathMatch, PathMatches,
    FsEntry, FsStat, GIT_LOG_PAGE, GIT_UNPUSHED_MAX, GitChange, GitLog, GitRev, GitStatus,
    LineKind, SearchMatch, SearchSummary, TextEncoding, TextSearch,
};
pub use git_actions::{
    BranchCheckout, BranchCommit, BranchRefreshCheck, BranchRefreshInfo, BranchRefreshOptions,
    BranchRefreshResult, CommitAction, GitBranches, LocalBranch, PullMode, PushExpected,
    RefreshConflict, RemoteBranch, RemoteFetchStatus, STASH_TAG, StashAction, StashActionResult,
    StashEntry, StatusEntry, SwitchMode, SyncStatus,
};
pub use frame::{Frame, FrameError, FrameKind, MAX_FRAME_LEN, MAX_PAYLOAD};
pub use mux::{
    DEFAULT_WINDOW, Mux, MuxConfig, MuxError, Role, RpcReceiver, Stream, StreamReceiver,
    StreamSender,
};
pub use rpc::{
    AGENT_TOOL_SESSION, AGENT_TOOL_SHOW_MESSAGE, AGENT_TOOL_WIDGET, AGENT_TOOL_PREVIEW, APPROVAL_REQUEST, ApprovalInfo, WidgetDataOp, WidgetInfo, AgentActivity, MergeResult, RepoMerge, AgentHello, AgentHookEvent, AgentTool, AgentToolResult, Attached, ChildAgent, Hello, NewSession, Notify, PROTO_VERSION, ATTACH_CHUNK, ATTACH_MAX, RepoFailureInfo, Request, Response,
    BranchBadge, BranchState, Landed, ResumeToken, RpcMessage, PortInfo, ScriptGroup, ScriptItem, SessionInfo, ShellInfo, SessionState, UserInfo, UserTerm, Welcome,
};
pub use term::{ChunkDecoder, TermChunk, encode_chunk};
// --- pull requests and agent prompts
pub use pr::{
    CommitPrPreview, CommitPrTextOptions, DriftCommit, LeftoverMatch, MERGE_MAX_WAIT_SECS,
    MERGE_POLL_SECS, MergeBlocker, MergeStep, MergeStop, PendingWork, PrCheck, PrCheckState,
    PrClosed, PrCommit, PrDrift, PrExclusions, PrGate, PrLeftover, PrLeftoverFile, PrMergeMethod,
    PrMerged, PrRecheckChange, PrResultKind, PrState, PrStatus, PrTargets, PrWorkFile,
    PullRequestResult, UNEXPLAINED_BLOCK_SECS, UnmergedWork, choose_pr_target, drift_change,
    effective_exclusions, merge_after_create_step, merge_blocker,
};

/// Build version of this crate (sent in `Hello`/`Welcome`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// In-memory transport: a connected pair of byte pipes (tests, in-process servers).
pub fn memory_pair() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    tokio::io::duplex(256 * 1024)
}
