//! RPC messages (MessagePack via `rmp-serde`, named fields) and the handshake.

use serde::{Deserialize, Serialize};

use crate::error::ProtoError;
use crate::files::{
    BrowseListing, DefResult, DiffAgainst, DiffFile, FileContent, FileMatch, FsEntry, FsStat,
    GitRev, GitStatus, SearchMatch, SearchSummary, TextEncoding, TextSearch,
};

/// Major protocol version. Peers with a different major version refuse each other.
pub const PROTO_VERSION: u32 = 1;

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
    /// Optional features (`"term"`, `"profiles"`, `"sessions"`).
    pub capabilities: Vec<String>,
    /// Token to send in the next `Hello` after a reconnect.
    pub resume_token: ResumeToken,
    /// True if `Hello.resume` named a client the server still knew.
    pub resumed: bool,
    /// Random id of the server process. Terminal offsets are only meaningful within one
    /// instance: after a change, clients re-attach without `from_offset` (full replay).
    pub server_instance: String,
}

/// Agent kind of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    /// `claude`.
    Claude,
    /// `codex`.
    Codex,
    /// cmd / pwsh / $SHELL.
    Shell,
}

/// A profile (secrets and config dirs stay on the server).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileInfo {
    /// Slug name.
    pub name: String,
    /// Unix seconds.
    pub created_at: u64,
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
    /// Profile name.
    pub profile: String,
    /// Repository root.
    pub repo_path: String,
    /// Session worktree.
    pub worktree_path: String,
    /// Branch (`ide/<id>`).
    pub branch: String,
    /// Agent kind.
    pub agent: Agent,
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
    /// Profile to run under.
    pub profile: String,
    /// Any folder inside the repository (server path).
    pub repo_path: String,
    /// Agent to run.
    pub agent: Agent,
    /// Title; empty = derived.
    pub title: String,
    /// Revision to branch from; empty = HEAD.
    pub base_ref: String,
    /// Start the process right away.
    pub start: bool,
    /// Initial terminal size.
    pub cols: u16,
    /// Initial terminal size.
    pub rows: u16,
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
    ProfileList,
    ProfileCreate {
        name: String,
    },
    ProfileRename {
        old: String,
        new: String,
    },
    ProfileDelete {
        name: String,
        remove_files: bool,
    },
    SessionList,
    SessionCreate(NewSession),
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

    // ---- Phase 2: files (paths root-relative to the session worktree) ----
    /// List a directory (`dir` = `""` for the root). `.git` is never listed; ignored entries
    /// only with `include_ignored`.
    FsList {
        session: String,
        dir: String,
        include_ignored: bool,
    },
    FsStat {
        session: String,
        path: String,
    },
    /// Read a file (`max_bytes` 0 = default 2 MiB).
    FsRead {
        session: String,
        path: String,
        max_bytes: u64,
    },
    /// Write a whole file. `expected_version` = the `FileContent::version` it was read at
    /// (`fs-conflict` if the file changed on disk since); `None` = overwrite unconditionally.
    /// `encoding` `None` = UTF-8 without BOM.
    FsWrite {
        session: String,
        path: String,
        content: String,
        expected_version: Option<String>,
        encoding: Option<TextEncoding>,
    },
    /// Create a new file (`fs-exists` if it exists); parent folders are created.
    FsCreateFile {
        session: String,
        path: String,
        content: String,
    },
    /// Create a folder (and missing parents); `fs-exists` if it exists.
    FsCreateDir {
        session: String,
        path: String,
    },
    /// Rename/move inside the session (`fs-exists` if `to` exists).
    FsRename {
        session: String,
        from: String,
        to: String,
    },
    /// Delete a file, symlink or folder (non-empty folders need `recursive`).
    FsDelete {
        session: String,
        path: String,
        recursive: bool,
    },
    /// Start sending `Notify::FsChanged`/`GitChanged` for the session to this connection.
    FsWatch {
        session: String,
    },
    FsUnwatch {
        session: String,
    },
    /// The absolute roots repositories can be picked from.
    BrowseRoots,
    /// Sub-folders of an absolute folder inside a browse root (`""` = the first root).
    BrowseList {
        abs_dir: String,
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
        query: String,
        max_results: u32,
    },
    /// `base` = branch to compare with (`None` = the branch checked out in the main repo).
    GitStatus {
        session: String,
        base: Option<String>,
    },
    /// `path` `None` = every changed tracked file.
    GitDiff {
        session: String,
        path: Option<String>,
        against: DiffAgainst,
        base: Option<String>,
    },
    /// File content at a revision; `Response::FileAt(None)` if it does not exist there.
    GitFileAt {
        session: String,
        path: String,
        rev: GitRev,
        base: Option<String>,
    },
    /// Go to definition at `line`/`col` (1-based; `col` in chars) of `path`. `text` = the
    /// editor's current (possibly unsaved) content; `None` = read the file.
    DefLookup {
        session: String,
        path: String,
        line: u32,
        col: u32,
        text: Option<String>,
    },
}

impl Request {
    /// Method name for logs and audit.
    pub fn method(&self) -> &'static str {
        match self {
            Request::Hello(_) => "hello",
            Request::Ping => "ping",
            Request::ProfileList => "profile.list",
            Request::ProfileCreate { .. } => "profile.create",
            Request::ProfileRename { .. } => "profile.rename",
            Request::ProfileDelete { .. } => "profile.delete",
            Request::SessionList => "session.list",
            Request::SessionCreate(_) => "session.create",
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
            Request::BrowseRoots => "browse.roots",
            Request::BrowseList { .. } => "browse.list",
            Request::SearchText { .. } => "search.text",
            Request::SearchCancel { .. } => "search.cancel",
            Request::SearchFiles { .. } => "search.files",
            Request::GitStatus { .. } => "git.status",
            Request::GitDiff { .. } => "git.diff",
            Request::GitFileAt { .. } => "git.file_at",
            Request::DefLookup { .. } => "def.lookup",
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
    Profiles(Vec<ProfileInfo>),
    Profile(ProfileInfo),
    Sessions(Vec<SessionInfo>),
    Session(SessionInfo),
    Attached(Attached),
    Entries(Vec<FsEntry>),
    Stat(FsStat),
    File(FileContent),
    /// `fs.write`: the new version.
    Written {
        version: String,
    },
    Roots(Vec<String>),
    Browse(BrowseListing),
    FileMatches(Vec<FileMatch>),
    GitStatus(GitStatus),
    Diff(Vec<DiffFile>),
    FileAt(Option<FileContent>),
    Def(DefResult),
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
    /// The profile list changed.
    ProfilesChanged,
    /// Files changed under a watched session (debounced; root-relative paths, deduplicated).
    /// `rescan` = too many changes to list: re-read everything shown.
    FsChanged {
        session: String,
        paths: Vec<String>,
        rescan: bool,
    },
    /// The session's git state changed (index, HEAD, refs) without necessarily touching files.
    GitChanged {
        session: String,
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
}
