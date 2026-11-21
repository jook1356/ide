//! RPC messages (MessagePack via `rmp-serde`, named fields) and the handshake.

use serde::{Deserialize, Serialize};

use crate::agents::{
    AccountInfo, AccountState, BaseRuleInfo, BaseRulePreview, ConfigurationInfo, ProviderInfo,
    RepoInfo, SessionKind, SessionMode, SessionRepo,
};
use crate::error::ProtoError;
use crate::files::{
    BrowseListing, DefResult, DiffAgainst, DiffFile, FileContent, FileMatch, FsEntry,
    FsStat, GitChange, GitLog, GitRev, GitStatus, SearchMatch, SearchSummary, TextEncoding, TextSearch,
};

/// Major protocol version. Peers with a different major version refuse each other.
pub const PROTO_VERSION: u32 = 6;

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
    /// Replace name, skills, MCP servers and instructions of the configuration `slug`.
    ConfigurationUpdate(ConfigurationInfo),
    /// Delete a configuration (`configuration-in-use`, `last-configuration`).
    ConfigurationDelete {
        slug: String,
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
    SessionDelete {
        id: String,
        remove_worktree: bool,
    },
    SessionRename {
        id: String,
        title: String,
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
    /// Delete a file, symlink or folder (non-empty folders need `recursive`).
    FsDelete {
        session: String,
        repo: String,
        path: String,
        recursive: bool,
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
            Request::SessionRename { .. } => "session.rename",
            Request::TermAttach { .. } => "term.attach",
            Request::TermDetach { .. } => "term.detach",
            Request::TermWrite { .. } => "term.write",
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
            Request::FsWatch { .. } => "fs.watch",
            Request::FsUnwatch { .. } => "fs.unwatch",
            Request::BrowsePickList { .. } => "browse.pick_list",
            Request::SearchText { .. } => "search.text",
            Request::SearchCancel { .. } => "search.cancel",
            Request::SearchFiles { .. } => "search.files",
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
    Sessions(Vec<SessionInfo>),
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
}

/// Server -> client event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum Notify {
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
    /// The session list changed (create, delete, rename); re-fetch with `SessionList`.
    SessionsChanged,
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
                    skills: vec!["C:/skills/lint".into()],
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
                        original_repo: crate::ModeCaps {
                            skills: crate::Support::Native,
                            mcp: crate::Support::Native,
                            instructions: crate::Support::Native,
                        },
                    },
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
