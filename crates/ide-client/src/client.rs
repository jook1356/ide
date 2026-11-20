//! [`Client`]: RPC, events, terminal attachments, reconnect + resume.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use ide_proto::{
    Attached, ChunkDecoder, Hello, Mux, MuxConfig, MuxError, NewSession, Notify, PROTO_VERSION,
    ProfileInfo, ProtoError, Request, Response, ResumeToken, Role, RpcMessage, RpcReceiver,
    SessionInfo, Stream, TermChunk, Welcome,
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

struct Conn {
    mux: Mux,
    pending: Pending,
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
                let _ = ctrl_tx.send(ClientEvent::Notify(n));
            }
            Ok(RpcMessage::Request { .. }) => {}
            Err(e) => tracing::warn!("bad message from server: {e}"),
        }
    }
    conn.pending.lock().unwrap().clear();
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
