//! Transport-agnostic stream multiplexer over one `AsyncRead + AsyncWrite`.
//!
//! - Stream 0 carries RPC messages (`Rpc` frames; messages above one frame are fragmented:
//!   `stream_id` 1 = fragment, more follows; 0 = last/complete).
//! - Data streams are opened with `Open` (client ids odd, server ids even), carry `Data`
//!   frames under per-stream credit flow control (`Window` frames) and end with `Close`.
//! - Writes are prioritized: RPC/Window/Open frames go before queued Data frames, and data is
//!   bounded by the credit windows, so a stalled stream never blocks RPC or another stream.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Notify, mpsc, watch};

use crate::frame::{self, Frame, FrameKind, MAX_PAYLOAD};

/// Default per-stream credit window.
pub const DEFAULT_WINDOW: u32 = 256 * 1024;
/// Largest RPC message (after fragmentation).
pub const MAX_RPC_MESSAGE: usize = 16 << 20;
const WRITE_BATCH: usize = 256 * 1024;

/// Which end of the connection this is (decides stream id parity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Opens odd stream ids.
    Client,
    /// Opens even stream ids.
    Server,
}

/// Multiplexer settings.
#[derive(Debug, Clone, Copy)]
pub struct MuxConfig {
    /// This end's role.
    pub role: Role,
    /// Receive window granted to the peer per stream.
    pub window: u32,
}

impl MuxConfig {
    /// Default window.
    pub fn new(role: Role) -> Self {
        MuxConfig {
            role,
            window: DEFAULT_WINDOW,
        }
    }
}

/// Multiplexer errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MuxError {
    /// The connection is closed.
    #[error("connection closed")]
    Closed,
    /// This stream was closed (by either side).
    #[error("stream closed")]
    StreamClosed,
    /// Message exceeds [`MAX_RPC_MESSAGE`].
    #[error("message too large")]
    TooLarge,
}

struct SendState {
    credit: Mutex<i64>,
    closed: AtomicBool,
    notify: Notify,
}

impl SendState {
    fn new(credit: u32) -> Arc<Self> {
        Arc::new(SendState {
            credit: Mutex::new(i64::from(credit)),
            closed: AtomicBool::new(false),
            notify: Notify::new(),
        })
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
}

struct Entry {
    data_tx: mpsc::UnboundedSender<Bytes>,
    send: Arc<SendState>,
    /// Bytes the peer may still send before we grant more.
    allowance: Arc<AtomicI64>,
}

struct Shared {
    ctrl: mpsc::UnboundedSender<Vec<Frame>>,
    data: mpsc::UnboundedSender<Frame>,
    streams: Mutex<HashMap<u32, Entry>>,
    incoming: Mutex<HashMap<u32, Stream>>,
    next_id: AtomicU32,
    window: u32,
    role: Role,
    closed_tx: watch::Sender<bool>,
}

impl Shared {
    fn is_closed(&self) -> bool {
        *self.closed_tx.borrow()
    }

    fn shutdown(&self) {
        if self.closed_tx.send_replace(true) {
            return;
        }
        let entries: Vec<Entry> = self
            .streams
            .lock()
            .unwrap()
            .drain()
            .map(|(_, e)| e)
            .collect();
        for e in entries {
            e.send.close();
        }
        let incoming: Vec<Stream> = self
            .incoming
            .lock()
            .unwrap()
            .drain()
            .map(|(_, s)| s)
            .collect();
        drop(incoming);
    }

    /// Remove the stream locally and tell the peer (ordered after its queued data).
    fn close_stream(&self, id: u32) {
        if let Some(e) = self.streams.lock().unwrap().remove(&id) {
            e.send.close();
            let _ = self
                .data
                .send(Frame::new(FrameKind::Close, id, Bytes::new()));
        }
    }
}

fn make_stream(shared: &Arc<Shared>, id: u32, credit: u32) -> (Entry, Stream) {
    let (data_tx, data_rx) = mpsc::unbounded_channel();
    let send = SendState::new(credit);
    let allowance = Arc::new(AtomicI64::new(i64::from(shared.window)));
    let entry = Entry {
        data_tx,
        send: send.clone(),
        allowance: allowance.clone(),
    };
    let stream = Stream {
        tx: StreamSender {
            id,
            shared: shared.clone(),
            state: send,
        },
        rx: StreamReceiver {
            id,
            shared: shared.clone(),
            rx: data_rx,
            allowance,
            consumed: 0,
        },
    };
    (entry, stream)
}

struct Handle(Arc<Shared>);

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// A multiplexed connection. Cheap to clone; the connection closes when the last clone is
/// dropped, on [`Mux::close`], or when the transport fails.
#[derive(Clone)]
pub struct Mux(Arc<Handle>);

impl std::fmt::Debug for Mux {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mux")
            .field("role", &self.0.0.role)
            .field("closed", &self.is_closed())
            .finish()
    }
}

/// Complete RPC messages received on stream 0. Ends when the connection closes.
pub type RpcReceiver = mpsc::UnboundedReceiver<Bytes>;

impl Mux {
    /// Start the reader and writer tasks on the current tokio runtime.
    pub fn new<T>(io: T, config: MuxConfig) -> (Mux, RpcReceiver)
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
        let (data_tx, data_rx) = mpsc::unbounded_channel();
        let (rpc_tx, rpc_rx) = mpsc::unbounded_channel();
        let (closed_tx, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            ctrl: ctrl_tx,
            data: data_tx,
            streams: Mutex::new(HashMap::new()),
            incoming: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(match config.role {
                Role::Client => 1,
                Role::Server => 2,
            }),
            window: config.window.max(1024),
            role: config.role,
            closed_tx,
        });
        let (r, w) = tokio::io::split(io);
        tokio::spawn(reader(r, shared.clone(), rpc_tx));
        tokio::spawn(writer(w, shared.clone(), ctrl_rx, data_rx));
        (Mux(Arc::new(Handle(shared))), rpc_rx)
    }

    fn shared(&self) -> &Arc<Shared> {
        &self.0.0
    }

    /// Send one complete RPC message (fragmented if above one frame).
    pub fn send_rpc(&self, msg: &[u8]) -> Result<(), MuxError> {
        if msg.len() > MAX_RPC_MESSAGE {
            return Err(MuxError::TooLarge);
        }
        let s = self.shared();
        if s.is_closed() {
            return Err(MuxError::Closed);
        }
        let mut frames = Vec::with_capacity(msg.len() / MAX_PAYLOAD + 1);
        let mut rest = msg;
        loop {
            let n = rest.len().min(MAX_PAYLOAD);
            let last = n == rest.len();
            frames.push(Frame::new(
                FrameKind::Rpc,
                if last { 0 } else { 1 },
                Bytes::copy_from_slice(&rest[..n]),
            ));
            rest = &rest[n..];
            if last {
                break;
            }
        }
        s.ctrl.send(frames).map_err(|_| MuxError::Closed)
    }

    /// Open a new outgoing stream. Data can be sent once the peer granted credit.
    pub fn open_stream(&self) -> Result<Stream, MuxError> {
        let s = self.shared();
        if s.is_closed() {
            return Err(MuxError::Closed);
        }
        let id = s.next_id.fetch_add(2, Ordering::Relaxed);
        let (entry, stream) = make_stream(s, id, 0);
        s.streams.lock().unwrap().insert(id, entry);
        s.ctrl
            .send(vec![Frame::new(
                FrameKind::Open,
                id,
                Bytes::copy_from_slice(&s.window.to_be_bytes()),
            )])
            .map_err(|_| MuxError::Closed)?;
        Ok(stream)
    }

    /// Take a stream the peer opened (its id usually arrives in an RPC response). Streams
    /// opened by the peer are buffered (within their window) until taken.
    pub fn take_incoming(&self, id: u32) -> Option<Stream> {
        self.shared().incoming.lock().unwrap().remove(&id)
    }

    /// Close the connection (queued frames are flushed best-effort).
    pub fn close(&self) {
        self.shared().shutdown();
    }

    /// True once closed.
    pub fn is_closed(&self) -> bool {
        self.shared().is_closed()
    }

    /// Resolves when the connection is closed.
    pub async fn closed(&self) {
        let mut rx = self.shared().closed_tx.subscribe();
        let _ = rx.wait_for(|c| *c).await;
    }

    /// Number of open data streams (diagnostics/tests).
    pub fn stream_count(&self) -> usize {
        self.shared().streams.lock().unwrap().len()
    }
}

/// A bidirectional data stream. Dropping it (or its receiver half) closes it.
pub struct Stream {
    tx: StreamSender,
    rx: StreamReceiver,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream").field("id", &self.tx.id).finish()
    }
}

impl Stream {
    /// Stream id.
    pub fn id(&self) -> u32 {
        self.tx.id
    }
    /// See [`StreamSender::send`].
    pub async fn send(&self, data: impl Into<Bytes>) -> Result<(), MuxError> {
        self.tx.send(data).await
    }
    /// See [`StreamReceiver::recv`].
    pub async fn recv(&mut self) -> Option<Bytes> {
        self.rx.recv().await
    }
    /// A sender handle (cloneable).
    pub fn sender(&self) -> StreamSender {
        self.tx.clone()
    }
    /// Split into halves. The stream closes when the receiver is dropped.
    pub fn split(self) -> (StreamSender, StreamReceiver) {
        (self.tx, self.rx)
    }
    /// Close now.
    pub fn close(&self) {
        self.tx.shared.close_stream(self.tx.id);
    }
}

/// Sending half. Only one task should send on a stream at a time (sends are not atomic).
#[derive(Clone)]
pub struct StreamSender {
    id: u32,
    shared: Arc<Shared>,
    state: Arc<SendState>,
}

impl StreamSender {
    /// Stream id.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Send bytes, waiting for credit; split into frames of at most `MAX_PAYLOAD` bytes.
    pub async fn send(&self, data: impl Into<Bytes>) -> Result<(), MuxError> {
        let mut data: Bytes = data.into();
        while !data.is_empty() {
            let n = self.reserve(data.len().min(MAX_PAYLOAD)).await?;
            let part = data.split_to(n);
            self.shared
                .data
                .send(Frame::new(FrameKind::Data, self.id, part))
                .map_err(|_| MuxError::Closed)?;
        }
        Ok(())
    }

    async fn reserve(&self, max: usize) -> Result<usize, MuxError> {
        loop {
            let notified = self.state.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.state.closed.load(Ordering::Acquire) {
                return Err(if self.shared.is_closed() {
                    MuxError::Closed
                } else {
                    MuxError::StreamClosed
                });
            }
            {
                let mut c = self.state.credit.lock().unwrap();
                if *c > 0 {
                    let n = (*c as usize).min(max);
                    *c -= n as i64;
                    return Ok(n);
                }
            }
            notified.await;
        }
    }

    /// Credit currently available (diagnostics/tests).
    pub fn credit(&self) -> i64 {
        *self.state.credit.lock().unwrap()
    }

    /// Resolves when the stream is closed (by either side or with the connection).
    pub async fn closed(&self) {
        loop {
            let notified = self.state.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.state.closed.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// True once closed.
    pub fn is_closed(&self) -> bool {
        self.state.closed.load(Ordering::Acquire)
    }

    /// Close the stream.
    pub fn close(&self) {
        self.shared.close_stream(self.id);
    }
}

/// Receiving half. Consuming data grants credit back to the peer.
pub struct StreamReceiver {
    id: u32,
    shared: Arc<Shared>,
    rx: mpsc::UnboundedReceiver<Bytes>,
    allowance: Arc<AtomicI64>,
    consumed: u32,
}

impl StreamReceiver {
    /// Stream id.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Next piece of data; `None` once the stream or connection is closed (after buffered data).
    pub async fn recv(&mut self) -> Option<Bytes> {
        let b = self.rx.recv().await?;
        self.consumed = self.consumed.saturating_add(b.len() as u32);
        if self.consumed >= self.shared.window / 4 {
            let n = std::mem::take(&mut self.consumed);
            self.allowance.fetch_add(i64::from(n), Ordering::AcqRel);
            let _ = self.shared.ctrl.send(vec![Frame::window(self.id, n)]);
        }
        Some(b)
    }
}

impl Drop for StreamReceiver {
    fn drop(&mut self) {
        self.shared.close_stream(self.id);
    }
}

async fn reader<R: AsyncRead + Send + Unpin + 'static>(
    mut r: R,
    shared: Arc<Shared>,
    rpc_tx: mpsc::UnboundedSender<Bytes>,
) {
    let mut closed = shared.closed_tx.subscribe();
    let mut buf = BytesMut::with_capacity(64 * 1024);
    let mut frag = BytesMut::new();
    'outer: loop {
        loop {
            match frame::decode(&mut buf) {
                Ok(Some(f)) => {
                    if let Err(e) = handle_frame(&shared, &rpc_tx, &mut frag, f) {
                        tracing::warn!("mux protocol error: {e}");
                        break 'outer;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("mux frame error: {e}");
                    break 'outer;
                }
            }
        }
        if buf.capacity() - buf.len() < 16 * 1024 {
            buf.reserve(64 * 1024);
        }
        tokio::select! {
            biased;
            _ = closed.wait_for(|c| *c) => break,
            n = r.read_buf(&mut buf) => match n {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            },
        }
    }
    shared.shutdown();
}

fn handle_frame(
    shared: &Arc<Shared>,
    rpc_tx: &mpsc::UnboundedSender<Bytes>,
    frag: &mut BytesMut,
    f: Frame,
) -> Result<(), String> {
    match f.kind {
        FrameKind::Rpc => {
            if frag.len() + f.payload.len() > MAX_RPC_MESSAGE {
                return Err("rpc message too large".into());
            }
            if f.stream == 1 {
                frag.extend_from_slice(&f.payload);
            } else if frag.is_empty() {
                let _ = rpc_tx.send(f.payload);
            } else {
                frag.extend_from_slice(&f.payload);
                let _ = rpc_tx.send(std::mem::take(frag).freeze());
            }
        }
        FrameKind::Data => {
            let streams = shared.streams.lock().unwrap();
            if let Some(e) = streams.get(&f.stream) {
                let left = e
                    .allowance
                    .fetch_sub(f.payload.len() as i64, Ordering::AcqRel)
                    - f.payload.len() as i64;
                if left < 0 {
                    return Err(format!("stream {} exceeded its window", f.stream));
                }
                let _ = e.data_tx.send(f.payload);
            }
            // Unknown stream: closed locally, data in flight is dropped.
        }
        FrameKind::Window => {
            let n = f.u32_payload().ok_or("short window frame")?;
            if let Some(e) = shared.streams.lock().unwrap().get(&f.stream) {
                *e.send.credit.lock().unwrap() += i64::from(n);
                e.send.notify.notify_waiters();
            }
        }
        FrameKind::Open => {
            let credit = f.u32_payload().ok_or("short open frame")?;
            let peer_parity = match shared.role {
                Role::Client => 0,
                Role::Server => 1,
            };
            if f.stream == 0 || f.stream % 2 != peer_parity {
                return Err(format!("bad stream id {} in open", f.stream));
            }
            let (entry, stream) = make_stream(shared, f.stream, credit);
            {
                let mut streams = shared.streams.lock().unwrap();
                if streams.contains_key(&f.stream) {
                    return Err(format!("stream {} opened twice", f.stream));
                }
                streams.insert(f.stream, entry);
            }
            shared.incoming.lock().unwrap().insert(f.stream, stream);
            let _ = shared
                .ctrl
                .send(vec![Frame::window(f.stream, shared.window)]);
        }
        FrameKind::Close => {
            let e = shared.streams.lock().unwrap().remove(&f.stream);
            if let Some(e) = e {
                e.send.close();
            }
            // An untaken incoming stream closed by the peer: drop it (its Drop is a no-op now).
            let s = shared.incoming.lock().unwrap().remove(&f.stream);
            drop(s);
        }
    }
    Ok(())
}

async fn writer<W: AsyncWrite + Send + Unpin + 'static>(
    mut w: W,
    shared: Arc<Shared>,
    mut ctrl_rx: mpsc::UnboundedReceiver<Vec<Frame>>,
    mut data_rx: mpsc::UnboundedReceiver<Frame>,
) {
    let mut closed = shared.closed_tx.subscribe();
    let mut buf = BytesMut::with_capacity(WRITE_BATCH + 64 * 1024);
    loop {
        tokio::select! {
            biased;
            _ = closed.wait_for(|c| *c) => break,
            Some(frames) = ctrl_rx.recv() => frames.iter().for_each(|f| f.encode(&mut buf)),
            Some(f) = data_rx.recv() => f.encode(&mut buf),
            else => break,
        }
        while buf.len() < WRITE_BATCH {
            if let Ok(frames) = ctrl_rx.try_recv() {
                frames.iter().for_each(|f| f.encode(&mut buf));
            } else if let Ok(f) = data_rx.try_recv() {
                f.encode(&mut buf);
            } else {
                break;
            }
        }
        if w.write_all(&buf).await.is_err() || w.flush().await.is_err() {
            shared.shutdown();
            return;
        }
        buf.clear();
    }
    // Closing: flush what is queued (e.g. an error response before a deliberate close).
    while let Ok(frames) = ctrl_rx.try_recv() {
        frames.iter().for_each(|f| f.encode(&mut buf));
    }
    while let Ok(f) = data_rx.try_recv() {
        if f.kind == FrameKind::Data || f.kind == FrameKind::Close {
            f.encode(&mut buf);
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        let _ = w.write_all(&buf).await;
        let _ = w.flush().await;
        let _ = w.shutdown().await;
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::time::{sleep, timeout};

    fn pair(window: u32) -> ((Mux, RpcReceiver), (Mux, RpcReceiver)) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let c = Mux::new(
            a,
            MuxConfig {
                role: Role::Client,
                window,
            },
        );
        let s = Mux::new(
            b,
            MuxConfig {
                role: Role::Server,
                window,
            },
        );
        (c, s)
    }

    async fn recv_all(s: &mut Stream, n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        while out.len() < n {
            let b = s.recv().await.expect("stream ended early");
            assert!(b.len() <= MAX_PAYLOAD);
            out.extend_from_slice(&b);
        }
        out
    }

    #[tokio::test]
    async fn rpc_round_trip_and_large_rpc() {
        let ((c, mut crx), (s, mut srx)) = pair(DEFAULT_WINDOW);
        c.send_rpc(b"ping").unwrap();
        assert_eq!(&srx.recv().await.unwrap()[..], b"ping");
        let big: Vec<u8> = (0..3 * MAX_PAYLOAD + 17).map(|i| (i % 251) as u8).collect();
        s.send_rpc(&big).unwrap();
        s.send_rpc(b"after").unwrap();
        assert_eq!(crx.recv().await.unwrap(), big);
        assert_eq!(&crx.recv().await.unwrap()[..], b"after");
        assert_eq!(
            c.send_rpc(&vec![0; MAX_RPC_MESSAGE + 1]),
            Err(MuxError::TooLarge)
        );
        drop(s);
        assert!(crx.recv().await.is_none());
        timeout(Duration::from_secs(2), c.closed()).await.unwrap();
        assert_eq!(c.send_rpc(b"x"), Err(MuxError::Closed));
    }

    #[tokio::test]
    async fn large_payload_is_split_and_reassembled() {
        let ((c, _crx), (s, _srx)) = pair(DEFAULT_WINDOW);
        let out = s.open_stream().unwrap();
        assert_eq!(out.id() % 2, 0);
        let data: Vec<u8> = (0..3 * MAX_PAYLOAD + 5).map(|i| (i % 253) as u8).collect();
        let d2 = data.clone();
        let sender = tokio::spawn(async move {
            out.send(d2).await.unwrap();
            out
        });
        let mut inc = loop {
            if let Some(st) = c.take_incoming(2) {
                break st;
            }
            sleep(Duration::from_millis(5)).await;
        };
        let got = timeout(Duration::from_secs(10), recv_all(&mut inc, data.len()))
            .await
            .unwrap();
        assert_eq!(got, data);
        // Client -> server direction on the same stream.
        let mut out = sender.await.unwrap();
        inc.send(Bytes::from_static(b"back")).await.unwrap();
        assert_eq!(&out.recv().await.unwrap()[..], b"back");
        // Close propagates.
        drop(inc);
        timeout(Duration::from_secs(2), out.sender().closed())
            .await
            .unwrap();
        assert!(out.recv().await.is_none());
        assert_eq!(
            out.send(Bytes::from_static(b"x")).await,
            Err(MuxError::StreamClosed)
        );
    }

    #[tokio::test]
    async fn stalled_stream_does_not_block_rpc_or_other_streams() {
        const W: u32 = 64 * 1024;
        let ((c, mut crx), (s, mut srx)) = pair(W);
        let a = s.open_stream().unwrap();
        let b = s.open_stream().unwrap();
        let (a_id, b_id) = (a.id(), b.id());
        let sent_a = Arc::new(AtomicUsize::new(0));
        let total = 2 << 20;
        let pump = |st: Stream, counter: Arc<AtomicUsize>| {
            tokio::spawn(async move {
                let chunk = Bytes::from(vec![1u8; 4096]);
                let mut n = 0;
                while n < total {
                    st.send(chunk.clone()).await.unwrap();
                    n += chunk.len();
                    counter.store(n, Ordering::SeqCst);
                }
                st
            })
        };
        let a_task = pump(a, sent_a.clone());
        let b_task = pump(b, Arc::new(AtomicUsize::new(0)));

        // Never read A for now; read all of B.
        sleep(Duration::from_millis(50)).await;
        let mut a_in = c.take_incoming(a_id).unwrap();
        let mut b_in = c.take_incoming(b_id).unwrap();
        let got = timeout(Duration::from_secs(20), recv_all(&mut b_in, total))
            .await
            .expect("stream B must not be blocked by stalled A");
        assert_eq!(got.len(), total);
        b_task.await.unwrap();

        // RPC still flows both ways while A is stalled.
        for i in 0..20u8 {
            c.send_rpc(&[i]).unwrap();
            let m = timeout(Duration::from_secs(2), srx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&m[..], &[i]);
            s.send_rpc(&[i]).unwrap();
            let m = timeout(Duration::from_secs(2), crx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&m[..], &[i]);
        }
        // A is stuck at exactly one window.
        assert_eq!(sent_a.load(Ordering::SeqCst), W as usize);
        assert!(!a_task.is_finished());

        // Draining A releases the sender.
        let got = timeout(Duration::from_secs(20), recv_all(&mut a_in, total))
            .await
            .unwrap();
        assert_eq!(got.len(), total);
        timeout(Duration::from_secs(5), a_task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn window_violation_closes_connection() {
        let (a, b) = tokio::io::duplex(1 << 20);
        let (m, _rx) = Mux::new(
            a,
            MuxConfig {
                role: Role::Client,
                window: 4096,
            },
        );
        // A raw peer that opens stream 2 and ignores the window.
        let mut raw = b;
        let mut wire = BytesMut::new();
        Frame::new(
            FrameKind::Open,
            2,
            Bytes::copy_from_slice(&1u32.to_be_bytes()),
        )
        .encode(&mut wire);
        Frame::new(FrameKind::Data, 2, vec![0u8; 8192]).encode(&mut wire);
        raw.write_all(&wire).await.unwrap();
        timeout(Duration::from_secs(2), m.closed()).await.unwrap();
    }
}
