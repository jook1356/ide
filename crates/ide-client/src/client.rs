//! [`Client`]: RPC, events, terminal attachments, reconnect + resume.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use ide_proto::{
    Attached, BrowseConfig, BrowseListing, ChunkDecoder, DefResult, DiffAgainst, DiffFile,
    FileContent, FileMatch, FsEntry, FsStat, GitRev, GitStatus, Hello, Mux, MuxConfig, MuxError,
    NewSession, Notify, PROTO_VERSION, ProfileInfo, ProtoError, Request, Response, ResumeToken,
    Role, RpcMessage, RpcReceiver, SearchMatch, SearchSummary, SessionInfo, Stream, TermChunk,
    TextEncoding, TextSearch, Welcome,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::transport::Transport;

/// Client settings.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// Sent in `Hello`.
    pub client_version: String,
    /// Reconnect automatically after the connection drops (and retry failed connects).
    pub reconnect: bool,
    /// First reconnect delay.
    pub backoff_min: Duration,
    /// Largest reconnect delay.
    pub backoff_max: Duration,
    /// Per-request timeout.
    pub request_timeout: Duration,
    /// Transport connect + handshake timeout.
    pub connect_timeout: Duration,
    /// Terminal chunks buffered for the app before the server's stream is throttled.
    pub term_buffer: usize,
}

impl Default for ClientOptions {
    fn default() -> Self {
        ClientOptions {
            client_version: ide_proto::VERSION.to_owned(),
            reconnect: true,
            backoff_min: Duration::from_millis(100),
            backoff_max: Duration::from_secs(5),
            request_timeout: Duration::from_secs(120),
            connect_timeout: Duration::from_secs(30),
            term_buffer: 64,
        }
    }
}

/// Something the app should react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    /// (Re)connected and handshaken. Terminals are being re-attached.
    Connected(Welcome),
    /// The connection dropped or a connect attempt failed (`error`). Reconnecting continues
    /// in the background if enabled.
    Disconnected {
        /// Why a connect attempt failed (`None` when an established connection dropped).
        error: Option<ProtoError>,
    },
    /// Server notification.
    Notify(Notify),
    /// Terminal output of an attached session, in order, exactly once. `reset` = clear the
    /// terminal model first (the chunk may be empty).
    Term(TermChunk),
    /// Re-attaching a terminal after a reconnect failed (e.g. the session was deleted).
    AttachFailed {
        /// Session id.
        session: String,
        /// Why.
        error: ProtoError,
    },
}

fn disconnected() -> ProtoError {
    ProtoError::new("disconnected")
}

#[derive(Debug, Clone, Default)]
struct Status {
    connected: Option<Welcome>,
    attempts: u64,
    last_error: Option<ProtoError>,
}

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Response, ProtoError>>>>;

/// One `search.text` result batch or its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEvent {
    /// Matching lines (in walk order).
    Hits(Vec<SearchMatch>),
    /// The search ended (always last).
    Done(SearchSummary),
}

struct Conn {
    mux: Mux,
    pending: Pending,
    /// `search.text` routes of this connection by search id.
    searches: Mutex<HashMap<u64, mpsc::UnboundedSender<SearchEvent>>>,
    _child: Mutex<Option<tokio::process::Child>>,
}

impl Conn {
    async fn call(&self, id: u64, req: Request, timeout: Duration) -> Result<Response, ProtoError> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = self.mux.send_rpc(&RpcMessage::Request { id, req }.encode()) {
            self.pending.lock().unwrap().remove(&id);
            return Err(match e {
                MuxError::TooLarge => ProtoError::new("message-too-large"),
                _ => disconnected(),
            });
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(disconnected()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(ProtoError::new("timeout"))
            }
        }
    }
}

struct AttachState {
    /// Offset of the next byte the app expects (`None` = full replay).
    next: Option<u64>,
    /// Incremented per attach; chunks of older streams are dropped.
    epoch: u64,
    /// A reset was delivered and no bytes since (a server reset at `next` is redundant).
    clean: bool,
}

struct Attachment {
    session: String,
    state: tokio::sync::Mutex<AttachState>,
    detached: AtomicBool,
}

type EventSlot = Arc<
    Mutex<
        Option<(
            mpsc::UnboundedReceiver<ClientEvent>,
            mpsc::Receiver<ClientEvent>,
        )>,
    >,
>;

struct Inner {
    id: u64,
    transport: Box<dyn Transport>,
    opts: ClientOptions,
    conn: Mutex<Option<Arc<Conn>>>,
    status: watch::Sender<Status>,
    next_id: AtomicU64,
    attachments: Mutex<HashMap<String, Arc<Attachment>>>,
    /// Sessions watched with `fs.watch` (re-watched after a reconnect).
    watched: Mutex<HashSet<String>>,
    ctrl_tx: mpsc::UnboundedSender<ClientEvent>,
    term_tx: mpsc::Sender<ClientEvent>,
    events: EventSlot,
    token: Mutex<Option<ResumeToken>>,
    instance: Mutex<Option<String>>,
    shutdown: watch::Sender<bool>,
    kick: tokio::sync::Notify,
}

struct Handle(Arc<Inner>);

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.shutdown.send_replace(true);
        if let Some(c) = self.0.conn.lock().unwrap().take() {
            c.mux.close();
        }
    }
}

/// A connection to a server (cheap to clone). Dropping the last clone disconnects.
#[derive(Clone)]
pub struct Client(Arc<Handle>);

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("id", &self.id())
            .field("transport", &self.inner().transport.describe())
            .field("connected", &self.is_connected())
            .finish()
    }
}

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);

impl Client {
    /// Start connecting in the background (needs a tokio runtime). Watch [`Client::events`]
    /// for `Connected`, or use [`Client::connect`] to wait for the first handshake.
    pub fn new(transport: impl Transport, opts: ClientOptions) -> Client {
        let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
        let (term_tx, term_rx) = mpsc::channel(opts.term_buffer.max(1));
        let inner = Arc::new(Inner {
            id: NEXT_CLIENT.fetch_add(1, Ordering::Relaxed),
            transport: Box::new(transport),
            opts,
            conn: Mutex::new(None),
            status: watch::channel(Status::default()).0,
            next_id: AtomicU64::new(1),
            attachments: Mutex::new(HashMap::new()),
            watched: Mutex::new(HashSet::new()),
            ctrl_tx,
            term_tx,
            events: Arc::new(Mutex::new(Some((ctrl_rx, term_rx)))),
            token: Mutex::new(None),
            instance: Mutex::new(None),
            shutdown: watch::channel(false).0,
            kick: tokio::sync::Notify::new(),
        });
        tokio::spawn(run(inner.clone()));
        Client(Arc::new(Handle(inner)))
    }

    /// Connect and wait for the first handshake; fails with the first attempt's error.
    pub async fn connect(
        transport: impl Transport,
        opts: ClientOptions,
    ) -> Result<Client, ProtoError> {
        let wait = opts.connect_timeout + Duration::from_secs(5);
        let client = Client::new(transport, opts);
        let mut rx = client.inner().status.subscribe();
        let r = tokio::time::timeout(
            wait,
            rx.wait_for(|s| s.connected.is_some() || s.last_error.is_some()),
        )
        .await;
        match r {
            Ok(Ok(s)) if s.connected.is_some() => Ok(client.clone()),
            Ok(Ok(s)) => Err(s.last_error.clone().unwrap_or_else(disconnected)),
            Ok(Err(_)) => Err(disconnected()),
            Err(_) => Err(ProtoError::new("timeout")),
        }
    }

    fn inner(&self) -> &Arc<Inner> {
        &self.0.0
    }

    /// Process-unique id of this client.
    pub fn id(&self) -> u64 {
        self.inner().id
    }

    /// The event stream. Only one exists at a time: `None` while another [`ClientEvents`] is
    /// alive (it hands the receivers back when dropped).
    pub fn events(&self) -> Option<ClientEvents> {
        let (ctrl, term) = self.inner().events.lock().unwrap().take()?;
        Some(ClientEvents {
            ctrl: Some(ctrl),
            term: Some(term),
            slot: self.inner().events.clone(),
        })
    }

    /// True while connected and handshaken.
    pub fn is_connected(&self) -> bool {
        self.inner().status.borrow().connected.is_some()
    }

    /// The current connection's `Welcome`.
    pub fn welcome(&self) -> Option<Welcome> {
        self.inner().status.borrow().connected.clone()
    }

    /// Wait until connected.
    pub async fn wait_connected(&self, timeout: Duration) -> Result<Welcome, ProtoError> {
        let mut rx = self.inner().status.subscribe();
        match tokio::time::timeout(timeout, rx.wait_for(|s| s.connected.is_some())).await {
            Ok(Ok(s)) => Ok(s.connected.clone().expect("checked")),
            Ok(Err(_)) => Err(disconnected()),
            Err(_) => Err(ProtoError::new("timeout")),
        }
    }

    /// Drop the current connection; the background loop reconnects (if enabled) and resumes.
    pub fn reconnect(&self) {
        if let Some(c) = self.inner().conn.lock().unwrap().as_ref() {
            c.mux.close();
        }
        self.inner().kick.notify_waiters();
    }

    /// Offset of the next terminal byte expected for an attached session.
    pub async fn term_offset(&self, session: &str) -> Option<u64> {
        let a = self
            .inner()
            .attachments
            .lock()
            .unwrap()
            .get(session)
            .cloned()?;
        a.state.lock().await.next
    }

    async fn call(&self, req: Request) -> Result<Response, ProtoError> {
        self.inner().call(req).await
    }

    /// Round trip.
    pub async fn ping(&self) -> Result<(), ProtoError> {
        expect(self.call(Request::Ping).await, |r| {
            matches!(r, Response::Pong).then_some(())
        })
    }

    /// `profile.list`.
    pub async fn profiles(&self) -> Result<Vec<ProfileInfo>, ProtoError> {
        expect(self.call(Request::ProfileList).await, |r| match r {
            Response::Profiles(p) => Some(p),
            _ => None,
        })
    }

    /// `profile.create`.
    pub async fn create_profile(&self, name: &str) -> Result<ProfileInfo, ProtoError> {
        expect_profile(
            self.call(Request::ProfileCreate { name: name.into() })
                .await,
        )
    }

    /// `profile.rename` (sessions follow).
    pub async fn rename_profile(&self, old: &str, new: &str) -> Result<ProfileInfo, ProtoError> {
        expect_profile(
            self.call(Request::ProfileRename {
                old: old.into(),
                new: new.into(),
            })
            .await,
        )
    }

    /// `profile.delete`.
    pub async fn delete_profile(&self, name: &str, remove_files: bool) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::ProfileDelete {
                name: name.into(),
                remove_files,
            })
            .await,
        )
    }

    /// `session.list`.
    pub async fn sessions(&self) -> Result<Vec<SessionInfo>, ProtoError> {
        expect(self.call(Request::SessionList).await, |r| match r {
            Response::Sessions(s) => Some(s),
            _ => None,
        })
    }

    /// `session.create` (repo -> worktree -> optional start).
    pub async fn create_session(&self, req: NewSession) -> Result<SessionInfo, ProtoError> {
        expect_session(self.call(Request::SessionCreate(req)).await)
    }

    /// `session.start`.
    pub async fn start_session(&self, id: &str) -> Result<SessionInfo, ProtoError> {
        expect_session(self.call(Request::SessionStart { id: id.into() }).await)
    }

    /// `session.stop` (waits until the process is gone).
    pub async fn stop_session(&self, id: &str) -> Result<SessionInfo, ProtoError> {
        expect_session(self.call(Request::SessionStop { id: id.into() }).await)
    }

    /// `session.restart`.
    pub async fn restart_session(&self, id: &str) -> Result<SessionInfo, ProtoError> {
        expect_session(self.call(Request::SessionRestart { id: id.into() }).await)
    }

    /// `session.delete` (stops it first; optionally removes worktree + branch).
    pub async fn delete_session(&self, id: &str, remove_worktree: bool) -> Result<(), ProtoError> {
        self.forget(id);
        expect_unit(
            self.call(Request::SessionDelete {
                id: id.into(),
                remove_worktree,
            })
            .await,
        )
    }

    /// `session.rename`.
    pub async fn rename_session(&self, id: &str, title: &str) -> Result<SessionInfo, ProtoError> {
        expect_session(
            self.call(Request::SessionRename {
                id: id.into(),
                title: title.into(),
            })
            .await,
        )
    }

    /// Attach a session's terminal: output from `from_offset` (`None` = everything the server
    /// still has, starting with a reset) arrives as [`ClientEvent::Term`]. The attachment is
    /// remembered: after a reconnect it resumes from the last delivered offset. If not
    /// connected, it is attached on the next connect and this returns `disconnected`.
    pub async fn attach(
        &self,
        session: &str,
        from_offset: Option<u64>,
    ) -> Result<Attached, ProtoError> {
        let inner = self.inner();
        let att = inner
            .attachments
            .lock()
            .unwrap()
            .entry(session.to_owned())
            .or_insert_with(|| {
                Arc::new(Attachment {
                    session: session.to_owned(),
                    state: tokio::sync::Mutex::new(AttachState {
                        next: None,
                        epoch: 0,
                        clean: false,
                    }),
                    detached: AtomicBool::new(false),
                })
            })
            .clone();
        {
            let mut st = att.state.lock().await;
            st.next = from_offset;
            st.clean = false;
        }
        let conn = inner.current().ok_or_else(disconnected)?;
        inner.attach_on(&conn, &att).await
    }

    /// Stop streaming a session's terminal.
    pub async fn detach(&self, session: &str) -> Result<(), ProtoError> {
        self.forget(session);
        if self.is_connected() {
            expect_unit(
                self.call(Request::TermDetach {
                    session: session.into(),
                })
                .await,
            )?;
        }
        Ok(())
    }

    fn forget(&self, session: &str) {
        if let Some(a) = self.inner().attachments.lock().unwrap().remove(session) {
            a.detached.store(true, Ordering::Release);
        }
    }

    /// Terminal input (keys, committed IME text, paste).
    pub async fn write(&self, session: &str, bytes: impl Into<Vec<u8>>) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::TermWrite {
                session: session.into(),
                bytes: bytes.into(),
            })
            .await,
        )
    }

    /// `server.shutdown`: the server stops every session and exits. The reply arrives before
    /// the process exits; afterwards the connection drops (and a local transport would start
    /// a new daemon on the next reconnect, so drop the client first if that is not wanted).
    pub async fn shutdown_server(&self) -> Result<(), ProtoError> {
        expect_unit(self.call(Request::ServerShutdown).await)
    }

    /// Terminal resize (the most recent caller sets the PTY size).
    pub async fn resize(&self, session: &str, cols: u16, rows: u16) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::TermResize {
                session: session.into(),
                cols,
                rows,
            })
            .await,
        )
    }

    // ---- Phase 2: files, browsing, search, git, go-to-definition ----
    // Paths are root-relative to the session worktree (`/`-separated, `""` = root).

    /// `fs.list`: entries of `dir` (directories first). `.git` is never listed; ignored
    /// entries only with `include_ignored` (flagged `ignored`).
    pub async fn fs_list(
        &self,
        session: &str,
        dir: &str,
        include_ignored: bool,
    ) -> Result<Vec<FsEntry>, ProtoError> {
        expect(
            self.call(Request::FsList {
                session: session.into(),
                dir: dir.into(),
                include_ignored,
            })
            .await,
            |r| match r {
                Response::Entries(e) => Some(e),
                _ => None,
            },
        )
    }

    /// `fs.stat` (a symlink is reported as such, not followed).
    pub async fn fs_stat(&self, session: &str, path: &str) -> Result<FsStat, ProtoError> {
        expect(
            self.call(Request::FsStat {
                session: session.into(),
                path: path.into(),
            })
            .await,
            |r| match r {
                Response::Stat(s) => Some(s),
                _ => None,
            },
        )
    }

    /// `fs.read` (`max_bytes` 0 = 2 MiB; at most 8 MiB). Keep `version` and `encoding` for
    /// [`Client::fs_write`].
    pub async fn fs_read(
        &self,
        session: &str,
        path: &str,
        max_bytes: u64,
    ) -> Result<FileContent, ProtoError> {
        expect(
            self.call(Request::FsRead {
                session: session.into(),
                path: path.into(),
                max_bytes,
            })
            .await,
            |r| match r {
                Response::File(f) => Some(f),
                _ => None,
            },
        )
    }

    /// `fs.write`: replace the file's content; returns the new version. With
    /// `expected_version`, fails with `fs-conflict` (args `path`, `expected`, `actual`) when
    /// the file changed on disk since it was read. `encoding` `None` = UTF-8 without BOM.
    pub async fn fs_write(
        &self,
        session: &str,
        path: &str,
        content: impl Into<String>,
        expected_version: Option<&str>,
        encoding: Option<TextEncoding>,
    ) -> Result<String, ProtoError> {
        expect_written(
            self.call(Request::FsWrite {
                session: session.into(),
                path: path.into(),
                content: content.into(),
                expected_version: expected_version.map(str::to_owned),
                encoding,
            })
            .await,
        )
    }

    /// `fs.create_file` (`fs-exists` if it exists); returns the version.
    pub async fn fs_create_file(
        &self,
        session: &str,
        path: &str,
        content: impl Into<String>,
    ) -> Result<String, ProtoError> {
        expect_written(
            self.call(Request::FsCreateFile {
                session: session.into(),
                path: path.into(),
                content: content.into(),
            })
            .await,
        )
    }

    /// `fs.create_dir` (with parents; `fs-exists` if it exists).
    pub async fn fs_create_dir(&self, session: &str, path: &str) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::FsCreateDir {
                session: session.into(),
                path: path.into(),
            })
            .await,
        )
    }

    /// `fs.rename` / move inside the session (`fs-exists` if `to` exists).
    pub async fn fs_rename(&self, session: &str, from: &str, to: &str) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::FsRename {
                session: session.into(),
                from: from.into(),
                to: to.into(),
            })
            .await,
        )
    }

    /// `fs.delete` (permanent; folders with content need `recursive`).
    pub async fn fs_delete(
        &self,
        session: &str,
        path: &str,
        recursive: bool,
    ) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::FsDelete {
                session: session.into(),
                path: path.into(),
                recursive,
            })
            .await,
        )
    }

    /// `fs.watch`: `Notify::FsChanged` / `Notify::GitChanged` for the session arrive as
    /// [`ClientEvent::Notify`]. Remembered: re-watched after a reconnect (refresh what is
    /// shown on `Connected`, changes during the gap are not replayed).
    pub async fn fs_watch(&self, session: &str) -> Result<(), ProtoError> {
        self.inner()
            .watched
            .lock()
            .unwrap()
            .insert(session.to_owned());
        expect_unit(
            self.call(Request::FsWatch {
                session: session.into(),
            })
            .await,
        )
    }

    /// Stop watching a session.
    pub async fn fs_unwatch(&self, session: &str) -> Result<(), ProtoError> {
        self.inner().watched.lock().unwrap().remove(session);
        if !self.is_connected() {
            return Ok(());
        }
        expect_unit(
            self.call(Request::FsUnwatch {
                session: session.into(),
            })
            .await,
        )
    }

    /// `browse.roots`: absolute folders repositories can be picked from.
    pub async fn browse_roots(&self) -> Result<Vec<String>, ProtoError> {
        expect(self.call(Request::BrowseRoots).await, |r| match r {
            Response::Roots(r) => Some(r),
            _ => None,
        })
    }

    /// `browse.list`: sub-folders of an absolute folder inside the browse roots (`""` = the
    /// first root), with `is_git_repo` flags.
    pub async fn browse_list(&self, abs_dir: &str) -> Result<BrowseListing, ProtoError> {
        expect(
            self.call(Request::BrowseList {
                abs_dir: abs_dir.into(),
            })
            .await,
            |r| match r {
                Response::Browse(b) => Some(b),
                _ => None,
            },
        )
    }

    /// `browse.config`: the home dir and the extra browse roots saved on the server.
    pub async fn browse_config(&self) -> Result<BrowseConfig, ProtoError> {
        expect(self.call(Request::BrowseConfig).await, |r| match r {
            Response::BrowseConfig(c) => Some(c),
            _ => None,
        })
    }

    /// `browse.set_roots`: replace the extra browse roots (saved in the server's
    /// `server.json`). Each root must be an existing absolute folder (`path-invalid`,
    /// `fs-not-found`, `fs-not-a-dir`, `path-denied` for a filesystem root).
    pub async fn browse_set_roots(&self, roots: Vec<String>) -> Result<BrowseConfig, ProtoError> {
        expect(
            self.call(Request::BrowseSetRoots { roots }).await,
            |r| match r {
                Response::BrowseConfig(c) => Some(c),
                _ => None,
            },
        )
    }

    /// `search.text`: start a search; results stream through the returned [`SearchStream`]
    /// (dropping it cancels the search). Query errors (`search-bad-regex`, ...) are returned
    /// here.
    pub async fn search_text(&self, query: TextSearch) -> Result<SearchStream, ProtoError> {
        let inner = self.inner();
        let conn = inner.current().ok_or_else(disconnected)?;
        let search = inner.next_id();
        let (tx, rx) = mpsc::unbounded_channel();
        conn.searches.lock().unwrap().insert(search, tx);
        let r = conn
            .call(
                inner.next_id(),
                Request::SearchText { search, query },
                inner.opts.request_timeout,
            )
            .await;
        if let Err(e) = expect_unit(r) {
            conn.searches.lock().unwrap().remove(&search);
            return Err(e);
        }
        Ok(SearchStream {
            id: search,
            rx,
            inner: Arc::downgrade(inner),
            done: false,
        })
    }

    /// `search.files`: fuzzy file-name search (`max_results` 0 = 100), best first.
    pub async fn search_files(
        &self,
        session: &str,
        query: &str,
        max_results: u32,
    ) -> Result<Vec<FileMatch>, ProtoError> {
        expect(
            self.call(Request::SearchFiles {
                session: session.into(),
                query: query.into(),
                max_results,
            })
            .await,
            |r| match r {
                Response::FileMatches(m) => Some(m),
                _ => None,
            },
        )
    }

    /// `git.status` (`base` = branch to compare with; `None` = the branch checked out in the
    /// session's repository, else `main`/`master`).
    pub async fn git_status(
        &self,
        session: &str,
        base: Option<&str>,
    ) -> Result<GitStatus, ProtoError> {
        expect(
            self.call(Request::GitStatus {
                session: session.into(),
                base: base.map(str::to_owned),
            })
            .await,
            |r| match r {
                Response::GitStatus(s) => Some(s),
                _ => None,
            },
        )
    }

    /// `git.diff`: structured hunks of one file (`path`) or of every changed tracked file.
    pub async fn git_diff(
        &self,
        session: &str,
        path: Option<&str>,
        against: DiffAgainst,
        base: Option<&str>,
    ) -> Result<Vec<DiffFile>, ProtoError> {
        expect(
            self.call(Request::GitDiff {
                session: session.into(),
                path: path.map(str::to_owned),
                against,
                base: base.map(str::to_owned),
            })
            .await,
            |r| match r {
                Response::Diff(d) => Some(d),
                _ => None,
            },
        )
    }

    /// `git.file_at`: content at a revision (`None` if the file does not exist there), for
    /// side-by-side views.
    pub async fn git_file_at(
        &self,
        session: &str,
        path: &str,
        rev: GitRev,
        base: Option<&str>,
    ) -> Result<Option<FileContent>, ProtoError> {
        expect(
            self.call(Request::GitFileAt {
                session: session.into(),
                path: path.into(),
                rev,
                base: base.map(str::to_owned),
            })
            .await,
            |r| match r {
                Response::FileAt(f) => Some(f),
                _ => None,
            },
        )
    }

    /// `def.lookup`: go to definition at 1-based `line`/`col` (`col` in chars) of `path`;
    /// `text` = the editor's unsaved content (`None` = the file on disk).
    pub async fn def_lookup(
        &self,
        session: &str,
        path: &str,
        line: u32,
        col: u32,
        text: Option<String>,
    ) -> Result<DefResult, ProtoError> {
        expect(
            self.call(Request::DefLookup {
                session: session.into(),
                path: path.into(),
                line,
                col,
                text,
            })
            .await,
            |r| match r {
                Response::Def(d) => Some(d),
                _ => None,
            },
        )
    }
}

/// Results of one [`Client::search_text`]. Dropping it before `Done` cancels the search.
pub struct SearchStream {
    id: u64,
    rx: mpsc::UnboundedReceiver<SearchEvent>,
    inner: std::sync::Weak<Inner>,
    done: bool,
}

impl std::fmt::Debug for SearchStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchStream")
            .field("id", &self.id)
            .field("done", &self.done)
            .finish()
    }
}

impl SearchStream {
    /// The search id (as in `Notify::SearchHits`).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Next batch, then `Done`, then `None`. `None` before `Done` = the connection dropped.
    pub async fn next(&mut self) -> Option<SearchEvent> {
        if self.done {
            return None;
        }
        let e = self.rx.recv().await;
        if matches!(e, Some(SearchEvent::Done(_)) | None) {
            self.done = true;
        }
        e
    }

    /// Wait for everything: all hits and the summary (`None` if the connection dropped).
    pub async fn collect(mut self) -> (Vec<SearchMatch>, Option<SearchSummary>) {
        let mut hits = Vec::new();
        while let Some(e) = self.next().await {
            match e {
                SearchEvent::Hits(h) => hits.extend(h),
                SearchEvent::Done(s) => return (hits, Some(s)),
            }
        }
        (hits, None)
    }

    /// Ask the server to stop; a `Done` with `cancelled` follows.
    pub fn cancel(&self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let search = self.id;
        rt.spawn(async move {
            let _ = inner.call(Request::SearchCancel { search }).await;
        });
    }
}

impl Drop for SearchStream {
    fn drop(&mut self) {
        if !self.done {
            self.cancel();
            if let Some(inner) = self.inner.upgrade()
                && let Some(c) = inner.current()
            {
                c.searches.lock().unwrap().remove(&self.id);
            }
        }
    }
}

fn expect<T>(
    r: Result<Response, ProtoError>,
    f: impl FnOnce(Response) -> Option<T>,
) -> Result<T, ProtoError> {
    f(r?).ok_or_else(|| ProtoError::new("bad-message").arg("detail", "unexpected response"))
}

fn expect_unit(r: Result<Response, ProtoError>) -> Result<(), ProtoError> {
    expect(r, |r| matches!(r, Response::Unit).then_some(()))
}

fn expect_session(r: Result<Response, ProtoError>) -> Result<SessionInfo, ProtoError> {
    expect(r, |r| match r {
        Response::Session(s) => Some(s),
        _ => None,
    })
}

fn expect_written(r: Result<Response, ProtoError>) -> Result<String, ProtoError> {
    expect(r, |r| match r {
        Response::Written { version } => Some(version),
        _ => None,
    })
}

fn expect_profile(r: Result<Response, ProtoError>) -> Result<ProfileInfo, ProtoError> {
    expect(r, |r| match r {
        Response::Profile(p) => Some(p),
        _ => None,
    })
}

impl Inner {
    fn current(&self) -> Option<Arc<Conn>> {
        self.conn.lock().unwrap().clone()
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn call(&self, req: Request) -> Result<Response, ProtoError> {
        let conn = self.current().ok_or_else(disconnected)?;
        conn.call(self.next_id(), req, self.opts.request_timeout)
            .await
    }

    async fn attach_on(
        self: &Arc<Self>,
        conn: &Arc<Conn>,
        att: &Arc<Attachment>,
    ) -> Result<Attached, ProtoError> {
        let mut st = att.state.lock().await;
        let resp = conn
            .call(
                self.next_id(),
                Request::TermAttach {
                    session: att.session.clone(),
                    from_offset: st.next,
                },
                self.opts.request_timeout,
            )
            .await?;
        let Response::Attached(a) = resp else {
            return Err(ProtoError::new("bad-message").arg("detail", "unexpected response"));
        };
        let stream = conn
            .mux
            .take_incoming(a.stream)
            .ok_or_else(|| ProtoError::internal("terminal stream missing"))?;
        st.epoch += 1;
        let epoch = st.epoch;
        st.next = Some(a.start_offset);
        if a.reset {
            st.clean = true;
            let _ = self
                .term_tx
                .send(ClientEvent::Term(TermChunk {
                    session: att.session.clone(),
                    offset: a.start_offset,
                    reset: true,
                    bytes: Bytes::new(),
                }))
                .await;
        }
        drop(st);
        tokio::spawn(pump(self.clone(), att.clone(), epoch, stream));
        Ok(a)
    }
}

/// Decode a terminal stream and deliver chunks exactly once (trimming overlap by offset).
async fn pump(inner: Arc<Inner>, att: Arc<Attachment>, epoch: u64, mut stream: Stream) {
    let mut dec = ChunkDecoder::default();
    while let Some(data) = stream.recv().await {
        dec.push(&data);
        loop {
            let (offset, reset, bytes) = match dec.next_record() {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("terminal stream {}: {e}", att.session);
                    return;
                }
            };
            let mut st = att.state.lock().await;
            if st.epoch != epoch || att.detached.load(Ordering::Acquire) {
                return;
            }
            let next = st.next.unwrap_or(offset);
            let (offset, bytes, reset) =
                if (reset && !(st.clean && offset == next)) || offset > next {
                    (offset, bytes, true)
                } else {
                    let skip = (next - offset) as usize;
                    if skip >= bytes.len() {
                        continue;
                    }
                    (next, bytes.slice(skip..), false)
                };
            st.next = Some(offset + bytes.len() as u64);
            st.clean = false;
            let chunk = TermChunk {
                session: att.session.clone(),
                offset,
                reset,
                bytes,
            };
            if inner.term_tx.send(ClientEvent::Term(chunk)).await.is_err() {
                return;
            }
        }
    }
}

async fn reader(conn: Arc<Conn>, mut rx: RpcReceiver, ctrl_tx: mpsc::UnboundedSender<ClientEvent>) {
    while let Some(m) = rx.recv().await {
        match RpcMessage::decode(&m) {
            Ok(RpcMessage::Response { id, result }) => {
                let tx = conn.pending.lock().unwrap().remove(&id);
                if let Some(tx) = tx {
                    let _ = tx.send(result);
                }
            }
            Ok(RpcMessage::Notify(n)) => {
                // Search results go to their stream; unknown searches fall through.
                let n = match n {
                    Notify::SearchHits { search, hits } => {
                        let tx = conn.searches.lock().unwrap().get(&search).cloned();
                        match tx {
                            Some(tx) => {
                                let _ = tx.send(SearchEvent::Hits(hits));
                                continue;
                            }
                            None => Notify::SearchHits { search, hits },
                        }
                    }
                    Notify::SearchDone { search, summary } => {
                        let tx = conn.searches.lock().unwrap().remove(&search);
                        match tx {
                            Some(tx) => {
                                let _ = tx.send(SearchEvent::Done(summary));
                                continue;
                            }
                            None => Notify::SearchDone { search, summary },
                        }
                    }
                    other => other,
                };
                let _ = ctrl_tx.send(ClientEvent::Notify(n));
            }
            Ok(RpcMessage::Request { .. }) => {}
            Err(e) => tracing::warn!("bad message from server: {e}"),
        }
    }
    conn.pending.lock().unwrap().clear();
    conn.searches.lock().unwrap().clear();
}

async fn connect_once(inner: &Arc<Inner>) -> Result<(Arc<Conn>, Welcome), ProtoError> {
    let opts = &inner.opts;
    let c = match tokio::time::timeout(opts.connect_timeout, inner.transport.connect()).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            return Err(ProtoError::new("connect-failed").arg("detail", e.to_string()));
        }
        Err(_) => return Err(ProtoError::new("timeout")),
    };
    let (mux, rx) = Mux::new(c.io, MuxConfig::new(Role::Client));
    let conn = Arc::new(Conn {
        mux,
        pending: Mutex::new(HashMap::new()),
        searches: Mutex::new(HashMap::new()),
        _child: Mutex::new(c.child),
    });
    tokio::spawn(reader(conn.clone(), rx, inner.ctrl_tx.clone()));
    let hello = Hello {
        proto_version: PROTO_VERSION,
        client_version: opts.client_version.clone(),
        resume: inner.token.lock().unwrap().clone(),
    };
    let r = conn
        .call(inner.next_id(), Request::Hello(hello), opts.connect_timeout)
        .await;
    let fail = |e: ProtoError| {
        conn.mux.close();
        Err(e)
    };
    match r {
        Ok(Response::Welcome(w)) if w.proto_version == PROTO_VERSION => Ok((conn.clone(), w)),
        Ok(Response::Welcome(w)) => fail(
            ProtoError::new("proto-version")
                .arg("server", w.proto_version.to_string())
                .arg("client", PROTO_VERSION.to_string()),
        ),
        Ok(_) => fail(ProtoError::new("bad-message").arg("detail", "expected welcome")),
        Err(e) => fail(e),
    }
}

async fn run(inner: Arc<Inner>) {
    let mut shutdown = inner.shutdown.subscribe();
    let mut backoff = inner.opts.backoff_min;
    let mut failure_reported = false;
    loop {
        if *shutdown.borrow() {
            break;
        }
        let attempt = tokio::select! {
            r = connect_once(&inner) => r,
            _ = shutdown.wait_for(|s| *s) => break,
        };
        match attempt {
            Ok((conn, welcome)) => {
                backoff = inner.opts.backoff_min;
                failure_reported = false;
                // Offsets are per server instance: after a server restart, replay fully.
                let new_instance = {
                    let mut inst = inner.instance.lock().unwrap();
                    let changed = inst
                        .as_deref()
                        .is_some_and(|i| i != welcome.server_instance);
                    *inst = Some(welcome.server_instance.clone());
                    changed
                };
                let atts: Vec<Arc<Attachment>> = inner
                    .attachments
                    .lock()
                    .unwrap()
                    .values()
                    .cloned()
                    .collect();
                if new_instance {
                    for a in &atts {
                        a.state.lock().await.next = None;
                    }
                }
                *inner.token.lock().unwrap() = Some(welcome.resume_token.clone());
                *inner.conn.lock().unwrap() = Some(conn.clone());
                inner.status.send_modify(|s| {
                    s.connected = Some(welcome.clone());
                    s.attempts += 1;
                    s.last_error = None;
                });
                let _ = inner.ctrl_tx.send(ClientEvent::Connected(welcome));
                let watched: Vec<String> = inner.watched.lock().unwrap().iter().cloned().collect();
                for session in watched {
                    let (inner2, conn2) = (inner.clone(), conn.clone());
                    tokio::spawn(async move {
                        let r = conn2
                            .call(
                                inner2.next_id(),
                                Request::FsWatch {
                                    session: session.clone(),
                                },
                                inner2.opts.request_timeout,
                            )
                            .await;
                        if let Err(e) = r {
                            tracing::debug!("re-watch {session}: {e}");
                        }
                    });
                }
                for a in atts {
                    let (inner2, conn2) = (inner.clone(), conn.clone());
                    tokio::spawn(async move {
                        if let Err(error) = inner2.attach_on(&conn2, &a).await
                            && !a.detached.load(Ordering::Acquire)
                            && !conn2.mux.is_closed()
                        {
                            let _ = inner2.ctrl_tx.send(ClientEvent::AttachFailed {
                                session: a.session.clone(),
                                error,
                            });
                        }
                    });
                }
                tokio::select! {
                    _ = conn.mux.closed() => {}
                    _ = shutdown.wait_for(|s| *s) => {}
                }
                conn.mux.close();
                *inner.conn.lock().unwrap() = None;
                inner.status.send_modify(|s| s.connected = None);
                let _ = inner
                    .ctrl_tx
                    .send(ClientEvent::Disconnected { error: None });
            }
            Err(e) => {
                tracing::debug!("connect to {} failed: {e}", inner.transport.describe());
                inner.status.send_modify(|s| {
                    s.attempts += 1;
                    s.last_error = Some(e.clone());
                });
                if !failure_reported {
                    failure_reported = true;
                    let _ = inner
                        .ctrl_tx
                        .send(ClientEvent::Disconnected { error: Some(e) });
                }
                if !inner.opts.reconnect {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = inner.kick.notified() => {}
                    _ = shutdown.wait_for(|s| *s) => break,
                }
                backoff = (backoff * 2).min(inner.opts.backoff_max);
                continue;
            }
        }
        if !inner.opts.reconnect {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(inner.opts.backoff_min) => {}
            _ = shutdown.wait_for(|s| *s) => break,
        }
    }
    if let Some(c) = inner.conn.lock().unwrap().take() {
        c.mux.close();
    }
}

/// The client's event stream (see [`ClientEvent`]). Connection events and notifications are
/// delivered before queued terminal output. Not reading it throttles terminal streams (the
/// server keeps buffering in its ring) but never blocks RPC.
pub struct ClientEvents {
    ctrl: Option<mpsc::UnboundedReceiver<ClientEvent>>,
    term: Option<mpsc::Receiver<ClientEvent>>,
    slot: EventSlot,
}

impl ClientEvents {
    /// Next event; `None` once the client is gone.
    pub async fn next(&mut self) -> Option<ClientEvent> {
        let ctrl = self.ctrl.as_mut()?;
        let term = self.term.as_mut()?;
        tokio::select! {
            biased;
            Some(e) = ctrl.recv() => Some(e),
            Some(e) = term.recv() => Some(e),
            else => None,
        }
    }

    /// Next event if one is ready.
    pub fn try_next(&mut self) -> Option<ClientEvent> {
        if let Ok(e) = self.ctrl.as_mut()?.try_recv() {
            return Some(e);
        }
        self.term.as_mut()?.try_recv().ok()
    }
}

impl Drop for ClientEvents {
    fn drop(&mut self) {
        if let (Some(c), Some(t)) = (self.ctrl.take(), self.term.take()) {
            *self.slot.lock().unwrap() = Some((c, t));
        }
    }
}
