//! RPC messages (MessagePack via `rmp-serde`, named fields) and the handshake.

use serde::{Deserialize, Serialize};

use crate::agents::{
    AccountInfo, AccountState, BaseRuleInfo, BaseRulePreview, ConfigurationInfo, ProviderInfo,
    RepoInfo, SessionKind, SessionMode, SessionRepo,
};
use crate::error::ProtoError;
use crate::files::{
    BrowseListing, DefResult, DiffAgainst, DiffFile, FileContent, FileMatch, FsEntry, PathMatches,
    FsStat, GitChange, GitLog, GitRev, GitStatus, SearchMatch, SearchSummary, TextEncoding, TextSearch,
};

/// Major protocol version. Peers with a different major version refuse each other.
pub const PROTO_VERSION: u32 = 19;
/// Largest `attach.put` piece (a frame holds at most 1 MiB).
pub const ATTACH_CHUNK: usize = 512 * 1024;
/// Largest attachment.
pub const ATTACH_MAX: u64 = 50 * 1024 * 1024;

/// The OS user a server runs as (and serves). Comes from the transport/auth step, never from
/// request fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserInfo {
    /// Login name.
    pub name: String,
    /// Stable id (`domain\name` on Windows, uid on Unix) used to address the user.
    pub id: String,
}

/// Opaque token identifying a logical client across reconnects.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResumeToken(pub String);

/// First message of a connection (client -> server, as `Request::Hello`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Must equal the server's [`PROTO_VERSION`].
    pub proto_version: u32,
    /// Client build version (informational).
    pub client_version: String,
    /// Token from a previous `Welcome` when reconnecting.
    pub resume: Option<ResumeToken>,
    /// Set by the agent bridge (`ide-server mcp`, run by a session's agent CLI): the
    /// connection acts for that session only and may send only `agent.*` requests.
    #[serde(default)]
    pub agent: Option<AgentHello>,
}

/// The session an agent bridge connection acts for, and the session's bridge token (passed
/// to the agent's MCP server by the server; see `docs/agent-bridge.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHello {
    pub session: String,
    pub token: String,
}

/// Agent bridge tool answered by the daemon: this session as JSON.
pub const AGENT_TOOL_SESSION: &str = "ide_session";
/// Agent bridge tool carried out by an app (`Notify::AgentRequest`): show `{"message"}` in the
/// session's panel.
pub const AGENT_TOOL_SHOW_MESSAGE: &str = "ide_show_message";
/// Agent bridge tool for widgets (`docs/widgets.md`): `guide` / `list` / `unregister` and
/// registrations without a question are answered by the daemon; `open` / `close` and the
/// registration question are carried out by an app (`Notify::AgentRequest`).
pub const AGENT_TOOL_WIDGET: &str = "ide_widget";
/// Not an agent tool: the daemon asks the apps for an approval (`docs/approvals.md`) as an
/// `Notify::AgentRequest` with this name; the answer is `once` / `session` / `always`, or an error
/// (refused).
pub const APPROVAL_REQUEST: &str = "ide.approval";

/// A kept approval (`approval.list`; `docs/approvals.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalInfo {
    pub id: String,
    /// What it allows (`session.steer`, `session.interrupt`, ...).
    pub capability: String,
    pub requester: String,
    pub requester_title: String,
    pub target: String,
    pub target_title: String,
    /// Kept across daemon restarts (else: until the daemon restarts).
    pub always: bool,
    pub created: u64,
}

/// A registered widget (`widget.list`; `docs/widgets.md`): every session can open it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetInfo {
    /// A URL host label (1-40 of `a-z0-9-`).
    pub id: String,
    pub name: String,
    pub description: String,
    /// ide tools it calls (allowed by the user when it was registered).
    pub tools: Vec<String>,
    /// Where its data lives: `session` / `family` / `global` (`global`: one 공용 tab).
    pub scope: String,
    pub version: u32,
    /// The session that registered it first, and its title (empty when it is gone).
    pub created_by: String,
    pub created_by_title: String,
    /// Unix seconds of the last registration.
    pub updated: u64,
    /// The sessions showing it (open tabs, remembered across restarts).
    pub open: Vec<String>,
}

/// `widget.data`: a widget's key-value store (values are JSON text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WidgetDataOp {
    Get { key: String },
    Set { key: String, value: String },
    Delete { key: String },
    Keys,
}

/// What a session's agent is doing, from its CLI's hooks (`Unknown`: no hooks, or not
/// reported yet since the process started).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentActivity {
    #[default]
    Unknown,
    /// Waiting for a prompt.
    Idle,
    /// Working on a turn (Desk's 답변 중).
    Working,
    /// Waiting for the user: a permission or a question (Desk's 응답 필요).
    NeedsInput,
}

/// A hook of a session's agent CLI, normalized (`ide-server hook`; see `docs/agent-bridge.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentHookEvent {
    /// The CLI started (or resumed / cleared) a conversation (`conversation`: the CLI's id of
    /// it, if reported; the server stores it for resuming).
    SessionStart {
        #[serde(default)]
        conversation: Option<String>,
    },
    PromptSubmitted,
    Stopped,
    NeedsInput { permission: bool },
    /// About to edit these files.
    PreEdit { tool: String, paths: Vec<String> },
    /// About to run a shell command.
    PreShell { tool: String, command: String },
    /// About to use another tool.
    PreTool { tool: String },
    /// Work that can outlive the turn started / ended (`id`: the CLI's, opaque).
    BackgroundStarted { id: String },
    BackgroundEnded { id: String },
}

/// A tool the agent bridge offers (MCP `tools/list`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTool {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments (a JSON object, as text).
    pub input_schema: String,
}

/// What an agent tool call returned (MCP `tools/call`): text, and whether it failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentToolResult {
    pub text: String,
    pub is_error: bool,
}

/// Server answer to [`Hello`] (as `Response::Welcome`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// Server protocol version.
    pub proto_version: u32,
    /// Server build version.
    pub server_version: String,
    /// The user this connection acts as.
    pub user: UserInfo,
    /// Optional features (`"term"`, `"providers"`, `"sessions"`, ...).
    pub capabilities: Vec<String>,
    /// Token to send in the next `Hello` after a reconnect.
    pub resume_token: ResumeToken,
    /// True if `Hello.resume` named a client the server still knew.
    pub resumed: bool,
    /// Random id of the server process. Terminal offsets are only meaningful within one
    /// instance: after a change, clients re-attach without `from_offset` (full replay).
    pub server_instance: String,
}

/// Process state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    /// No process (never started since the server started, or stopped).
    Stopped,
    /// Process running.
    Running,
    /// Process exited by itself (exit code if known).
    Exited(Option<u32>),
}

/// A user terminal of a session (Hermes Desk's `UserTerminal`): a shell or a command run in
/// the session's folder. Kept by the server while it runs (not across server restarts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserTerm {
    /// `<session id>:t<n>`; also the `session` of its `term.*` requests and notifications.
    pub id: String,
    pub session: String,
    /// `<shell> <n>` or the given title.
    pub title: String,
    /// The shell or the command line.
    pub command: String,
    /// Working directory (server path).
    pub cwd: String,
    pub state: SessionState,
    /// End offset of its output (to attach).
    pub end: u64,
}

/// A shell a user terminal can run (`uterm.shells`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellInfo {
    /// Stable id (`pwsh`, `cmd`, `git-bash`, `zsh`, ...).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Its program (server path).
    pub path: String,
}

/// A TCP port listening in a terminal's process tree (`uterm.ports`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortInfo {
    pub port: u16,
    pub pid: u32,
    /// The listening process' name.
    pub process: String,
}

/// Scripts of one file (Desk's `ScriptGroup`): `package.json`, a Makefile, `Cargo.toml`, ...
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptGroup {
    /// Root-relative path of the file (also its id).
    pub file: String,
    /// Its folder (root-relative, empty = the root): where the scripts run.
    pub folder: String,
    /// The ecosystem (`npm`, `make`, `cargo`, ...).
    pub kind: String,
    pub scripts: Vec<ScriptItem>,
}

/// One runnable script: its name, the command line that runs it, and what it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptItem {
    pub name: String,
    pub run: String,
    pub detail: Option<String>,
}

/// A session as seen by clients. Paths are server paths (strings, the server's OS syntax).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Session id.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Agent session or login terminal.
    pub kind: SessionKind,
    /// Provider id.
    pub provider: String,
    /// Account slug (of `provider`), if any.
    pub account: Option<String>,
    /// Configuration slug (empty for login sessions).
    pub configuration: String,
    /// Workspace or original-repo session.
    pub mode: SessionMode,
    /// The agent's cwd: the session folder (workspace), the repository (original repo) or a
    /// login session's empty dir.
    pub root: String,
    /// Attached repositories (worktrees), in attach order.
    pub repos: Vec<SessionRepo>,
    /// Unix seconds.
    pub created_at: u64,
    /// Unix seconds.
    pub last_active: u64,
    /// Process state.
    pub state: SessionState,
    /// Last terminal title (OSC 0/2), empty if none.
    pub term_title: String,
    /// End offset of the terminal output (bytes produced so far).
    pub term_end: u64,
    /// Pinned to the top of its group in the session list.
    #[serde(default)]
    pub pinned: bool,
    /// In its group's 보관함 (archived).
    #[serde(default)]
    pub archived: bool,
    /// What its agent is doing (hooks).
    #[serde(default)]
    pub activity: AgentActivity,
    /// Its parent session (`docs/session-family.md`).
    #[serde(default)]
    pub parent: Option<String>,
    /// A child working in its parent's folder and worktrees.
    #[serde(default)]
    pub shared: bool,
    /// Holds the write access of a folder other sessions share (`docs/session-family.md`).
    #[serde(default)]
    pub writer: bool,
}

/// What merging one repo of a session family did (`session.merge_child` / `sync_parent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoMerge {
    pub repo: String,
    pub name: String,
    pub result: MergeResult,
}

/// The result of one merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeResult {
    Merged,
    UpToDate,
    /// Stopped at conflicts in these files (left in progress to resolve).
    Conflicts { files: Vec<String> },
    /// Refused / failed; nothing changed.
    Failed { error: String },
}

/// `session.create` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSession {
    /// Provider id (see `provider.list`).
    pub provider: String,
    /// Account slug of that provider (`None` = the CLI's default login / no account).
    pub account: Option<String>,
    /// Configuration slug.
    pub configuration: String,
    /// Registered repos to give worktrees (workspace mode; empty = an empty workspace).
    pub repos: Vec<String>,
    /// Original-repo mode: run in this registered repo's checkout (no workspace).
    pub original_repo: Option<String>,
    /// 기준 브랜치 최신화: fetch each base from its remote before creating the worktree (a
    /// failed fetch fails that repo).
    #[serde(default)]
    pub refresh_base: bool,
    /// Base picked per repo id (one of origin's branches); missing = the repo's default.
    #[serde(default)]
    pub bases: std::collections::BTreeMap<String, String>,
    /// Title; empty = derived.
    pub title: String,
    /// Start the process right away.
    pub start: bool,
    /// Initial terminal size.
    pub cols: u16,
    /// Initial terminal size.
    pub rows: u16,
}

/// A repository whose worktree could not be created (`session.create` still succeeds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoFailureInfo {
    /// Repo id.
    pub repo: String,
    /// Repo name.
    pub name: String,
    /// Why.
    pub error: ProtoError,
}

/// Result of `term.attach`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attached {
    /// Session id.
    pub session: String,
    /// Data stream (server-opened) carrying [`crate::TermChunk`] records.
    pub stream: u32,
    /// Offset the replay starts at.
    pub start_offset: u64,
    /// True if `from_offset` was no longer in the ring: the client must clear its model.
    pub reset: bool,
}

/// Client -> server request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum Request {
    Hello(Hello),
    Ping,
    /// Agent providers with detection and capabilities (`refresh` = re-run detection).
    ProviderList {
        refresh: bool,
    },
    AccountList,
    /// Create an account (slug derived from `name`) with an empty config dir.
    AccountCreate {
        provider: String,
        name: String,
    },
    /// Change an account's display name.
    AccountRename {
        provider: String,
        slug: String,
        name: String,
    },
    /// Remove an account (`account-in-use` if sessions use it). `remove_files` deletes its
    /// config dir (credentials) when it lies inside the data dir.
    AccountDelete {
        provider: String,
        slug: String,
        remove_files: bool,
    },
    /// Ask the CLI whether the account is logged in (slow: runs the CLI).
    AccountStatus {
        provider: String,
        slug: String,
    },
    /// Start the provider's login command for the account in a new login session (a
    /// terminal without repository; delete it when done). Returns the session.
    AccountLogin {
        provider: String,
        slug: String,
        cols: u16,
        rows: u16,
    },
    ConfigurationList,
    /// Create an empty configuration (slug derived from `name`).
    ConfigurationCreate {
        name: String,
    },
    /// Replace name, MCP servers, instructions and default repos of the configuration `slug`
    /// (its `skills` are ignored: they change with the `skill.*` requests).
    ConfigurationUpdate(ConfigurationInfo),
    /// Delete a configuration (`configuration-in-use`, `last-configuration`).
    ConfigurationDelete {
        slug: String,
    },
    /// Skills a configuration owns (`<data>/configurations/<slug>/skills/<name>/`). Each
    /// change answers with the configuration (`Configuration`).
    /// A new skill: a `SKILL.md` with `name` (made a folder name) and `description`.
    SkillCreate {
        configuration: String,
        name: String,
        description: String,
    },
    /// Copy a skill folder (server path, with a `SKILL.md`) into the configuration.
    SkillImport {
        configuration: String,
        path: String,
    },
    /// The `SKILL.md` text (`SkillText`).
    SkillRead {
        configuration: String,
        name: String,
    },
    /// Replace the `SKILL.md` text.
    SkillWrite {
        configuration: String,
        name: String,
        text: String,
    },
    SkillDelete {
        configuration: String,
        name: String,
    },
    /// Offer the skill to new sessions or not.
    SkillEnable {
        configuration: String,
        name: String,
        enabled: bool,
    },
    /// Registered repositories.
    RepoList,
    /// Register the git repository containing `path` (absolute server path). A plain folder
    /// fails with `not-a-git-folder` unless `init` (then `git init` + first commit). An already
    /// registered repository is returned as is.
    RepoRegister {
        path: String,
        #[serde(default)]
        init: bool,
    },
    /// Fixed base branch of a repo (full ref from `repo.branches`; `None` = automatic).
    RepoSetBaseBranch {
        id: String,
        base: Option<String>,
    },
    /// Save (checked against the remote) or remove (`None`) a repo's base branch rule.
    RepoSetBaseRule {
        id: String,
        rule: Option<BaseRuleInfo>,
    },
    /// What a rule would pick right now (`git ls-remote`); saves nothing.
    RepoPreviewBaseRule {
        id: String,
        rule: BaseRuleInfo,
    },
    /// Local and remote-tracking branch refs of a repo (fixed-base picker).
    RepoBranches {
        id: String,
    },
    /// origin's branches of a repo as of its last fetch (base pickers).
    RepoOriginBranches {
        id: String,
    },
    /// A session's own base branch for one repo (`None` = the repo's default).
    SessionSetBase {
        id: String,
        repo: String,
        base: Option<String>,
    },
    /// Unregister (files kept); `repo-in-use` while sessions have it attached.
    RepoUnregister {
        id: String,
    },
    SessionList,
    SessionCreate(NewSession),
    /// Give a workspace session a worktree of a registered repo (no-op if attached).
    SessionAttachRepo {
        id: String,
        repo: String,
        /// Base branch for the new worktree (origin's branch; `None` = the repo's default).
        #[serde(default)]
        base: Option<String>,
    },
    /// Remove a repo's worktree from a workspace session (uncommitted changes are discarded;
    /// the branch is kept).
    SessionDetachRepo {
        id: String,
        repo: String,
    },
    SessionStart {
        id: String,
    },
    SessionStop {
        id: String,
    },
    SessionRestart {
        id: String,
    },
    /// A user terminal in the session's folder (Hermes Desk's `term:openShell` / `term:run`):
    /// a shell in `repo`'s worktree (`None` = the session folder), or `command` run by that
    /// shell. Its output, input and size use `term.*` with the terminal's id.
    UTermOpen {
        session: String,
        repo: Option<String>,
        /// Folder inside `repo`'s worktree to start in (a script's folder).
        folder: Option<String>,
        command: Option<String>,
        title: Option<String>,
        /// Shell id of `uterm.shells` (`None` = the server's default).
        shell: Option<String>,
        /// 공용 terminals (empty `session`): the folder to start in (server path; `None` =
        /// the home folder).
        cwd: Option<String>,
    },
    /// Runnable scripts of `repo`'s worktree (Desk's `scripts:list`): one group per file that
    /// names tasks (package.json, Makefile, ...) or per build tool with well-known commands.
    ScriptsList {
        session: String,
        repo: String,
    },
    /// The shells `uterm.open` can start (first = the default).
    UTermShells,
    /// TCP ports listening in the session's terminals' process trees (its agent and user
    /// terminals; empty `session`: the 공용 terminals): Desk's 실행 중인 서버.
    UTermPorts {
        session: String,
    },
    /// The session's user terminals, oldest first.
    UTermList {
        session: String,
    },
    UTermStop {
        id: String,
    },
    /// Run an exited terminal's command again (same id, output appended).
    UTermRerun {
        id: String,
    },
    /// Stop (if running) and forget a terminal.
    UTermRemove {
        id: String,
    },
    SessionDelete {
        id: String,
        remove_worktree: bool,
    },
    SessionRename {
        id: String,
        title: String,
    },
    /// Pin / unpin and archive / unarchive (`None` = unchanged); archiving unpins, an archived
    /// session cannot be pinned. Answers `Session`.
    SessionSetFlags {
        id: String,
        pinned: Option<bool>,
        archived: Option<bool>,
    },
    /// A child of `parent` (`SessionCreated`): `shared` = in the parent's folder and worktrees,
    /// else own worktrees of `repos` (the parent's; empty = all) branched from the parent's.
    /// The parent's agent (provider, account, configuration).
    SessionCreateChild {
        parent: String,
        shared: bool,
        repos: Vec<String>,
        title: String,
        start: bool,
        cols: u16,
        rows: u16,
    },
    /// Merge child `id`'s branches into its parent's worktrees (`Merges`).
    SessionMergeChild {
        id: String,
        squash: bool,
    },
    /// Merge the parent's branches into child `id`'s worktrees (`Merges`).
    SessionSyncParent {
        id: String,
    },
    /// An account's integration (`Integration`): its state, install or uninstall (only the
    /// user asks for it: the accounts page).
    AccountIntegration {
        provider: String,
        slug: String,
        action: crate::IntegrationAction,
    },
    /// Agent bridge connections only: the tools of the bridge (`AgentTools`).
    AgentTools,
    /// Agent bridge connections only: call a tool with JSON arguments (`AgentResult`).
    AgentCall {
        tool: String,
        args: String,
    },
    /// Agent bridge connections only: a hook of the agent CLI (`AgentHookDecision`).
    AgentHook {
        event: AgentHookEvent,
    },
    /// App connections: the answer to a `Notify::AgentRequest` (`Ok`: the tool's text).
    AgentReply {
        request: u64,
        result: Result<String, String>,
    },
    /// Open (or replace) this connection's terminal stream for `session`.
    TermAttach {
        session: String,
        from_offset: Option<u64>,
    },
    TermDetach {
        session: String,
    },
    TermWrite {
        session: String,
        #[serde(with = "crate::bin")]
        bytes: Vec<u8>,
    },
    /// A file for an agent's prompt (an input-box attachment: a screenshot, or a file the
    /// client has but the server's machine has not), stored under the session's data folder
    /// (`<data>/sessions/<id>/attachments/`). Sent in pieces of at most [`ATTACH_CHUNK`]:
    /// the first (`upload: None`) creates the file (`name` made a safe, unique file name), the
    /// next ones append to it (`upload` = the stored name). Answers `AttachPath`.
    AttachPut {
        session: String,
        name: String,
        upload: Option<String>,
        #[serde(with = "crate::bin")]
        data: Vec<u8>,
    },
    TermResize {
        session: String,
        cols: u16,
        rows: u16,
    },
    /// Stop every session and exit the server process (daemon or `--no-daemon`).
    ServerShutdown,

    // ---- Phase 2: files (paths root-relative to the worktree of `repo` in `session`) ----
    /// List a directory (`dir` = `""` for the root). `.git` is never listed; ignored entries
    /// only with `include_ignored`.
    FsList {
        session: String,
        repo: String,
        dir: String,
        include_ignored: bool,
    },
    FsStat {
        session: String,
        repo: String,
        path: String,
    },
    /// Read a file (`max_bytes` 0 = default 2 MiB).
    FsRead {
        session: String,
        repo: String,
        path: String,
        max_bytes: u64,
    },
    /// Write a whole file. `expected_version` = the `FileContent::version` it was read at
    /// (`fs-conflict` if the file changed on disk since); `None` = overwrite unconditionally.
    /// `encoding` `None` = UTF-8 without BOM.
    FsWrite {
        session: String,
        repo: String,
        path: String,
        content: String,
        expected_version: Option<String>,
        encoding: Option<TextEncoding>,
    },
    /// Create a new file (`fs-exists` if it exists); parent folders are created.
    FsCreateFile {
        session: String,
        repo: String,
        path: String,
        content: String,
    },
    /// Create a folder (and missing parents); `fs-exists` if it exists.
    FsCreateDir {
        session: String,
        repo: String,
        path: String,
    },
    /// Rename/move inside the session (`fs-exists` if `to` exists).
    FsRename {
        session: String,
        repo: String,
        from: String,
        to: String,
    },
    /// Delete a file, symlink or folder (non-empty folders need `recursive`); with `trash`, to
    /// the recycle bin / trash instead (Explore's 삭제).
    FsDelete {
        session: String,
        repo: String,
        path: String,
        recursive: bool,
        #[serde(default)]
        trash: bool,
    },
    /// Copy `from` (root-relative, or with `external` an absolute path of the server's machine)
    /// into the folder `to_dir` (`""` = the root); a taken name gets ` copy` (VS Code).
    /// Answers `FsCopied` (the new root-relative path).
    FsCopy {
        session: String,
        repo: String,
        from: String,
        #[serde(default)]
        external: bool,
        to_dir: String,
    },
    /// The web address of `path` (root-relative) on the repo's `origin` at its current branch
    /// (Explore's 원격 파일 URL 복사); `git-no-origin` without one. Answers `Url`.
    GitRemoteUrl {
        session: String,
        repo: String,
        path: String,
    },
    /// Start sending `Notify::FsChanged`/`GitChanged` for the session to this connection.
    FsWatch {
        session: String,
        repo: String,
    },
    FsUnwatch {
        session: String,
        repo: String,
    },
    /// Folders anywhere the server's OS user can read, for choosing a repository to register. `None` = the start list: the drives (Windows) or
    /// `/` (Unix) plus the home dir, with the listing path `""`. Directories only, hidden and
    /// system folders skipped, sorted, capped ([`BrowseListing::truncated`]).
    BrowsePickList {
        abs_dir: Option<String>,
    },
    /// Start a text search; `search` is a client-chosen id echoed in `Notify::SearchHits` /
    /// `Notify::SearchDone` (sent to this connection only, after the `Unit` reply).
    SearchText {
        search: u64,
        query: TextSearch,
    },
    /// Cancel a running search of this connection (no-op if finished).
    SearchCancel {
        search: u64,
    },
    /// Fuzzy file-name search over non-ignored files (`max_results` 0 = 100).
    SearchFiles {
        session: String,
        /// `None` = every repo of the session.
        repo: Option<String>,
        query: String,
        max_results: u32,
    },
    /// Hermes Desk's Explore 파일 찾기: files and folders whose root-relative path contains
    /// `query` (case-insensitive), in path order; `include_ignored` also lists gitignored
    /// files and ignored folders (once each, not their contents).
    SearchPaths {
        session: String,
        /// `None` = every repo of the session.
        repo: Option<String>,
        query: String,
        max_results: u32,
        include_ignored: bool,
    },
    /// `base` = branch to compare with (`None` = the branch checked out in the main repo).
    GitStatus {
        session: String,
        repo: String,
        base: Option<String>,
    },
    /// `path` `None` = every changed tracked file.
    GitDiff {
        session: String,
        repo: String,
        path: Option<String>,
        against: DiffAgainst,
        base: Option<String>,
    },
    /// File content at a revision; `Response::FileAt(None)` if it does not exist there.
    GitFileAt {
        session: String,
        repo: String,
        path: String,
        rev: GitRev,
        base: Option<String>,
    },
    /// A page of HEAD's history, newest first (`git log HEAD`): skip `skip` commits, return at
    /// most `limit` (0 = [`crate::GIT_LOG_PAGE`]). No commit yet = an empty page.
    GitLog {
        session: String,
        repo: String,
        skip: u32,
        limit: u32,
    },
    /// Full ids of the History tab's not-yet-pushed commits (Hermes Desk's ↑ marks): with an
    /// upstream `upstream..HEAD`, with a gone upstream none, without one the commits no
    /// remote-tracking branch has; at most [`crate::GIT_UNPUSHED_MAX`]. Empty without a remote.
    GitUnpushed {
        session: String,
        repo: String,
    },
    /// Files a commit changed against its first parent (a root commit: against the empty
    /// tree), renames detected. `sha`: hex commit id (4-64 digits), else `git-bad-rev`.
    GitCommitFiles {
        session: String,
        repo: String,
        sha: String,
    },
    /// Diff of a commit against its first parent (root commit: the empty tree); `path` `None`
    /// = every file, else that file (a renamed file's old path matches too). Same
    /// `Response::Diff` as `git.diff`.
    GitCommitDiff {
        session: String,
        repo: String,
        sha: String,
        path: Option<String>,
    },
    // --- local git actions (Changes / History footer)
    /// Discard one file's uncommitted change (Desk `changes:discard`): `Untracked` is deleted,
    /// `Added`/`Copied` removed from index and tree, `Renamed` restored to `orig_path` (and the
    /// new path removed), anything else restored from HEAD (index and worktree).
    GitDiscard {
        session: String,
        repo: String,
        file: GitChange,
    },
    /// Every stash of the repository, newest first (Desk `git:listStashes`).
    GitStashList {
        session: String,
        repo: String,
    },
    /// Stash the changes (untracked included): all, or only `paths` (literal, both sides of a
    /// rename). Nothing to stash = `git-stash-empty` (Desk `git:stashPush`).
    GitStashPush {
        session: String,
        repo: String,
        message: String,
        paths: Option<Vec<String>>,
    },
    /// Pop / apply / drop the stash with commit id `sha` (Desk `git:stashAction`).
    GitStashAction {
        session: String,
        repo: String,
        sha: String,
        action: crate::StashAction,
    },
    /// History context menu action on commit `sha` (full id); needs a clean tree and no
    /// merge/rebase in progress (Desk `git:commitAction`).
    GitCommitAction {
        session: String,
        repo: String,
        sha: String,
        action: crate::CommitAction,
    },
    /// Sync button numbers; `fetch` first fetches the upstream (failures ignored).
    /// `merged_head`: head commit of the branch's merged PR, if known (Desk `github:syncStatus`).
    GitSyncStatus {
        session: String,
        repo: String,
        fetch: bool,
        merged_head: Option<String>,
    },
    /// Push the checked-out branch to origin (`-u`; `force` = `--force-with-lease`); with
    /// `expected` push exactly that verified commit (Desk `github:push`). Returns the branch.
    GitPush {
        session: String,
        repo: String,
        force: bool,
        expected: Option<crate::PushExpected>,
    },
    /// Pull the upstream into the checked-out branch (Desk `github:pull`). Returns the branch.
    GitPull {
        session: String,
        repo: String,
        mode: crate::PullMode,
    },
    /// Local and remote-tracking branches with their checkout holders (Desk `git:branches`,
    /// `git:remoteTrackingBranches`, `git:branchBases`); local refs only.
    GitBranches {
        session: String,
        repo: String,
    },
    /// `git fetch --prune` every remote, throttled per repository (Desk `git:fetchRemotes`).
    GitFetchRemotes {
        session: String,
        repo: String,
    },
    /// Switch the worktree to local `branch`; with `remote` to that remote's branch (fetched
    /// fresh, created locally tracking it) unless a local one exists (Desk `git:switch`,
    /// `git:switchRemote`).
    GitSwitchBranch {
        session: String,
        repo: String,
        branch: String,
        remote: Option<String>,
        mode: crate::SwitchMode,
    },
    /// Create branch `name` from `base` (a local branch or `remote/branch`, no tracking) and
    /// switch to it (Desk `git:createBranch`).
    GitCreateBranch {
        session: String,
        repo: String,
        name: String,
        base: String,
        mode: crate::SwitchMode,
    },
    /// 새 브랜치로 이어가기 dialog data against the session's base; `fetch` = the dialog
    /// opening: fetch `origin/<base>` first (Desk `git:branchRefreshInfo`).
    GitBranchRefreshInfo {
        session: String,
        repo: String,
        fetch: bool,
    },
    /// Dry run of the picked commits (Desk `git:branchRefreshCheck`).
    GitBranchRefreshCheck {
        session: String,
        repo: String,
        commits: Vec<String>,
        move_files: bool,
    },
    /// Continue on a new branch from the fresh `origin/<base>` with the picked commits; the
    /// session worktree's branch becomes the new one (Desk `git:refreshBranch`).
    GitRefreshBranch {
        session: String,
        repo: String,
        options: crate::BranchRefreshOptions,
    },
    /// Go to definition at `line`/`col` (1-based; `col` in chars) of `path`. `text` = the
    /// editor's current (possibly unsaved) content; `None` = read the file.
    DefLookup {
        session: String,
        repo: String,
        path: String,
        line: u32,
        col: u32,
        text: Option<String>,
    },
    // --- pull requests and agent prompts
    // GitHub through the `gh` CLI (`gh api`); every request names `(session, repo)` and runs in
    // that worktree. See docs/changes-actions.md, "Pull requests and agent prompts".
    /// The checked-out branch's PR (open first, else the latest closed/merged one); `None` =
    /// none (Desk `github:prStatus`).
    PrStatus {
        session: String,
        repo: String,
    },
    /// Review decision, checks and merge state of PR `number` (Desk `github:prGate`).
    PrGate {
        session: String,
        repo: String,
        number: u64,
    },
    /// Read-only lookup of the branch's open PR (`kind: Exists`), with `session_base` when its
    /// target is not the session's base; `None` = no open PR (Desk `github:findOpenPr`).
    PrFindOpen {
        session: String,
        repo: String,
    },
    /// origin's branches (`git ls-remote`), its default and the current branch: the PR
    /// dialog's target list (Desk `github:remoteBranches`).
    PrRemoteBranches {
        session: String,
        repo: String,
    },
    /// Unpushed commits and uncommitted files, asked before the PR dialog (Desk
    /// `github:pendingWork`).
    PrPendingWork {
        session: String,
        repo: String,
    },
    /// The PR text left behind and what the branch has for `base` (Desk `github:prLeftover`).
    PrLeftover {
        session: String,
        repo: String,
        base: Option<String>,
    },
    /// Remove a stale PR file (+ its meta) and create its folder; returns the PR file's path
    /// (Desk `github:prFilePath` + `github:resetPrFile`). Workspace sessions only.
    PrResetFile {
        session: String,
        repo: String,
    },
    /// HEAD, uncommitted files and the commits since `since` (Desk `github:prDrift`).
    PrDrift {
        session: String,
        repo: String,
        since: Option<String>,
        allowed: Vec<String>,
    },
    /// Work the session's base does not have yet ("커밋 보유", Desk `github:unmerged`);
    /// `None` = cannot tell (no `origin/<base>`).
    PrUnmerged {
        session: String,
        repo: String,
    },
    /// Read-only PR text from committed history against `origin/<base>` (fetches the base;
    /// `push` false = the fetched `origin/<branch>` instead of HEAD) (Desk
    /// `github:commitPrPreview`).
    PrCommitPreview {
        session: String,
        repo: String,
        base: String,
        push: bool,
    },
    /// Write the PR file from the commits (pins checked); returns the title (Desk
    /// `github:commitPrText`).
    PrCommitText {
        session: String,
        repo: String,
        base: String,
        options: crate::CommitPrTextOptions,
    },
    /// Push (unless `push` false) and open the PR into `base` from the PR file (Desk
    /// `github:createPr`). `push_sha`: push that commit of the branch instead of HEAD;
    /// `options`: a commit-based request's pins, revalidated.
    PrCreate {
        session: String,
        repo: String,
        base: String,
        draft: bool,
        push: bool,
        push_sha: Option<String>,
        options: Option<crate::CommitPrTextOptions>,
    },
    /// Merge PR `number` only if its head is still `sha` (Desk `github:mergePr`).
    PrMerge {
        session: String,
        repo: String,
        number: u64,
        sha: String,
        method: crate::PrMergeMethod,
        delete_branch: bool,
    },
    /// Close open PR `number` without merging (Desk `github:closePr`).
    PrClose {
        session: String,
        repo: String,
        number: u64,
    },
    /// GitHub's "Update branch" for PR `number`, expecting head `sha` (Desk
    /// `github:updatePrBranch`).
    PrUpdateBranch {
        session: String,
        repo: String,
        number: u64,
        sha: String,
    },
    /// Desk's commit prompt ("커밋 맡기기") for `files` (`all` = every change).
    PromptCommit {
        session: String,
        repo: String,
        files: Vec<String>,
        all: bool,
    },
    /// Desk's PR prompt for target `target` (the PR file path included).
    PromptPr {
        session: String,
        repo: String,
        target: String,
        exclude: crate::PrExclusions,
    },
    /// Submit `text` to the session's agent the way its provider takes a prompt (bracketed
    /// paste + Enter for TUIs, typed lines for shells); `prompt-not-ready` when the TUI is not
    /// at its input yet.
    SessionSendPrompt {
        session: String,
        text: String,
    },
    /// The registered widgets (protocol 17, `docs/widgets.md`) -> `Widgets`.
    WidgetList,
    /// A file of a registered widget (`path` empty = its entry page) -> `WidgetFile`.
    WidgetFile {
        session: String,
        widget: String,
        path: String,
    },
    /// The user allowed registering the session's draft `widget` with `tools` (as shown to
    /// them); refused when the draft now lists others (`widget-changed`) -> `Unit`.
    WidgetRegister {
        session: String,
        widget: String,
        tools: Vec<String>,
    },
    /// The user deleted a registered widget -> `Unit`.
    WidgetDelete {
        widget: String,
    },
    /// A widget tab was opened / closed in the session (remembered across restarts) -> `Unit`.
    WidgetSetOpen {
        session: String,
        widget: String,
        open: bool,
    },
    /// Turn an agent's auto answer on or off (protocol 19, `docs/auto-answers.md`) -> `Unit`.
    ProviderSetAutoAnswer {
        provider: String,
        answer: String,
        enabled: bool,
    },
    /// The kept approvals (protocol 18, `docs/approvals.md`) -> `Approvals`.
    ApprovalList,
    /// Remove a kept approval -> `Unit`.
    ApprovalRevoke {
        id: String,
    },
    /// A widget calls an ide tool it declared and the user approved -> `AgentResult`.
    WidgetCall {
        session: String,
        widget: String,
        tool: String,
        args: String,
    },
    /// A widget's data in its scope -> `WidgetData`.
    WidgetData {
        session: String,
        widget: String,
        op: WidgetDataOp,
    },
}

impl Request {
    /// Method name for logs and audit.
    pub fn method(&self) -> &'static str {
        match self {
            Request::Hello(_) => "hello",
            Request::Ping => "ping",
            Request::ProviderList { .. } => "provider.list",
            Request::AccountList => "account.list",
            Request::AccountCreate { .. } => "account.create",
            Request::AccountRename { .. } => "account.rename",
            Request::AccountDelete { .. } => "account.delete",
            Request::AccountStatus { .. } => "account.status",
            Request::AccountLogin { .. } => "account.login",
            Request::ConfigurationList => "configuration.list",
            Request::ConfigurationCreate { .. } => "configuration.create",
            Request::ConfigurationUpdate(_) => "configuration.update",
            Request::ConfigurationDelete { .. } => "configuration.delete",
            Request::SkillCreate { .. } => "skill.create",
            Request::SkillImport { .. } => "skill.import",
            Request::SkillRead { .. } => "skill.read",
            Request::SkillWrite { .. } => "skill.write",
            Request::SkillDelete { .. } => "skill.delete",
            Request::SkillEnable { .. } => "skill.enable",
            Request::RepoList => "repo.list",
            Request::RepoRegister { .. } => "repo.register",
            Request::RepoUnregister { .. } => "repo.unregister",
            Request::RepoSetBaseBranch { .. } => "repo.set_base_branch",
            Request::RepoSetBaseRule { .. } => "repo.set_base_rule",
            Request::RepoPreviewBaseRule { .. } => "repo.preview_base_rule",
            Request::RepoBranches { .. } => "repo.branches",
            Request::RepoOriginBranches { .. } => "repo.origin_branches",
            Request::SessionSetBase { .. } => "session.set_base",
            Request::SessionList => "session.list",
            Request::SessionCreate(_) => "session.create",
            Request::SessionAttachRepo { .. } => "session.attach_repo",
            Request::SessionDetachRepo { .. } => "session.detach_repo",
            Request::SessionStart { .. } => "session.start",
            Request::SessionStop { .. } => "session.stop",
            Request::SessionRestart { .. } => "session.restart",
            Request::SessionDelete { .. } => "session.delete",
            Request::UTermOpen { .. } => "uterm.open",
            Request::UTermShells => "uterm.shells",
            Request::UTermPorts { .. } => "uterm.ports",
            Request::ScriptsList { .. } => "scripts.list",
            Request::UTermList { .. } => "uterm.list",
            Request::UTermStop { .. } => "uterm.stop",
            Request::UTermRerun { .. } => "uterm.rerun",
            Request::UTermRemove { .. } => "uterm.remove",
            Request::SessionRename { .. } => "session.rename",
            Request::SessionSetFlags { .. } => "session.set_flags",
            Request::SessionCreateChild { .. } => "session.create_child",
            Request::SessionMergeChild { .. } => "session.merge_child",
            Request::SessionSyncParent { .. } => "session.sync_parent",
            Request::AccountIntegration { .. } => "account.integration",
            Request::AgentTools => "agent.tools",
            Request::AgentCall { .. } => "agent.call",
            Request::AgentReply { .. } => "agent.reply",
            Request::AgentHook { .. } => "agent.hook",
            Request::TermAttach { .. } => "term.attach",
            Request::TermDetach { .. } => "term.detach",
            Request::TermWrite { .. } => "term.write",
            Request::AttachPut { .. } => "attach.put",
            Request::TermResize { .. } => "term.resize",
            Request::ServerShutdown => "server.shutdown",
            Request::FsList { .. } => "fs.list",
            Request::FsStat { .. } => "fs.stat",
            Request::FsRead { .. } => "fs.read",
            Request::FsWrite { .. } => "fs.write",
            Request::FsCreateFile { .. } => "fs.create_file",
            Request::FsCreateDir { .. } => "fs.create_dir",
            Request::FsRename { .. } => "fs.rename",
            Request::FsDelete { .. } => "fs.delete",
            Request::FsCopy { .. } => "fs.copy",
            Request::GitRemoteUrl { .. } => "git.remote_url",
            Request::FsWatch { .. } => "fs.watch",
            Request::FsUnwatch { .. } => "fs.unwatch",
            Request::BrowsePickList { .. } => "browse.pick_list",
            Request::SearchText { .. } => "search.text",
            Request::SearchCancel { .. } => "search.cancel",
            Request::SearchFiles { .. } => "search.files",
            Request::SearchPaths { .. } => "search.paths",
            Request::GitStatus { .. } => "git.status",
            Request::GitDiff { .. } => "git.diff",
            Request::GitFileAt { .. } => "git.file_at",
            Request::GitLog { .. } => "git.log",
            Request::GitUnpushed { .. } => "git.unpushed",
            Request::GitCommitFiles { .. } => "git.commit_files",
            Request::GitCommitDiff { .. } => "git.commit_diff",
            // --- local git actions (Changes / History footer)
            Request::GitDiscard { .. } => "git.discard",
            Request::GitStashList { .. } => "git.stash_list",
            Request::GitStashPush { .. } => "git.stash_push",
            Request::GitStashAction { .. } => "git.stash_action",
            Request::GitCommitAction { .. } => "git.commit_action",
            Request::GitSyncStatus { .. } => "git.sync_status",
            Request::GitPush { .. } => "git.push",
            Request::GitPull { .. } => "git.pull",
            Request::GitBranches { .. } => "git.branches",
            Request::GitFetchRemotes { .. } => "git.fetch_remotes",
            Request::GitSwitchBranch { .. } => "git.switch_branch",
            Request::GitCreateBranch { .. } => "git.create_branch",
            Request::GitBranchRefreshInfo { .. } => "git.branch_refresh_info",
            Request::GitBranchRefreshCheck { .. } => "git.branch_refresh_check",
            Request::GitRefreshBranch { .. } => "git.refresh_branch",
            Request::DefLookup { .. } => "def.lookup",
            // --- pull requests and agent prompts
            Request::PrStatus { .. } => "pr.status",
            Request::PrGate { .. } => "pr.gate",
            Request::PrFindOpen { .. } => "pr.find_open",
            Request::PrRemoteBranches { .. } => "pr.remote_branches",
            Request::PrPendingWork { .. } => "pr.pending_work",
            Request::PrLeftover { .. } => "pr.leftover",
            Request::PrResetFile { .. } => "pr.reset_file",
            Request::PrDrift { .. } => "pr.drift",
            Request::PrUnmerged { .. } => "pr.unmerged",
            Request::PrCommitPreview { .. } => "pr.commit_preview",
            Request::PrCommitText { .. } => "pr.commit_text",
            Request::PrCreate { .. } => "pr.create",
            Request::PrMerge { .. } => "pr.merge",
            Request::PrClose { .. } => "pr.close",
            Request::PrUpdateBranch { .. } => "pr.update_branch",
            Request::PromptCommit { .. } => "prompt.commit",
            Request::PromptPr { .. } => "prompt.pr",
            Request::SessionSendPrompt { .. } => "session.send_prompt",
            Request::WidgetList => "widget.list",
            Request::WidgetFile { .. } => "widget.file",
            Request::WidgetRegister { .. } => "widget.register",
            Request::WidgetDelete { .. } => "widget.delete",
            Request::WidgetSetOpen { .. } => "widget.set_open",
            Request::ProviderSetAutoAnswer { .. } => "provider.set_auto_answer",
            Request::ApprovalList => "approval.list",
            Request::ApprovalRevoke { .. } => "approval.revoke",
            Request::WidgetCall { .. } => "widget.call",
            Request::WidgetData { .. } => "widget.data",
        }
    }
}

/// Successful response payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum Response {
    Welcome(Welcome),
    Pong,
    Unit,
    Providers(Vec<ProviderInfo>),
    Accounts(Vec<AccountInfo>),
    Account(AccountInfo),
    AccountState(AccountState),
    Configurations(Vec<ConfigurationInfo>),
    Configuration(ConfigurationInfo),
    /// Where an attachment is stored on the server (`attach.put`): its file name and
    /// absolute path.
    AttachPath {
        name: String,
        path: String,
    },
    /// Where `fs.copy` put the copy (root-relative).
    FsCopied(String),
    /// A web address (`git.remote_url`).
    Url(String),
    /// A skill's `SKILL.md`.
    SkillText(String),
    Sessions(Vec<SessionInfo>),
    UTerm(UserTerm),
    UTerms(Vec<UserTerm>),
    Scripts(Vec<ScriptGroup>),
    Shells(Vec<ShellInfo>),
    Ports(Vec<PortInfo>),
    Session(SessionInfo),
    /// `session.create`: the session and the repos whose worktree failed.
    SessionCreated {
        session: SessionInfo,
        failures: Vec<RepoFailureInfo>,
        /// Rule picks that changed ("name: origin/old → origin/new", Desk's "기준 브랜치가
        /// 규칙에 따라 바뀌었습니다").
        #[serde(default)]
        base_changes: Vec<String>,
    },
    /// `repo.preview_base_rule`.
    BaseRulePreview(BaseRulePreview),
    /// `repo.branches`: current branch and refs; `repo.origin_branches`: names (no current).
    Branches {
        current: Option<String>,
        branches: Vec<String>,
    },
    Repos(Vec<RepoInfo>),
    Repo(RepoInfo),
    Attached(Attached),
    Entries(Vec<FsEntry>),
    Stat(FsStat),
    File(FileContent),
    /// `fs.write`: the new version.
    Written {
        version: String,
    },
    Browse(BrowseListing),
    FileMatches(Vec<FileMatch>),
    PathMatches(PathMatches),
    GitStatus(GitStatus),
    Diff(Vec<DiffFile>),
    FileAt(Option<FileContent>),
    /// `git.log`.
    GitLog(GitLog),
    /// `git.unpushed`: full commit ids.
    GitUnpushed(Vec<String>),
    /// `git.commit_files`.
    GitChanges(Vec<GitChange>),
    // --- local git actions (Changes / History footer)
    /// `git.stash_list`.
    GitStashes(Vec<crate::StashEntry>),
    /// `git.stash_action`.
    GitStashResult(crate::StashActionResult),
    /// `git.sync_status`.
    GitSyncStatus(crate::SyncStatus),
    /// `git.push` / `git.pull`: the branch pushed / pulled.
    GitBranchName(String),
    /// `git.branches`.
    GitBranches(crate::GitBranches),
    /// `git.fetch_remotes`.
    GitFetchStatus(crate::RemoteFetchStatus),
    /// `git.branch_refresh_info`.
    GitRefreshInfo(crate::BranchRefreshInfo),
    /// `git.branch_refresh_check`.
    GitRefreshCheck(crate::BranchRefreshCheck),
    /// `git.refresh_branch`.
    GitRefreshResult(crate::BranchRefreshResult),
    Def(DefResult),
    // --- pull requests and agent prompts
    /// `pr.status` (`None` = the branch has no PR).
    PrStatus(Option<crate::PrStatus>),
    /// `pr.gate`.
    PrGate(crate::PrGate),
    /// `pr.find_open` (`None` = no open PR).
    PrFound(Option<crate::PullRequestResult>),
    /// `pr.remote_branches`.
    PrTargets(crate::PrTargets),
    /// `pr.pending_work`.
    PrPendingWork(crate::PendingWork),
    /// `pr.leftover`.
    PrLeftover(crate::PrLeftover),
    /// `pr.reset_file`: the PR file's server path.
    PrFile { path: String },
    /// `pr.drift`.
    PrDrift(crate::PrDrift),
    /// `pr.unmerged` (`None` = cannot tell).
    PrUnmerged(Option<crate::UnmergedWork>),
    /// `pr.commit_preview`.
    PrCommitPreview(crate::CommitPrPreview),
    /// `pr.commit_text`: the title written.
    PrTitle(String),
    /// `pr.create`.
    PrCreated(crate::PullRequestResult),
    /// `pr.merge`.
    PrMerged(crate::PrMerged),
    /// `pr.close`.
    PrClosed(crate::PrClosed),
    /// `pr.update_branch`: GitHub's message (`None` = none given).
    PrBranchUpdated { message: Option<String> },
    /// `prompt.commit` / `prompt.pr`: the text to send.
    Prompt(String),
    AgentTools(Vec<AgentTool>),
    AgentResult(AgentToolResult),
    Widgets(Vec<WidgetInfo>),
    WidgetFile {
        mime: String,
        #[serde(with = "crate::bin")]
        data: Vec<u8>,
    },
    /// `Get`: the value as JSON text (`None`: not set); `Keys`: a JSON array; else `None`.
    WidgetData(Option<String>),
    Approvals(Vec<ApprovalInfo>),
    /// `None`: no opinion; `Some(reason)`: refuse the tool call.
    AgentHookDecision(Option<String>),
    Merges(Vec<RepoMerge>),
    Integration(crate::IntegrationInfo),
}

/// Server -> client event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum Notify {
    /// A session's agent activity changed (hooks).
    Activity {
        session: String,
        activity: AgentActivity,
    },
    /// A session's agent called a bridge tool the app carries out (e.g. showing something):
    /// answer with `Request::AgentReply` (the first answer wins; unanswered calls time out).
    AgentRequest {
        request: u64,
        session: String,
        tool: String,
        args: String,
    },
    /// OSC 0/2 title (empty = reset).
    Title {
        session: String,
        title: String,
    },
    Bell {
        session: String,
    },
    /// Process state changed (start, stop, exit).
    State {
        session: String,
        state: SessionState,
    },
    /// A session's user terminals changed (opened, removed); re-fetch with `UTermList`.
    UTermsChanged {
        session: String,
    },
    /// The session list changed (create, delete, rename); re-fetch with `SessionList`.
    SessionsChanged,
    /// The registered widgets or their open tabs changed; re-fetch with `WidgetList`.
    WidgetsChanged,
    /// The account list changed.
    AccountsChanged,
    /// The configuration list (or a configuration) changed.
    ConfigurationsChanged,
    /// The repository registry changed.
    ReposChanged,
    /// Files changed under a watched session (debounced; root-relative paths, deduplicated).
    /// `rescan` = too many changes to list: re-read everything shown.
    FsChanged {
        session: String,
        repo: String,
        paths: Vec<String>,
        rescan: bool,
    },
    /// The session's git state changed (index, HEAD, refs) without necessarily touching files.
    GitChanged {
        session: String,
        repo: String,
    },
    /// A batch of `search.text` results.
    SearchHits {
        search: u64,
        hits: Vec<SearchMatch>,
    },
    /// `search.text` finished (always sent last, also after a cancel).
    SearchDone {
        search: u64,
        summary: SearchSummary,
    },
}

/// One RPC frame payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum RpcMessage {
    Request {
        id: u64,
        req: Request,
    },
    Response {
        id: u64,
        result: Result<Response, ProtoError>,
    },
    Notify(Notify),
}

impl RpcMessage {
    /// MessagePack encoding (named fields).
    pub fn encode(&self) -> Vec<u8> {
        rmp_serde::to_vec_named(self).expect("RpcMessage serializes")
    }

    /// Decode MessagePack.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        rmp_serde::from_slice(bytes)
            .map_err(|e| ProtoError::new("bad-message").arg("detail", e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let msgs = vec![
            RpcMessage::Request {
                id: 1,
                req: Request::Hello(Hello {
                    proto_version: PROTO_VERSION,
                    client_version: "0.1".into(),
                    resume: Some(ResumeToken("t".into())),
                    agent: None,
                }),
            },
            RpcMessage::Request {
                id: 2,
                req: Request::TermWrite {
                    session: "abc".into(),
                    bytes: b"echo hi\r\x00\xff".to_vec(),
                },
            },
            RpcMessage::Response {
                id: 2,
                result: Err(ProtoError::new("session-not-found").arg("id", "abc")),
            },
            RpcMessage::Response {
                id: 3,
                result: Ok(Response::Attached(Attached {
                    session: "abc".into(),
                    stream: 2,
                    start_offset: 10,
                    reset: true,
                })),
            },
            RpcMessage::Notify(Notify::State {
                session: "abc".into(),
                state: SessionState::Exited(Some(3)),
            }),
            RpcMessage::Notify(Notify::SessionsChanged),
            RpcMessage::Request {
                id: 4,
                req: Request::ConfigurationUpdate(ConfigurationInfo {
                    slug: "web".into(),
                    name: "Web".into(),
                    skills: vec![crate::SkillInfo {
                        name: "lint".into(),
                        title: "lint".into(),
                        description: "Lints".into(),
                        enabled: true,
                    }],
                    mcp_servers: vec![crate::McpServerInfo {
                        name: "docs".into(),
                        command: "node".into(),
                        args: vec!["s.js".into()],
                        env: vec![("K".into(), "v".into())],
                    }],
                    instructions: "Use pnpm.".into(),
                    default_repos: Some(vec!["0a1b2c3d".into()]),
                    created_at: 1,
                }),
            },
            RpcMessage::Response {
                id: 5,
                result: Ok(Response::Providers(vec![ProviderInfo {
                    id: "claude".into(),
                    name: "Claude Code".into(),
                    glyph: "✻".into(),
                    color: Some(0xd97757),
                    detection: crate::ProviderDetection {
                        path: Some("C:/bin/claude.exe".into()),
                        version: Some("2.1.329".into()),
                        min_version: Some("2.1.318".into()),
                        usable: true,
                        error: None,
                    },
                    caps: crate::ProviderCaps {
                        accounts: true,
                        login: true,
                        skills: crate::Support::Native,
                        mcp: crate::Support::Native,
                        instructions: crate::Support::Native,
                        hooks: crate::Support::Unsupported,
                        resume: false,
                        session_name: true,
                        integration: false,
                        original_repo: crate::ModeCaps {
                            skills: crate::Support::Native,
                            mcp: crate::Support::Native,
                            instructions: crate::Support::Native,
                        },
                    },
                    answers: vec![crate::AutoAnswerInfo { id: "trust_workspace".into(), enabled: true }],
                }])),
            },
            RpcMessage::Response {
                id: 6,
                result: Ok(Response::AccountState(AccountState::LoggedOut)),
            },
            RpcMessage::Notify(Notify::AccountsChanged),
            RpcMessage::Request {
                id: 7,
                req: Request::GitCommitDiff {
                    session: "s".into(),
                    repo: "r".into(),
                    sha: "abc1234".into(),
                    path: Some("한글.txt".into()),
                },
            },
            RpcMessage::Response {
                id: 8,
                result: Ok(Response::GitLog(GitLog {
                    commits: vec![crate::CommitInfo {
                        sha: "a".repeat(40),
                        short: "aaaaaaa".into(),
                        subject: "첫 커밋".into(),
                        author: "t".into(),
                        email: "t@example.com".into(),
                        date: 1_700_000_000,
                    }],
                    has_more: true,
                })),
            },
            RpcMessage::Response {
                id: 9,
                result: Ok(Response::GitChanges(vec![GitChange {
                    path: "b.txt".into(),
                    orig_path: Some("a.txt".into()),
                    kind: crate::ChangeKind::Renamed,
                }])),
            },
            RpcMessage::Response {
                id: 10,
                result: Ok(Response::GitUnpushed(vec!["b".repeat(40)])),
            },
            // --- local git actions (Changes / History footer)
            RpcMessage::Request {
                id: 11,
                req: Request::GitCommitAction {
                    session: "s".into(),
                    repo: "r".into(),
                    sha: "c".repeat(40),
                    action: crate::CommitAction::Branch {
                        name: "기능/x".into(),
                    },
                },
            },
            RpcMessage::Request {
                id: 12,
                req: Request::GitStashPush {
                    session: "s".into(),
                    repo: "r".into(),
                    message: "메모".into(),
                    paths: Some(vec!["a.txt".into(), "한글.txt".into()]),
                },
            },
            RpcMessage::Response {
                id: 13,
                result: Ok(Response::GitStashes(vec![crate::StashEntry {
                    sha: "d".repeat(40),
                    message: String::new(),
                    branch: "main".into(),
                    auto: true,
                    date: 1_700_000_000,
                    files: vec![GitChange {
                        path: "n.txt".into(),
                        orig_path: None,
                        kind: crate::ChangeKind::Untracked,
                    }],
                }])),
            },
            RpcMessage::Response {
                id: 14,
                result: Ok(Response::GitBranches(crate::GitBranches {
                    current: Some("main".into()),
                    local: vec![crate::LocalBranch {
                        name: "f".into(),
                        checkout: crate::BranchCheckout::Worktree("C:/w/f".into()),
                    }],
                    remote: vec![crate::RemoteBranch {
                        remote: "origin".into(),
                        branch: "f".into(),
                        short_ref: "origin/f".into(),
                        has_local: true,
                    }],
                })),
            },
            RpcMessage::Request {
                id: 15,
                req: Request::GitRefreshBranch {
                    session: "s".into(),
                    repo: "r".into(),
                    options: crate::BranchRefreshOptions {
                        name: None,
                        commits: vec!["e".repeat(40)],
                        move_files: true,
                    },
                },
            },
            RpcMessage::Response {
                id: 16,
                result: Ok(Response::GitRefreshCheck(crate::BranchRefreshCheck {
                    conflict: Some(crate::RefreshConflict {
                        sha: "e".repeat(40),
                        subject: "x".into(),
                        files: vec!["a".into()],
                    }),
                    skipped: vec![],
                    files_conflict: vec!["b".into()],
                })),
            },
            RpcMessage::Response {
                id: 17,
                result: Ok(Response::GitSyncStatus(crate::SyncStatus {
                    ahead: 2,
                    upstream: Some("origin/main".into()),
                    ..Default::default()
                })),
            },
        ];
        for m in msgs {
            let b = m.encode();
            assert_eq!(RpcMessage::decode(&b).unwrap(), m);
        }
        // Bytes are encoded as a MessagePack bin, not an array of integers.
        let w = RpcMessage::Request {
            id: 9,
            req: Request::TermWrite {
                session: "s".into(),
                bytes: vec![0xff; 1000],
            },
        };
        assert!(w.encode().len() < 1100);
        assert_eq!(
            RpcMessage::decode(b"\xc1").unwrap_err().id,
            "bad-message".to_string()
        );
    }

    // --- pull requests and agent prompts
    #[test]
    fn pr_messages_round_trip() {
        let pr = crate::PrStatus {
            number: 12,
            url: "https://github.com/acme/w/pull/12".into(),
            title: "기능 추가".into(),
            state: crate::PrState::Open,
            draft: false,
            head_ref: "feature".into(),
            head_sha: "c".repeat(40),
            base_ref: "main".into(),
            mergeable: None,
            mergeable_state: "unknown".into(),
            approved: true,
            changes_requested: false,
        };
        let msgs = vec![
            RpcMessage::Request {
                id: 1,
                req: Request::PrCreate {
                    session: "s".into(),
                    repo: "r".into(),
                    base: "main".into(),
                    draft: true,
                    push: false,
                    push_sha: Some("d".repeat(40)),
                    options: Some(crate::CommitPrTextOptions {
                        title: Some("제목".into()),
                        push: Some(false),
                        ..Default::default()
                    }),
                },
            },
            RpcMessage::Request {
                id: 2,
                req: Request::PrMerge {
                    session: "s".into(),
                    repo: "r".into(),
                    number: 12,
                    sha: "c".repeat(40),
                    method: crate::PrMergeMethod::Rebase,
                    delete_branch: true,
                },
            },
            RpcMessage::Request {
                id: 3,
                req: Request::SessionSendPrompt {
                    session: "s".into(),
                    text: "[커밋 요청]\n- a".into(),
                },
            },
            RpcMessage::Response {
                id: 4,
                result: Ok(Response::PrStatus(Some(pr))),
            },
            RpcMessage::Response {
                id: 5,
                result: Ok(Response::PrCreated(crate::PullRequestResult {
                    kind: crate::PrResultKind::Page,
                    url: "https://github.com/acme/w/compare/main...feature?expand=1".into(),
                    title: "t".into(),
                    number: None,
                    reason: Some(ProtoError::new("gh-auth").arg("host", "github.com")),
                    base: None,
                    mergeable_state: None,
                    session_base: None,
                })),
            },
            RpcMessage::Response {
                id: 6,
                result: Ok(Response::PrGate(crate::PrGate {
                    head_sha: "c".repeat(40),
                    review_decision: None,
                    merge_state: "BLOCKED".into(),
                    checks: vec![crate::PrCheck {
                        name: "ci".into(),
                        state: crate::PrCheckState::Pending,
                        required: true,
                    }],
                })),
            },
            RpcMessage::Response {
                id: 7,
                result: Ok(Response::PrBranchUpdated { message: None }),
            },
        ];
        for m in msgs {
            let b = m.encode();
            assert_eq!(RpcMessage::decode(&b).unwrap(), m);
        }
    }
}
