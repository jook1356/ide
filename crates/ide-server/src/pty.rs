//! Server-side terminal of one session: the PTY process, an `alacritty_terminal::Term` that
//! sees all output in order and answers terminal queries (DSR, DA, colours, size) whether or
//! not a client is attached, and the output ring buffer that attached clients stream from.
//! The PTY is always drained into the ring, so a slow or absent client never blocks the agent.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as AlacEvent, EventListener, WindowSize};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config as AlacConfig, Term};
use alacritty_terminal::vte::ansi::{Processor, Rgb};
use ide_proto::{Notify, ProtoError, SessionState};
use portable_pty::{ChildKiller, MasterPty, PtySize};
use tokio::sync::{broadcast, watch};

use crate::ring::{RING_CAPACITY, Ring, RingRead};
use crate::runner::PtyChild;

type InputSlot = Arc<Mutex<Option<(u64, std_mpsc::Sender<Vec<u8>>)>>>;

/// alacritty event sink: query replies go to the PTY input, title/bell become notifications.
struct Proxy {
    session: String,
    input: InputSlot,
    events: broadcast::Sender<Notify>,
    title: Arc<Mutex<String>>,
    size: Arc<Mutex<(u16, u16)>>,
}

impl Proxy {
    fn pty_write(&self, s: String) {
        if let Some((_, tx)) = self.input.lock().unwrap().as_ref() {
            let _ = tx.send(s.into_bytes());
        }
    }
    fn set_title(&self, t: String) {
        *self.title.lock().unwrap() = t.clone();
        let _ = self.events.send(Notify::Title {
            session: self.session.clone(),
            title: t,
        });
    }
}

impl EventListener for Proxy {
    fn send_event(&self, event: AlacEvent) {
        match event {
            AlacEvent::PtyWrite(s) => self.pty_write(s),
            AlacEvent::Title(t) => self.set_title(t),
            AlacEvent::ResetTitle => self.set_title(String::new()),
            AlacEvent::Bell => {
                let _ = self.events.send(Notify::Bell {
                    session: self.session.clone(),
                });
            }
            AlacEvent::ColorRequest(index, fmt) => {
                let (r, g, b) = palette(index);
                self.pty_write(fmt(Rgb { r, g, b }));
            }
            AlacEvent::TextAreaSizeRequest(fmt) => {
                let (cols, rows) = *self.size.lock().unwrap();
                self.pty_write(fmt(WindowSize {
                    num_lines: rows,
                    num_cols: cols,
                    cell_width: 0,
                    cell_height: 0,
                }));
            }
            _ => {}
        }
    }
}

/// Colours reported for OSC 4/10/11/12 queries: the client's default dark theme (ide-term
/// `Palette::default`).
fn palette(index: usize) -> (u8, u8, u8) {
    const BASE16: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x31, 0x31),
        (0x0d, 0xbc, 0x79),
        (0xe5, 0xe5, 0x10),
        (0x24, 0x72, 0xc8),
        (0xbc, 0x3f, 0xbc),
        (0x11, 0xa8, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x66, 0x66, 0x66),
        (0xf1, 0x4c, 0x4c),
        (0x23, 0xd1, 0x8b),
        (0xf5, 0xf5, 0x43),
        (0x3b, 0x8e, 0xea),
        (0xd6, 0x70, 0xd6),
        (0x29, 0xb8, 0xdb),
        (0xff, 0xff, 0xff),
    ];
    match index {
        0..=15 => BASE16[index],
        16..=231 => {
            let i = (index - 16) as u8;
            let c = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (c(i / 36), c((i / 6) % 6), c(i % 6))
        }
        232..=255 => {
            let v = 8 + (index as u8 - 232) * 10;
            (v, v, v)
        }
        256 => (0xd4, 0xd4, 0xd4),
        257 => (0x26, 0x28, 0x2d),
        _ => (0xd0, 0xd0, 0xd0),
    }
}

struct Core {
    parser: Processor,
    term: Term<Proxy>,
}

/// Row `r` of the grid as text (0 = the screen's top, negative = scrollback), trailing blanks
/// trimmed.
fn row_text(term: &Term<Proxy>, r: i32) -> String {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::Flags;
    let grid = term.grid();
    let row = &grid[Line(r)];
    let mut line: String = (0..grid.columns())
        .map(|c| &row[Column(c)])
        .filter(|cell| !cell.flags.contains(Flags::WIDE_CHAR_SPACER))
        .map(|cell| cell.c)
        .collect();
    line.truncate(line.trim_end().len());
    line
}

struct Proc {
    generation: u64,
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    pid: Option<u32>,
}

/// One session's terminal (survives process restarts; the offsets continue).
pub struct TermHost {
    session: String,
    ring: Mutex<Ring>,
    end_tx: watch::Sender<u64>,
    state_tx: watch::Sender<SessionState>,
    title: Arc<Mutex<String>>,
    core: Mutex<Core>,
    input: InputSlot,
    proc: Mutex<Option<Proc>>,
    size: Arc<Mutex<(u16, u16)>>,
    events: broadcast::Sender<Notify>,
    generation: AtomicU64,
    stopping: AtomicBool,
}

impl TermHost {
    /// A host without a process. `events` receives title/bell/state notifications.
    pub fn new(
        session: &str,
        cols: u16,
        rows: u16,
        events: broadcast::Sender<Notify>,
    ) -> Arc<Self> {
        Self::build(session, cols, rows, events, RING_CAPACITY, 0)
    }

    /// Like [`TermHost::new`], keeping up to `lines` rows that scrolled off the screen
    /// ([`TermHost::history_text`]; user terminals, read by `ide_terminal_read`).
    pub fn with_scrollback(
        session: &str,
        cols: u16,
        rows: u16,
        events: broadcast::Sender<Notify>,
        lines: usize,
    ) -> Arc<Self> {
        Self::build(session, cols, rows, events, RING_CAPACITY, lines)
    }

    /// Like [`TermHost::new`] with a custom ring capacity (tests).
    pub fn with_capacity(
        session: &str,
        cols: u16,
        rows: u16,
        events: broadcast::Sender<Notify>,
        ring_capacity: usize,
    ) -> Arc<Self> {
        Self::build(session, cols, rows, events, ring_capacity, 0)
    }

    fn build(
        session: &str,
        cols: u16,
        rows: u16,
        events: broadcast::Sender<Notify>,
        ring_capacity: usize,
        scrollback: usize,
    ) -> Arc<Self> {
        let cols = cols.max(2);
        let rows = rows.max(1);
        let input: InputSlot = Arc::new(Mutex::new(None));
        let title = Arc::new(Mutex::new(String::new()));
        let size = Arc::new(Mutex::new((cols, rows)));
        let proxy = Proxy {
            session: session.to_owned(),
            input: input.clone(),
            events: events.clone(),
            title: title.clone(),
            size: size.clone(),
        };
        let config = AlacConfig {
            scrolling_history: scrollback,
            ..AlacConfig::default()
        };
        let term = Term::new(config, &TermSize::new(cols as usize, rows as usize), proxy);
        Arc::new(TermHost {
            session: session.to_owned(),
            ring: Mutex::new(Ring::new(ring_capacity)),
            end_tx: watch::channel(0).0,
            state_tx: watch::channel(SessionState::Stopped).0,
            title,
            core: Mutex::new(Core {
                parser: Processor::new(),
                term,
            }),
            input,
            proc: Mutex::new(None),
            size,
            events,
            generation: AtomicU64::new(0),
            stopping: AtomicBool::new(false),
        })
    }

    /// Session id.
    pub fn session(&self) -> &str {
        &self.session
    }

    /// Process state.
    pub fn state(&self) -> SessionState {
        *self.state_tx.borrow()
    }

    /// True while a process runs.
    pub fn is_running(&self) -> bool {
        self.proc.lock().unwrap().is_some()
    }

    /// Process id of the running process.
    pub fn pid(&self) -> Option<u32> {
        self.proc.lock().unwrap().as_ref().and_then(|p| p.pid)
    }

    /// Last OSC title.
    pub fn title(&self) -> String {
        self.title.lock().unwrap().clone()
    }

    /// Current grid size.
    pub fn size(&self) -> (u16, u16) {
        *self.size.lock().unwrap()
    }

    /// Offset just past the newest output byte.
    pub fn end(&self) -> u64 {
        self.ring.lock().unwrap().end()
    }

    /// Watch the end offset (changes whenever output arrives).
    pub fn subscribe_end(&self) -> watch::Receiver<u64> {
        self.end_tx.subscribe()
    }

    /// Watch the process state.
    pub fn subscribe_state(&self) -> watch::Receiver<SessionState> {
        self.state_tx.subscribe()
    }

    /// Where a replay of `from` starts: `(offset, reset)`.
    pub fn resolve(&self, from: Option<u64>) -> (u64, bool) {
        self.ring.lock().unwrap().resolve(from)
    }

    /// Read output from the ring.
    pub fn read(&self, from: u64, max: usize) -> RingRead {
        self.ring.lock().unwrap().read(from, max)
    }

    /// Feed process output: parse (answering queries) and append to the ring.
    pub fn ingest(&self, data: &[u8]) {
        {
            let mut guard = self.core.lock().unwrap();
            let core = &mut *guard;
            core.parser.advance(&mut core.term, data);
        }
        let end = {
            let mut ring = self.ring.lock().unwrap();
            ring.push(data);
            ring.end()
        };
        self.end_tx.send_replace(end);
    }

    /// The visible screen as text, one string per row (trailing blanks trimmed), for screen
    /// rules (`ide_agents::screen`).
    pub fn screen_text(&self) -> Vec<String> {
        use alacritty_terminal::grid::Dimensions;
        let core = self.core.lock().unwrap();
        let rows = core.term.grid().screen_lines() as i32;
        (0..rows).map(|r| row_text(&core.term, r)).collect()
    }

    /// The rows that scrolled off above the screen (oldest first, at most the last `max`), and
    /// how many are kept in all. Empty unless made [`TermHost::with_scrollback`].
    pub fn history_text(&self, max: usize) -> (Vec<String>, usize) {
        use alacritty_terminal::grid::Dimensions;
        let core = self.core.lock().unwrap();
        let total = core.term.grid().history_size();
        let n = total.min(max) as i32;
        ((-n..0).map(|r| row_text(&core.term, r)).collect(), total)
    }

    /// Cursor keys are in application mode (`ESC[?1h`): arrows send `ESC O A` instead of
    /// `ESC [ A`.
    pub fn app_cursor(&self) -> bool {
        self.core
            .lock()
            .unwrap()
            .term
            .mode()
            .contains(alacritty_terminal::term::TermMode::APP_CURSOR)
    }

    /// Write to the process' input.
    pub fn write(&self, bytes: Vec<u8>) -> Result<(), ProtoError> {
        match self.input.lock().unwrap().as_ref() {
            Some((_, tx)) => tx
                .send(bytes)
                .map_err(|_| ProtoError::new("session-not-running").arg("id", &self.session)),
            None => Err(ProtoError::new("session-not-running").arg("id", &self.session)),
        }
    }

    /// The program has bracketed paste on (`ESC[?2004h`): a TUI waiting at its input.
    pub fn bracketed_paste(&self) -> bool {
        self.core
            .lock()
            .unwrap()
            .term
            .mode()
            .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE)
    }

    /// Resize the server-side Term and the PTY (the most recent caller wins).
    pub fn resize(&self, cols: u16, rows: u16) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        {
            let mut s = self.size.lock().unwrap();
            if *s == (cols, rows) {
                return;
            }
            *s = (cols, rows);
        }
        self.core
            .lock()
            .unwrap()
            .term
            .resize(TermSize::new(cols as usize, rows as usize));
        if let Some(p) = self.proc.lock().unwrap().as_ref()
            && let Some(m) = p.master.lock().unwrap().as_ref()
        {
            let _ = m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    /// Start a process: `spawn` runs under the host's process lock (so concurrent starts
    /// cannot both spawn). Fails with `session-running` if a process is running.
    pub fn start(
        self: &Arc<Self>,
        spawn: impl FnOnce(u16, u16) -> Result<PtyChild, ProtoError>,
    ) -> Result<(), ProtoError> {
        let mut proc = self.proc.lock().unwrap();
        if proc.is_some() {
            return Err(ProtoError::new("session-running").arg("id", &self.session));
        }
        let (cols, rows) = self.size();
        let PtyChild {
            mut reader,
            mut writer,
            master,
            mut child,
        } = spawn(cols, rows)?;
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.stopping.store(false, Ordering::Release);
        let pid = child.process_id();
        let killer = child.clone_killer();
        let master = Arc::new(Mutex::new(Some(master)));

        // Input: one writer thread, so neither clients nor query replies ever block on the PTY.
        let (tx, rx) = std_mpsc::channel::<Vec<u8>>();
        *self.input.lock().unwrap() = Some((generation, tx));
        let name = |what: &str| format!("pty-{what}-{}-{generation}", self.session);
        let spawn_thread = |n: String, f: Box<dyn FnOnce() + Send>| {
            std::thread::Builder::new()
                .name(n)
                .spawn(f)
                .map(|_| ())
                .map_err(ProtoError::internal)
        };
        spawn_thread(
            name("in"),
            Box::new(move || {
                while let Ok(b) = rx.recv() {
                    if writer.write_all(&b).and_then(|_| writer.flush()).is_err() {
                        break;
                    }
                }
            }),
        )?;

        let reader_done = Arc::new(AtomicBool::new(false));
        {
            let host = self.clone();
            let reader_done = reader_done.clone();
            spawn_thread(
                name("out"),
                Box::new(move || {
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => host.ingest(&buf[..n]),
                        }
                    }
                    reader_done.store(true, Ordering::Release);
                }),
            )?;
        }

        // Waiter: child exit -> close the pseudoconsole (ConPTY keeps the output pipe open
        // until then) -> let the reader drain -> state change.
        {
            let host = self.clone();
            let master = master.clone();
            spawn_thread(
                name("wait"),
                Box::new(move || {
                    let code = child.wait().ok().map(|s| s.exit_code());
                    std::thread::sleep(Duration::from_millis(50));
                    drop(master.lock().unwrap().take());
                    let t = Instant::now();
                    while !reader_done.load(Ordering::Acquire)
                        && t.elapsed() < Duration::from_secs(2)
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    host.finish(generation, code);
                }),
            )?;
        }

        *proc = Some(Proc {
            generation,
            master,
            killer,
            pid,
        });
        drop(proc);
        self.set_state(SessionState::Running);
        Ok(())
    }

    fn finish(&self, generation: u64, code: Option<u32>) {
        {
            let mut proc = self.proc.lock().unwrap();
            if proc.as_ref().is_some_and(|p| p.generation == generation) {
                *proc = None;
            }
        }
        {
            let mut input = self.input.lock().unwrap();
            if input.as_ref().is_some_and(|(g, _)| *g == generation) {
                *input = None;
            }
        }
        let state = if self.stopping.load(Ordering::Acquire) {
            SessionState::Stopped
        } else {
            SessionState::Exited(code)
        };
        self.set_state(state);
    }

    fn set_state(&self, state: SessionState) {
        self.state_tx.send_replace(state);
        let _ = self.events.send(Notify::State {
            session: self.session.clone(),
            state,
        });
    }

    /// Kill the process (state becomes `Stopped` once it is gone). No-op if not running.
    pub fn kill(&self) {
        if let Some(p) = self.proc.lock().unwrap().as_mut() {
            self.stopping.store(true, Ordering::Release);
            let _ = p.killer.kill();
        }
    }

    /// Kill and wait until the process is gone (up to `timeout`). True if it stopped.
    pub async fn stop(&self, timeout: Duration) -> bool {
        self.kill();
        let mut rx = self.subscribe_state();
        let done = tokio::time::timeout(timeout, async {
            loop {
                if !self.is_running() {
                    return;
                }
                if rx.changed().await.is_err() {
                    return;
                }
            }
        })
        .await;
        done.is_ok() && !self.is_running()
    }

    #[cfg(test)]
    pub(crate) fn set_test_input(&self, tx: std_mpsc::Sender<Vec<u8>>) {
        *self.input.lock().unwrap() = Some((u64::MAX, tx));
    }
}

impl Drop for TermHost {
    fn drop(&mut self) {
        if let Some(p) = self.proc.get_mut().unwrap().as_mut() {
            let _ = p.killer.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_queries_without_a_client() {
        let (ev, mut ev_rx) = broadcast::channel(16);
        let host = TermHost::with_capacity("s1", 80, 24, ev, 1024);
        let (tx, rx) = std_mpsc::channel();
        host.set_test_input(tx);
        host.ingest(b"ab\x1b[6n");
        assert_eq!(rx.try_recv().unwrap(), b"\x1b[1;3R");
        host.ingest(b"\x1b[c");
        assert_eq!(rx.try_recv().unwrap(), b"\x1b[?6c");
        host.ingest(b"\x1b]11;?\x07");
        let reply = String::from_utf8(rx.try_recv().unwrap()).unwrap();
        assert!(reply.starts_with("\x1b]11;rgb:1e1e/1e1e/1e1e"), "{reply:?}");
        host.ingest(b"\x1b]0;hello\x07\x07");
        assert_eq!(host.title(), "hello");
        assert_eq!(
            ev_rx.try_recv().unwrap(),
            Notify::Title {
                session: "s1".into(),
                title: "hello".into()
            }
        );
        assert_eq!(
            ev_rx.try_recv().unwrap(),
            Notify::Bell {
                session: "s1".into()
            }
        );
        // Everything (queries included) is in the ring, offsets monotonic.
        let r = host.read(0, 100);
        assert_eq!(&r.bytes[..6], b"ab\x1b[6n");
        assert_eq!(host.end(), r.bytes.len() as u64);
        // Resize is reflected in the size report.
        host.resize(100, 40);
        host.ingest(b"\x1b[18t");
        assert_eq!(rx.try_recv().unwrap(), b"\x1b[8;40;100t");
    }

    #[test]
    fn write_without_process_fails() {
        let (ev, _) = broadcast::channel(4);
        let host = TermHost::new("s2", 80, 24, ev);
        assert_eq!(
            host.write(b"x".to_vec()).unwrap_err().id,
            "session-not-running"
        );
        assert_eq!(host.state(), SessionState::Stopped);
    }
}
