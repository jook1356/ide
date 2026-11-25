//! Client side of the protocol: connects to an `ide-server` through a [`Transport`], offers
//! typed async RPC methods, a merged event stream (connection state, notifications, terminal
//! output) and reconnects automatically, re-attaching terminals from the last offset the app
//! received so output is delivered exactly once.
//!
//! ```no_run
//! # async fn demo() -> Result<(), ide_proto::ProtoError> {
//! use ide_client::{Client, ClientEvent, ClientOptions, LocalChild};
//! let transport = LocalChild::find().expect("ide-server binary");
//! let client = Client::connect(transport, ClientOptions::default()).await?;
//! let mut events = client.events().expect("first call");
//! let sessions = client.sessions().await?;
//! client.attach(&sessions[0].id, None).await?;
//! client.write(&sessions[0].id, b"echo hi\r".to_vec()).await?;
//! while let Some(ev) = events.next().await {
//!     if let ClientEvent::Term(chunk) = ev { /* feed chunk.bytes to the terminal model */ }
//! }
//! # Ok(()) }
//! ```

mod client;
pub mod transport;

pub use client::{Client, ClientEvent, ClientEvents, ClientOptions, SearchEvent, SearchStream, UTermOpen};
pub use ide_proto::{
    AccountInfo, AccountState, BaseRuleInfo, ChildAgent, BaseRulePreview, ConfigurationInfo, IntegrationAction,
    IntegrationInfo, IntegrationState, McpServerInfo, ModeCaps, ProviderCaps, SkillInfo,
    ProviderDetection, ProviderInfo, AutoAnswerInfo, RepoFailureInfo, RepoInfo, SessionKind, SessionMode,
    SessionRepo, Support,
};
pub use ide_proto::{
    AheadBehind, Attached, BrowseEntry, BrowseListing, ByteRange, ChangeKind, CommitInfo,
    DefResult, DefTarget, DiffAgainst, DiffFile, DiffHunk, DiffLine, EntryKind, FileContent,
    FileMatch, FsEntry, PathMatch, PathMatches, FsStat, GIT_LOG_PAGE, GIT_UNPUSHED_MAX, GitChange, GitLog, GitRev, GitStatus, LineKind, NewSession, Notify,
    ProtoError, SearchMatch, SearchSummary, SessionInfo, SessionState, TermChunk, TextEncoding, ScriptGroup, ScriptItem, UserTerm, PortInfo, ShellInfo,
    TextSearch, UserInfo, Welcome, AGENT_TOOL_SESSION, AGENT_TOOL_SHOW_MESSAGE, AGENT_TOOL_WIDGET, AGENT_TOOL_PREVIEW, APPROVAL_REQUEST, ApprovalInfo, WidgetDataOp, WidgetInfo, AgentActivity, MergeResult, RepoMerge, BranchBadge, BranchState, Landed,
};
// --- local git actions (Changes / History footer)
pub use ide_proto::git_actions::{
    BranchCheckout, BranchCommit, BranchRefreshCheck, BranchRefreshInfo, BranchRefreshOptions,
    BranchRefreshResult, CommitAction, GitBranches, LocalBranch, PullMode, PushExpected,
    RefreshConflict, RemoteBranch, RemoteFetchStatus, STASH_TAG, StashAction, StashActionResult,
    StashEntry, StatusEntry, SwitchMode, SyncStatus,
};
// --- pull requests and agent prompts
pub use ide_proto::pr::{
    CommitPrPreview, CommitPrTextOptions, DriftCommit, LeftoverMatch, MERGE_MAX_WAIT_SECS,
    MERGE_POLL_SECS, MergeBlocker, MergeStep, MergeStop, PendingWork, PrCheck, PrCheckState,
    PrClosed, PrCommit, PrDrift, PrExclusions, PrGate, PrLeftover, PrLeftoverFile, PrMergeMethod,
    PrMerged, PrRecheckChange, PrResultKind, PrState, PrStatus, PrTargets, PrWorkFile,
    PullRequestResult, UNEXPLAINED_BLOCK_SECS, UnmergedWork, choose_pr_target, drift_change,
    effective_exclusions, merge_after_create_step, merge_blocker,
};
pub use transport::{
    AsyncIo, ConnectFuture, Connection, LocalChild, Memory, SERVER_BIN_ENV, Ssh, Switchable,
    Transport,
};

/// iced adapter (feature `iced`).
#[cfg(feature = "iced")]
pub mod iced_support {
    use std::hash::{Hash, Hasher};

    use crate::{Client, ClientEvent};

    struct Key(Client);

    impl Hash for Key {
        fn hash<H: Hasher>(&self, state: &mut H) {
            "ide-client".hash(state);
            self.0.id().hash(state);
        }
    }

    /// All events of `client` (connected/disconnected, notifications, terminal chunks) as an
    /// iced subscription. One per client; the event receiver is returned to the client when
    /// the subscription ends, so it can be re-created.
    pub fn subscription(client: &Client) -> iced::Subscription<ClientEvent> {
        iced::Subscription::run_with(Key(client.clone()), |key| {
            futures::stream::unfold(key.0.events(), |events| async move {
                let mut events = events?;
                let e = events.next().await?;
                Some((e, Some(events)))
            })
        })
    }
}
