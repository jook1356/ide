//! `TermSession`: a child process in a ConPTY (portable-pty) whose output is parsed into an
//! `alacritty_terminal::Term` on a reader thread. Many sessions can coexist; each keeps running and
//! parsing whether or not a widget shows it.
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as AlacEvent, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config as AlacConfig, Term};
use alacritty_terminal::vte::ansi::{self, Processor};
use anyhow::Context as _;
use iced::Subscription;
use iced::futures::channel::mpsc;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::config::{Palette, TermConfig};

/// Process-unique id of a [`TermSession`].
pub type SessionId = u64;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// What to run in a terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct TermSpec {
    pub program: String,
    pub args: Vec<String>,
    /// Working directory (`None` = inherit).
    pub cwd: Option<PathBuf>,
    /// Extra environment variables, applied after `TERM=xterm-256color` and `COLORTERM=truecolor`.
    pub env: Vec<(String, String)>,
    /// Initial grid size; the widget resizes the PTY to its own size on first layout.
    pub cols: u16,
    pub rows: u16,
}

impl TermSpec {
    pub fn new(program: impl Into<String>) -> Self {
        TermSpec { program: program.into(), args: Vec::new(), cwd: None, env: Vec::new(), cols: 120, rows: 30 }
    }
    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }
    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }
    pub fn size(mut self, cols: u16, rows: u16) -> Self {
        self.cols = cols;
        self.rows = rows;
        self
    }
}

/// Notification from a session, produced by [`TermSession::subscription`].
#[derive(Debug, Clone, PartialEq)]
pub struct TermEvent {
    pub id: SessionId,
    pub kind: TermEventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TermEventKind {
    /// New output was parsed. Coalesced: at most one is pending until [`TermSession::ack`]
    /// is called or a `TerminalView` of the session draws.
    Output,
    /// The program set the window title (OSC 0/2); carries the latest title when delivered.
    /// Coalesced like `Output` (re-armed by [`TermSession::ack`]). An empty string = title reset.
    Title(String),
    /// BEL. Coalesced like `Output`.
    Bell,
    /// The child exited (exit code if known). All its output has been parsed by now.
    Exited(Option<u32>),
}

/// Grid size in cells plus the cell size in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

/// Event sink shared by the reader thread, the alacritty `Term` and the subscription.
pub(crate) struct Notifier {
    id: SessionId,
    tx: Mutex<Option<mpsc::Sender<TermEventKind>>>,
    output_pending: AtomicBool,
    title: Mutex<String>,
    title_pending: AtomicBool,
    title_dirty: AtomicBool,
    bell_pending: AtomicBool,
    exit: Mutex<Option<Option<u32>>>,
    exited: AtomicBool,
}

impl Notifier {
    fn send(&self, kind: TermEventKind) -> bool {
        match self.tx.lock().unwrap().as_mut() {
            Some(tx) => tx.try_send(kind).is_ok(),
            None => false,
        }
    }

    fn output(&self) {
        if !self.output_pending.swap(true, Ordering::AcqRel) && !self.send(TermEventKind::Output) {
            self.output_pending.store(false, Ordering::Release);
        }
    }

    /// Coalesced flag-based event: sent only if no event of this kind is in flight.
    fn flagged(&self, flag: &AtomicBool, kind: TermEventKind) {
        if !flag.swap(true, Ordering::AcqRel) && !self.send(kind) {
            flag.store(false, Ordering::Release);
        }
    }

    /// Called as an event leaves the subscription stream: fills in the latest title.
    fn deliver(&self, kind: TermEventKind) -> TermEventKind {
        match kind {
            TermEventKind::Title(_) => {
                self.title_dirty.store(false, Ordering::Release);
                TermEventKind::Title(self.title.lock().unwrap().clone())
            }
            k => k,
        }
    }

    fn title_changed(&self) {
        self.title_dirty.store(true, Ordering::Release);
        self.flagged(&self.title_pending, TermEventKind::Title(String::new()));
    }

    /// Re-arms the coalesced Output/Title/Bell notifications; re-sends Title if it changed while
    /// one was in flight.
    fn ack(&self) {
        self.output_pending.store(false, Ordering::Release);
        self.bell_pending.store(false, Ordering::Release);
        self.title_pending.store(false, Ordering::Release);
        if self.title_dirty.load(Ordering::Acquire) {
            self.flagged(&self.title_pending, TermEventKind::Title(String::new()));
        }
    }

    fn attach(&self, tx: mpsc::Sender<TermEventKind>) {
        *self.tx.lock().unwrap() = Some(tx);
        self.output_pending.store(false, Ordering::Release);
        self.title_pending.store(false, Ordering::Release);
        self.bell_pending.store(false, Ordering::Release);
        self.output();
        if self.exited.load(Ordering::Acquire) {
            let code = self.exit.lock().unwrap().flatten();
            self.send(TermEventKind::Exited(code));
        }
    }
}

/// alacritty `EventListener`: answers terminal queries (PtyWrite, colour and size requests) and
/// forwards title/bell to the [`Notifier`].
#[derive(Clone)]
pub struct EventProxy {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    notifier: Arc<Notifier>,
    palette: Arc<Palette>,
    size: Arc<Mutex<GridSize>>,
}

impl EventProxy {
    fn pty_write(&self, s: &str) {
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(s.as_bytes());
        let _ = w.flush();
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: AlacEvent) {
        match event {
            AlacEvent::PtyWrite(s) => self.pty_write(&s),
            AlacEvent::Title(t) => {
                *self.notifier.title.lock().unwrap() = t;
                self.notifier.title_changed();
            }
            AlacEvent::ResetTitle => {
                self.notifier.title.lock().unwrap().clear();
                self.notifier.title_changed();
            }
            AlacEvent::Bell => {
                self.notifier.flagged(&self.notifier.bell_pending, TermEventKind::Bell);
            }
            AlacEvent::ColorRequest(index, fmt) => {
                let p = &self.palette;
                let c = match index {
                    0..=255 => p.indexed(index as u8),
                    256 => p.foreground,
                    257 => p.background,
                    _ => p.cursor,
                };
                let [r, g, b, _] = c.into_rgba8();
                self.pty_write(&fmt(ansi::Rgb { r, g, b }));
            }
            AlacEvent::TextAreaSizeRequest(fmt) => {
                let s = *self.size.lock().unwrap();
                let ws = WindowSize {
                    num_lines: s.rows,
                    num_cols: s.cols,
                    cell_width: s.cell_width.round() as u16,
                    cell_height: s.cell_height.round() as u16,
                };
                self.pty_write(&fmt(ws));
            }
            _ => {}
        }
    }
}

/// A synthetic iced event for the widget (test hook): `pos` is in widget-local logical pixels.
#[derive(Debug, Clone)]
pub struct Injected {
    pub event: iced::Event,
    pub pos: Option<iced::Point>,
}

/// Instrumentation for tests and benchmarks (all optional, cheap when unused).
#[derive(Debug, Default)]
pub struct Diagnostics {
    /// When true, every `draw()` pushes (start instant, draw duration in ms) to `frames`.
    pub measure: bool,
    pub frames: Vec<(Instant, f32)>,
    /// When set, `draw()` checks the visible rows for this text and records the first hit.
    pub probe: Option<String>,
    pub probe_hit: Option<Instant>,
    /// Every IME cursor area the widget requested (logical px), deduplicated.
    pub ime_rects: Vec<iced::Rectangle>,
    /// Every string the widget put on the clipboard.
    pub copies: Vec<String>,
    /// PTY writes made through `TermSession::write`, if `Some` (enable with `Some(Vec::new())`).
    pub write_log: Option<Vec<u8>>,
    /// Every (cols, rows) the session was resized to.
    pub resizes: Vec<(u16, u16)>,
}

pub(crate) struct Shared {
    id: SessionId,
    pub(crate) term: Arc<FairMutex<Term<EventProxy>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    pub(crate) size: Arc<Mutex<GridSize>>,
    pub(crate) notifier: Arc<Notifier>,
    pub(crate) inject: Mutex<VecDeque<Injected>>,
    pub(crate) diag: Mutex<Diagnostics>,
    pub(crate) counters: Arc<Counters>,
}

/// Reader-thread counters.
#[derive(Debug, Default)]
pub struct Counters {
    /// Bytes read from the PTY.
    pub bytes_in: AtomicU64,
    /// Nanoseconds spent in the VT parser.
    pub parse_ns: AtomicU64,
}

impl Drop for Shared {
    fn drop(&mut self) {
        if !self.notifier.exited.load(Ordering::Acquire) {
            let _ = self.killer.lock().unwrap().kill();
        }
    }
}

/// A running terminal session. Cheap to clone (shared handle); the child is killed when the last
/// handle is dropped.
#[derive(Clone)]
pub struct TermSession(pub(crate) Arc<Shared>);

impl std::fmt::Debug for TermSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermSession").field("id", &self.0.id).field("pid", &self.0.pid).finish()
    }
}

impl TermSession {
    /// Spawns `spec` in a new ConPTY. `config.scrollback_lines` and `config.palette` (for colour
    /// queries) are taken from `config`.
    pub fn spawn(spec: &TermSpec, config: &TermConfig) -> anyhow::Result<TermSession> {
        let cols = spec.cols.max(2);
        let rows = spec.rows.max(1);
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("openpty")?;
        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(dir) = &spec.cwd {
            cmd.cwd(dir);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        let mut child = pair.slave.spawn_command(cmd).with_context(|| format!("spawn {}", spec.program))?;
        drop(pair.slave);
        let pid = child.process_id();
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader().context("pty reader")?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer().context("pty writer")?));
        let master = Arc::new(Mutex::new(Some(pair.master)));

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let notifier = Arc::new(Notifier {
            id,
            tx: Mutex::new(None),
            output_pending: AtomicBool::new(false),
            title: Mutex::new(String::new()),
            title_pending: AtomicBool::new(false),
            title_dirty: AtomicBool::new(false),
            bell_pending: AtomicBool::new(false),
            exit: Mutex::new(None),
            exited: AtomicBool::new(false),
        });
        let size = Arc::new(Mutex::new(GridSize { cols, rows, cell_width: 0.0, cell_height: 0.0 }));
        let proxy = EventProxy {
            writer: writer.clone(),
            notifier: notifier.clone(),
            palette: Arc::new(config.palette.clone()),
            size: size.clone(),
        };
        let alac = AlacConfig { scrolling_history: config.scrollback_lines, ..AlacConfig::default() };
        let term = Arc::new(FairMutex::new(Term::new(alac, &TermSize::new(cols as usize, rows as usize), proxy)));
        let counters = Arc::new(Counters::default());

        // Reader: PTY bytes -> VT parser -> Term, then a coalesced Output notification.
        let reader_done = Arc::new(AtomicBool::new(false));
        {
            let term = term.clone();
            let notifier = notifier.clone();
            let counters = counters.clone();
            let reader_done = reader_done.clone();
            std::thread::Builder::new().name(format!("pty-reader-{id}")).spawn(move || {
                let mut parser: Processor = Processor::new();
                let mut buf = vec![0u8; 1 << 16];
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let t = Instant::now();
                    parser.advance(&mut *term.lock(), &buf[..n]);
                    counters.parse_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    counters.bytes_in.fetch_add(n as u64, Ordering::Relaxed);
                    notifier.output();
                }
                reader_done.store(true, Ordering::Release);
                notifier.output();
            })?;
        }

        // Waiter: child exit -> close the pseudoconsole (ConPTY keeps the output pipe open until
        // then) -> wait for the reader to drain -> Exited.
        {
            let notifier = notifier.clone();
            let master = master.clone();
            std::thread::Builder::new().name(format!("pty-wait-{id}")).spawn(move || {
                let code = child.wait().ok().map(|s| s.exit_code());
                *notifier.exit.lock().unwrap() = Some(code);
                std::thread::sleep(Duration::from_millis(50));
                drop(master.lock().unwrap().take());
                let t = Instant::now();
                while !reader_done.load(Ordering::Acquire) && t.elapsed() < Duration::from_secs(2) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                notifier.exited.store(true, Ordering::Release);
                notifier.send(TermEventKind::Exited(code));
            })?;
        }

        Ok(TermSession(Arc::new(Shared {
            id,
            term,
            writer,
            master,
            killer: Mutex::new(killer),
            pid,
            size,
            notifier,
            inject: Mutex::new(VecDeque::new()),
            diag: Mutex::new(Diagnostics::default()),
            counters,
        })))
    }

    pub fn id(&self) -> SessionId {
        self.0.id
    }

    pub fn pid(&self) -> Option<u32> {
        self.0.pid
    }

    /// Notifications for this session (output, title, bell, exit). Keyed by the session id, so
    /// batching one per live session is the intended use.
    pub fn subscription(&self) -> Subscription<TermEvent> {
        Subscription::run_with(SubKey(self.0.notifier.clone()), |key| {
            use iced::futures::StreamExt;
            let (tx, rx) = mpsc::channel(64);
            key.0.attach(tx);
            let notifier = key.0.clone();
            rx.map(move |kind| TermEvent { id: notifier.id, kind: notifier.deliver(kind) })
        })
    }

    /// Re-arms the coalesced `Output`, `Title` and `Bell` notifications: after one of them is
    /// delivered, the next of that kind is only sent after `ack()`. Call it when handling those
    /// events (a visible `TerminalView` also calls it on every draw).
    pub fn ack(&self) {
        self.0.notifier.ack();
    }

    /// Writes raw bytes to the PTY (keyboard input).
    pub fn write(&self, bytes: &[u8]) {
        if let Some(log) = self.0.diag.lock().unwrap().write_log.as_mut() {
            log.extend_from_slice(bytes);
        }
        let mut w = self.0.writer.lock().unwrap();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    /// Pastes text: newlines become CR, wrapped in bracketed-paste markers when the program
    /// enabled them (`ESC[?2004h`).
    pub fn paste(&self, text: &str) {
        let bracketed = self.0.term.lock().mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
        let t = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            let t = t.replace("\x1b[201~", "");
            self.write(format!("\x1b[200~{t}\x1b[201~").as_bytes());
        } else {
            self.write(t.as_bytes());
        }
    }

    /// Resizes the VT grid and the ConPTY. No-op if the grid size is unchanged.
    pub fn resize(&self, cols: u16, rows: u16, cell_width: f32, cell_height: f32) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        {
            let mut size = self.0.size.lock().unwrap();
            let changed = size.cols != cols || size.rows != rows;
            size.cell_width = cell_width;
            size.cell_height = cell_height;
            if !changed {
                return;
            }
            size.cols = cols;
            size.rows = rows;
        }
        self.0.term.lock().resize(TermSize::new(cols as usize, rows as usize));
        if let Some(m) = self.0.master.lock().unwrap().as_ref() {
            let _ = m.resize(PtySize {
                rows,
                cols,
                pixel_width: (cols as f32 * cell_width).round() as u16,
                pixel_height: (rows as f32 * cell_height).round() as u16,
            });
        }
        self.0.diag.lock().unwrap().resizes.push((cols, rows));
    }

    /// Current grid size (and the cell size measured by the last widget layout, 0 before that).
    pub fn grid_size(&self) -> GridSize {
        *self.0.size.lock().unwrap()
    }

    /// Kills the child process. `Exited` follows once it is gone.
    pub fn kill(&self) {
        let _ = self.0.killer.lock().unwrap().kill();
    }

    /// `None` while running; `Some(code)` once the child exited (`code` may be unknown).
    pub fn exit_status(&self) -> Option<Option<u32>> {
        *self.0.notifier.exit.lock().unwrap()
    }

    pub fn is_running(&self) -> bool {
        self.exit_status().is_none()
    }

    /// Last title set with OSC 0/2 (empty if none).
    pub fn title(&self) -> String {
        self.0.notifier.title.lock().unwrap().clone()
    }

    /// Locks the terminal state (grid, modes, selection). Keep the guard short-lived: the reader
    /// thread parses under the same lock.
    pub fn lock_term(&self) -> impl std::ops::DerefMut<Target = Term<EventProxy>> + '_ {
        self.0.term.lock()
    }

    /// Visible rows as text (wide-char spacers skipped, trailing spaces trimmed).
    pub fn visible_text(&self) -> Vec<String> {
        visible_rows(&self.0.term.lock())
            .into_iter()
            .map(|cells| cells.into_iter().map(|c| c.0).collect::<String>().trim_end().to_string())
            .collect()
    }

    /// Finds `needle` in the visible grid, searching bottom-up. Returns the row and, for each
    /// char of `needle`, its (column, width in cells).
    pub fn find(&self, needle: &str) -> Option<(usize, Vec<(usize, usize)>)> {
        let rows = visible_rows(&self.0.term.lock());
        let n: Vec<char> = needle.chars().collect();
        for (r, cells) in rows.iter().enumerate().rev() {
            if cells.len() < n.len() || n.is_empty() {
                continue;
            }
            for i in 0..=cells.len() - n.len() {
                if (0..n.len()).all(|k| cells[i + k].0 == n[k]) {
                    return Some((r, (0..n.len()).map(|k| (cells[i + k].1, cells[i + k].2)).collect()));
                }
            }
        }
        None
    }

    /// Reader counters (bytes in, parse time).
    pub fn counters(&self) -> &Counters {
        &self.0.counters
    }

    /// Test/benchmark instrumentation.
    pub fn diagnostics(&self) -> MutexGuard<'_, Diagnostics> {
        self.0.diag.lock().unwrap()
    }

    /// Test hook: queues a synthetic iced event that the session's `TerminalView` handles on its
    /// next `update` exactly like a real one (`pos` = widget-local position for mouse events).
    /// The widget drains the queue on its next runtime event (e.g. the redraw that follows any
    /// app message), so send a message after injecting.
    pub fn inject(&self, event: iced::Event, pos: Option<iced::Point>) {
        self.0.inject.lock().unwrap().push_back(Injected { event, pos });
    }

    pub(crate) fn mark_drawn(&self) {
        self.ack();
    }
}

/// Visible rows as (char, column, width) cells.
fn visible_rows(term: &Term<EventProxy>) -> Vec<Vec<(char, usize, usize)>> {
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let mut rows = vec![Vec::new(); term.screen_lines()];
    for cell in content.display_iter {
        if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let r = (cell.point.line.0 + offset) as usize;
        if r < rows.len() {
            let w = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
            rows[r].push((cell.c, cell.point.column.0, w));
        }
    }
    rows
}

struct SubKey(Arc<Notifier>);

impl Hash for SubKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
    }
}
