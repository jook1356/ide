//! One client connection: handshake, request dispatch, notifications and terminal streams.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::BytesMut;
use ide_proto::{
    AgentHello, Attached, Mux, MuxConfig, Notify, PROTO_VERSION, ProtoError, Request, Response,
    Role, RpcMessage, RpcReceiver, SearchSummary, Stream, TextSearch, UserInfo, VERSION, Welcome,
    encode_chunk,
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

    /// `fs.watch`: idempotent per connection, session and repo.
    fn watch(self: &Arc<Self>, session: &str, repo: &str) -> Result<(), ProtoError> {
        let key = crate::watch::key(session, repo);
        if self.watches.lock().unwrap().contains_key(&key) {
            return Ok(());
        }
        let mut rx = self.ctx.watch(session, repo)?;
        let conn = self.clone();
        let sid = session.to_owned();
        let rid = repo.to_owned();
        let task = tokio::spawn(async move {
            loop {
                let n = match rx.recv().await {
                    Ok(n) => n,
                    Err(broadcast::error::RecvError::Lagged(_)) => Notify::FsChanged {
                        session: sid.clone(),
                        repo: rid.clone(),
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
            .insert(key, task.abort_handle());
        if let Some(old) = old {
            // A concurrent watch of the same worktree won the race: keep one reference.
            old.abort();
            self.ctx.unwatch(session, repo);
        }
        Ok(())
    }

    fn unwatch(&self, session: &str, repo: &str) {
        let key = crate::watch::key(session, repo);
        if let Some(h) = self.watches.lock().unwrap().remove(&key) {
            h.abort();
            self.ctx.unwatch(session, repo);
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
            let (roots, prepared) = match prepared {
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
                let refs: Vec<(String, &std::path::Path)> =
                    roots.iter().map(|(r, j)| (r.clone(), j.root())).collect();
                crate::search::run_text(&refs, &prepared, &flag, |hits| {
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
    if let Some(agent) = hello.agent {
        serve_agent(&server, ctx, mux.clone(), rx, id, agent).await;
        return;
    }
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
                "repos",
                "browse-pick",
                "search",
                "git",
                "def",
                // --- pull requests and agent prompts
                "pr",
                "send-prompt",
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
            Request::FsWatch { session, repo } => {
                let r = conn.watch(&session, &repo).map(|_| Response::Unit);
                conn.reply(id, r);
            }
            Request::FsUnwatch { session, repo } => {
                conn.unwatch(&session, &repo);
                conn.reply(id, Ok(Response::Unit));
            }
            Request::SearchText { search, query } => conn.search(id, search, query),
            Request::SearchCancel { search } => {
                if let Some(f) = conn.searches.lock().unwrap().get(&search) {
                    f.store(true, Ordering::Relaxed);
                }
                conn.reply(id, Ok(Response::Unit));
            }
            Request::AgentReply { request, result } => {
                conn.ctx.agent_reply(request, result);
                conn.reply(id, Ok(Response::Unit));
            }
            Request::WidgetCall { session, widget, tool, args } => {
                // Async: an app-carried tool waits for an app's answer.
                let conn = conn.clone();
                tokio::spawn(async move {
                    let r = conn.ctx.widget_call(&session, &widget, &tool, &args).await;
                    conn.reply(id, r.map(Response::AgentResult));
                });
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
    for k in watched {
        if let Some((s, r)) = k.split_once(':') {
            conn.unwatch(s, r);
        }
    }
    for (_, f) in conn.searches.lock().unwrap().drain() {
        f.store(true, Ordering::Relaxed);
    }
    mux.close();
    tracing::info!("client disconnected: user={}", user.id);
}

/// An agent bridge connection (`ide-server mcp` of one session's agent): checked against the
/// session's bridge token, then only `agent.*` requests (and pings), no notifications.
async fn serve_agent(
    server: &Arc<Server>,
    ctx: Arc<UserCtx>,
    mux: Mux,
    mut rx: RpcReceiver,
    id: u64,
    agent: AgentHello,
) {
    let send = |id, result| {
        let _ = mux.send_rpc(&RpcMessage::Response { id, result }.encode());
    };
    if !ctx.agent_check(&agent.session, &agent.token) {
        tracing::warn!("agent bridge: refused a connection for session {}", agent.session);
        send(id, Err(ProtoError::new("unauthorized")));
        mux.close();
        return;
    }
    tracing::info!("agent bridge connected: session {}", agent.session);
    send(
        id,
        Ok(Response::Welcome(Welcome {
            proto_version: PROTO_VERSION,
            server_version: VERSION.to_owned(),
            user: ctx.user().clone(),
            capabilities: vec!["agent".to_owned()],
            resume_token: ide_proto::ResumeToken(String::new()),
            resumed: false,
            server_instance: server.instance().to_owned(),
        })),
    );
    while let Some(msg) = rx.recv().await {
        let Ok(RpcMessage::Request { id, req }) = RpcMessage::decode(&msg) else {
            continue;
        };
        match req {
            Request::Ping => send(id, Ok(Response::Pong)),
            Request::AgentTools => send(id, Ok(Response::AgentTools(crate::bridge::tools()))),
            Request::AgentHook { event } => {
                let deny = ctx.agent_hook(&agent.session, event);
                send(id, Ok(Response::AgentHookDecision(deny)));
            }
            Request::AgentCall { tool, args } => {
                let (ctx, mux, session) = (ctx.clone(), mux.clone(), agent.session.clone());
                tokio::spawn(async move {
                    ctx.audit_action("agent.call", &format!("{session} {tool}"));
                    let r = ctx.agent_call(&session, &tool, &args).await;
                    let result = Ok(Response::AgentResult(r));
                    let _ = mux.send_rpc(&RpcMessage::Response { id, result }.encode());
                });
            }
            other => send(
                id,
                Err(ProtoError::new("bad-request").arg("method", other.method())),
            ),
        }
    }
    mux.close();
    tracing::info!("agent bridge disconnected: session {}", agent.session);
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
    let host = conn.ctx.term_host(session)?;
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
