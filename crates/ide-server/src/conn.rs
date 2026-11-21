//! One client connection: handshake, request dispatch, notifications and terminal streams.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::BytesMut;
use ide_proto::{
    Attached, Mux, MuxConfig, Notify, PROTO_VERSION, ProtoError, Request, Response, Role,
    RpcMessage, SearchSummary, Stream, TextSearch, UserInfo, VERSION, Welcome, encode_chunk,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;
use tokio::task::AbortHandle;

use crate::pty::TermHost;
use crate::service::{Server, UserCtx};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const CHUNK: usize = 64 * 1024;

struct Conn {
    mux: Mux,
    ctx: Arc<UserCtx>,
    attachments: Mutex<HashMap<String, AbortHandle>>,
    /// `fs.watch` forwarders of this connection, by session.
    watches: Mutex<HashMap<String, AbortHandle>>,
    /// Running `search.text` of this connection: cancel flags by client search id.
    searches: Mutex<HashMap<u64, Arc<AtomicBool>>>,
}

impl Conn {
    fn reply(&self, id: u64, result: Result<Response, ProtoError>) {
        let r = self
            .mux
            .send_rpc(&RpcMessage::Response { id, result }.encode());
        if let Err(ide_proto::MuxError::TooLarge) = r {
            let result = Err(ProtoError::new("message-too-large"));
            let _ = self
                .mux
                .send_rpc(&RpcMessage::Response { id, result }.encode());
        }
    }

    fn notify(&self, n: Notify) -> bool {
        self.mux.send_rpc(&RpcMessage::Notify(n).encode()).is_ok()
    }

    /// `fs.watch`: idempotent per connection and session.
    fn watch(self: &Arc<Self>, session: &str) -> Result<(), ProtoError> {
        if self.watches.lock().unwrap().contains_key(session) {
            return Ok(());
        }
        let mut rx = self.ctx.watch(session)?;
        let conn = self.clone();
        let sid = session.to_owned();
        let task = tokio::spawn(async move {
            loop {
                let n = match rx.recv().await {
                    Ok(n) => n,
                    Err(broadcast::error::RecvError::Lagged(_)) => Notify::FsChanged {
                        session: sid.clone(),
                        paths: Vec::new(),
                        rescan: true,
                    },
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                if !conn.notify(n) {
                    return;
                }
            }
        });
        let old = self
            .watches
            .lock()
            .unwrap()
            .insert(session.to_owned(), task.abort_handle());
        if let Some(old) = old {
            // A concurrent watch of the same session won the race: keep one reference.
            old.abort();
            self.ctx.unwatch(session);
        }
        Ok(())
    }

    fn unwatch(&self, session: &str) {
        if let Some(h) = self.watches.lock().unwrap().remove(session) {
            h.abort();
            self.ctx.unwatch(session);
        }
    }

    /// `search.text`: validate, reply, then stream hits and a final `SearchDone`.
    fn search(self: &Arc<Self>, id: u64, search: u64, query: TextSearch) {
        let cancel = Arc::new(AtomicBool::new(false));
        let conn = self.clone();
        tokio::spawn(async move {
            let ctx = conn.ctx.clone();
            let q = query.clone();
            let prepared = tokio::task::spawn_blocking(move || ctx.search_prepare(&q))
                .await
                .unwrap_or_else(|e| Err(ProtoError::internal(e)));
            let (jail, prepared) = match prepared {
                Ok(p) => p,
                Err(e) => {
                    conn.reply(id, Err(e));
                    return;
                }
            };
            if let Some(old) = conn.searches.lock().unwrap().insert(search, cancel.clone()) {
                old.store(true, Ordering::Relaxed);
            }
            conn.reply(id, Ok(Response::Unit));
            let c2 = conn.clone();
            let flag = cancel.clone();
            let summary = tokio::task::spawn_blocking(move || {
                crate::search::run_text(jail.root(), &prepared, &flag, |hits| {
                    c2.notify(Notify::SearchHits { search, hits })
                })
            })
            .await
            .unwrap_or_else(|e| SearchSummary {
                matches: 0,
                files: 0,
                searched: 0,
                truncated: false,
                cancelled: false,
                error: Some(ProtoError::internal(e)),
            });
            {
                let mut s = conn.searches.lock().unwrap();
                if s.get(&search).is_some_and(|f| Arc::ptr_eq(f, &cancel)) {
                    s.remove(&search);
                }
            }
            conn.notify(Notify::SearchDone { search, summary });
        });
    }
}

impl Server {
    /// Serve one connection until it closes. `user` is the identity established by the
    /// transport (local socket / stdio: the server's OS user).
    pub async fn serve<T>(self: &Arc<Self>, io: T, user: UserInfo)
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        self.connection_opened();
        serve_inner(self.clone(), io, user).await;
        self.connection_closed();
    }
}

async fn serve_inner<T>(server: Arc<Server>, io: T, user: UserInfo)
where
    T: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mux, mut rx) = Mux::new(io, MuxConfig::new(Role::Server));
    let reply = |id, result| {
        let _ = mux.send_rpc(&RpcMessage::Response { id, result }.encode());
    };

    // Handshake.
    let first = match tokio::time::timeout(HANDSHAKE_TIMEOUT, rx.recv()).await {
        Ok(Some(b)) => b,
        _ => return,
    };
    let (id, hello) = match RpcMessage::decode(&first) {
        Ok(RpcMessage::Request {
            id,
            req: Request::Hello(h),
        }) => (id, h),
        Ok(RpcMessage::Request { id, .. }) => {
            reply(id, Err(ProtoError::new("not-handshaken")));
            mux.close();
            return;
        }
        _ => {
            mux.close();
            return;
        }
    };
    if hello.proto_version != PROTO_VERSION {
        reply(
            id,
            Err(ProtoError::new("proto-version")
                .arg("server", PROTO_VERSION.to_string())
                .arg("client", hello.proto_version.to_string())),
        );
        mux.close();
        return;
    }
    let ctx = match server.authorize(&user) {
        Ok(c) => c,
        Err(e) => {
            reply(id, Err(e));
            mux.close();
            return;
        }
    };
    let (token, resumed) = server.resume(hello.resume.as_ref().map(|t| t.0.as_str()));
    tracing::info!(
        "client connected: user={} client={} resumed={resumed}",
        user.id,
        hello.client_version
    );
    reply(
        id,
        Ok(Response::Welcome(Welcome {
            proto_version: PROTO_VERSION,
            server_version: VERSION.to_owned(),
            user: user.clone(),
            capabilities: [
                "providers",
                "accounts",
                "configurations",
                "sessions",
                "term",
                "files",
                "browse",
                "browse-pick",
                "search",
                "git",
                "def",
            ]
            .map(String::from)
            .to_vec(),
            resume_token: ide_proto::ResumeToken(token),
            resumed,
            server_instance: server.instance().to_owned(),
        })),
    );

    let conn = Arc::new(Conn {
        mux: mux.clone(),
        ctx: ctx.clone(),
        attachments: Mutex::new(HashMap::new()),
        watches: Mutex::new(HashMap::new()),
        searches: Mutex::new(HashMap::new()),
    });
    let forwarder = tokio::spawn(forward_notifications(mux.clone(), ctx.subscribe()));

    while let Some(msg) = rx.recv().await {
        let (id, req) = match RpcMessage::decode(&msg) {
            Ok(RpcMessage::Request { id, req }) => (id, req),
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!("bad message: {e}");
                break;
            }
        };
        match req {
            Request::Hello(_) => conn.reply(id, Err(ProtoError::new("bad-request"))),
            Request::Ping => conn.reply(id, Ok(Response::Pong)),
            Request::TermWrite { session, bytes } => conn.reply(
                id,
                conn.ctx.term_write(&session, bytes).map(|_| Response::Unit),
            ),
            Request::TermResize {
                session,
                cols,
                rows,
            } => conn.reply(
                id,
                conn.ctx
                    .term_resize(&session, cols, rows)
                    .map(|_| Response::Unit),
            ),
            Request::TermAttach {
                session,
                from_offset,
            } => {
                let r = attach(&conn, &session, from_offset);
                conn.reply(id, r.map(Response::Attached));
            }
            Request::ServerShutdown => {
                let (conn, server) = (conn.clone(), server.clone());
                tokio::spawn(async move {
                    conn.ctx.audit_action("server.shutdown", "");
                    let s = server.clone();
                    let _ = tokio::task::spawn_blocking(move || s.stop_all()).await;
                    conn.reply(id, Ok(Response::Unit));
                    tracing::info!("shutdown requested by a client");
                    server.request_shutdown();
                });
            }
            Request::FsWatch { session } => {
                let r = conn.watch(&session).map(|_| Response::Unit);
                conn.reply(id, r);
            }
            Request::FsUnwatch { session } => {
                conn.unwatch(&session);
                conn.reply(id, Ok(Response::Unit));
            }
            Request::SearchText { search, query } => conn.search(id, search, query),
            Request::SearchCancel { search } => {
                if let Some(f) = conn.searches.lock().unwrap().get(&search) {
                    f.store(true, Ordering::Relaxed);
                }
                conn.reply(id, Ok(Response::Unit));
            }
            Request::TermDetach { session } => {
                if let Some(h) = conn.attachments.lock().unwrap().remove(&session) {
                    h.abort();
                }
                conn.reply(id, Ok(Response::Unit));
            }
            other => {
                let conn = conn.clone();
                tokio::spawn(async move {
                    let ctx = conn.ctx.clone();
                    let r = tokio::task::spawn_blocking(move || ctx.handle_blocking(other))
                        .await
                        .unwrap_or_else(|e| Err(ProtoError::internal(e)));
                    conn.reply(id, r);
                });
            }
        }
    }

    forwarder.abort();
    for (_, h) in conn.attachments.lock().unwrap().drain() {
        h.abort();
    }
    let watched: Vec<String> = conn.watches.lock().unwrap().keys().cloned().collect();
    for s in watched {
        conn.unwatch(&s);
    }
    for (_, f) in conn.searches.lock().unwrap().drain() {
        f.store(true, Ordering::Relaxed);
    }
    mux.close();
    tracing::info!("client disconnected: user={}", user.id);
}

async fn forward_notifications(mux: Mux, mut rx: broadcast::Receiver<Notify>) {
    loop {
        tokio::select! {
            _ = mux.closed() => return,
            n = rx.recv() => match n {
                Ok(n) => {
                    if mux.send_rpc(&RpcMessage::Notify(n).encode()).is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Missed events: make the client re-fetch.
                    let _ = mux.send_rpc(&RpcMessage::Notify(Notify::SessionsChanged).encode());
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

fn attach(conn: &Arc<Conn>, session: &str, from: Option<u64>) -> Result<Attached, ProtoError> {
    let s = conn.ctx.resolve_session(session)?;
    let host = conn.ctx.host(&s);
    let (start, reset) = host.resolve(from);
    let stream = conn
        .mux
        .open_stream()
        .map_err(|_| ProtoError::new("disconnected"))?;
    let stream_id = stream.id();
    let task = tokio::spawn(pump(host, stream, start, reset));
    if let Some(old) = conn
        .attachments
        .lock()
        .unwrap()
        .insert(session.to_owned(), task.abort_handle())
    {
        old.abort();
    }
    Ok(Attached {
        session: session.to_owned(),
        stream: stream_id,
        start_offset: start,
        reset,
    })
}

/// Stream the ring from `next` onwards. Reads at the client's pace (credit-limited); if the
/// client falls more than the ring size behind, the next chunk restarts at the ring start
/// with the reset flag.
async fn pump(host: Arc<TermHost>, stream: Stream, mut next: u64, mut reset: bool) {
    let tx = stream.sender();
    let mut end_rx = host.subscribe_end();
    let mut buf = BytesMut::new();
    loop {
        end_rx.borrow_and_update();
        let r = host.read(next, CHUNK);
        if r.bytes.is_empty() {
            tokio::select! {
                ch = end_rx.changed() => if ch.is_err() { break },
                _ = tx.closed() => break,
            }
            continue;
        }
        encode_chunk(r.offset, reset || r.reset, &r.bytes, &mut buf);
        reset = false;
        next = r.offset + r.bytes.len() as u64;
        if tx.send(buf.split().freeze()).await.is_err() {
            break;
        }
    }
    drop(stream);
}
