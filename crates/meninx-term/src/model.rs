//! `TermModel`: client-side terminal state. An `alacritty_terminal::Term` fed with the byte stream
//! of a terminal that runs elsewhere (the server owns the PTY). No process, no PTY, no writes:
//! query replies (DSR, DA, colour/size requests) come from the server-side Term, so this model's
//! listener drops them. Many models can coexist; each keeps parsing whether or not a widget shows it.
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use alacritty_terminal::event::{Event as AlacEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config as AlacConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use iced::Subscription;
use iced::futures::channel::mpsc;

use crate::config::TermConfig;

/// Process-unique id of a [`TermModel`].
pub type ModelId = u64;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Notification from a model, produced by [`TermModel::subscription`].
#[derive(Debug, Clone, PartialEq)]
pub struct TermEvent {
    pub id: ModelId,
    pub kind: TermEventKind,
}

/// Local display notifications. Process exit and authoritative title/bell are server events.
#[derive(Debug, Clone, PartialEq)]
pub enum TermEventKind {
    /// New bytes were parsed (or the model was reset). Coalesced: at most one is pending until
    /// [`TermModel::ack`] is called or a `TerminalView` of the model draws.
    Output,
    /// The stream set the window title (OSC 0/2); carries the latest title when delivered.
    /// Coalesced like `Output`. An empty string = title reset.
    Title(String),
    /// BEL. Coalesced like `Output`.
    Bell,
}

/// Grid size in cells plus the cell size in logical pixels (0 until a widget laid it out).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

/// Event sink shared by the alacritty `Term` and the subscription.
pub(crate) struct Notifier {
    id: ModelId,
    tx: Mutex<Option<mpsc::Sender<TermEventKind>>>,
    output_pending: AtomicBool,
    title: Mutex<String>,
    title_pending: AtomicBool,
    title_dirty: AtomicBool,
    bell_pending: AtomicBool,
}

impl Notifier {
    fn send(&self, kind: TermEventKind) -> bool {
        match self.tx.lock().unwrap().as_mut() {
            Some(tx) => tx.try_send(kind).is_ok(),
            None => false,
        }
    }

    fn output(&self) {
        self.flagged(&self.output_pending, TermEventKind::Output);
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

    fn set_title(&self, t: Option<String>) {
        {
            let mut title = self.title.lock().unwrap();
            match t {
                Some(t) => *title = t,
                None => title.clear(),
            }
        }
        self.title_dirty.store(true, Ordering::Release);
        self.flagged(&self.title_pending, TermEventKind::Title(String::new()));
    }

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
    }
}

/// alacritty `EventListener` of a [`TermModel`]: forwards title/bell for local display and drops
/// everything that would write to the PTY (`PtyWrite`, colour/size requests): the server-side
/// Term answers those.
#[derive(Clone)]
pub struct EventProxy {
    notifier: Arc<Notifier>,
    dropped_replies: Arc<AtomicU64>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: AlacEvent) {
        match event {
            AlacEvent::Title(t) => self.notifier.set_title(Some(t)),
            AlacEvent::ResetTitle => self.notifier.set_title(None),
            AlacEvent::Bell => self
                .notifier
                .flagged(&self.notifier.bell_pending, TermEventKind::Bell),
            AlacEvent::PtyWrite(_)
            | AlacEvent::ColorRequest(..)
            | AlacEvent::TextAreaSizeRequest(_) => {
                self.dropped_replies.fetch_add(1, Ordering::Relaxed);
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
    /// Every (cols, rows) the local grid was resized to.
    pub resizes: Vec<(u16, u16)>,
}

/// Feed counters.
#[derive(Debug, Default)]
pub struct Counters {
    /// Bytes parsed (duplicates skipped by `feed` are not counted).
    pub bytes_in: AtomicU64,
    /// Nanoseconds spent in the VT parser.
    pub parse_ns: AtomicU64,
    /// Query replies (`PtyWrite`, colour/size requests) the model dropped.
    pub dropped_replies: Arc<AtomicU64>,
}

/// Parser state and stream position, guarded together so concurrent `feed`s stay ordered.
struct FeedState {
    parser: Processor,
    /// End offset of the bytes applied so far; `None` after `new`/`reset` = accept any offset.
    end: Option<u64>,
}

pub(crate) struct Shared {
    id: ModelId,
    pub(crate) term: FairMutex<Term<EventProxy>>,
    feed: Mutex<FeedState>,
    /// Mirror of `FeedState::end` for lock-free reads (0 = nothing applied).
    last_offset: AtomicU64,
    scrollback: usize,
    proxy: EventProxy,
    pub(crate) size: Mutex<GridSize>,
    pub(crate) notifier: Arc<Notifier>,
    pub(crate) inject: Mutex<VecDeque<Injected>>,
    pub(crate) diag: Mutex<Diagnostics>,
    pub(crate) counters: Counters,
}

/// Client-side terminal state. Cheap to clone (shared handle); `Send + Sync`, so a reader thread
/// may `feed` while the UI thread draws.
#[derive(Clone)]
pub struct TermModel(pub(crate) Arc<Shared>);

impl std::fmt::Debug for TermModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermModel")
            .field("id", &self.0.id)
            .field("last_offset", &self.last_offset())
            .finish()
    }
}

impl TermModel {
    /// An empty terminal of `cols` x `rows` cells. `config.scrollback_lines` sets the history.
    pub fn new(cols: u16, rows: u16, config: &TermConfig) -> TermModel {
        let cols = cols.max(2);
        let rows = rows.max(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let notifier = Arc::new(Notifier {
            id,
            tx: Mutex::new(None),
            output_pending: AtomicBool::new(false),
            title: Mutex::new(String::new()),
            title_pending: AtomicBool::new(false),
            title_dirty: AtomicBool::new(false),
            bell_pending: AtomicBool::new(false),
        });
        let counters = Counters::default();
        let proxy = EventProxy {
            notifier: notifier.clone(),
            dropped_replies: counters.dropped_replies.clone(),
        };
        let term = new_term(config.scrollback_lines, cols, rows, proxy.clone());
        TermModel(Arc::new(Shared {
            id,
            term: FairMutex::new(term),
            feed: Mutex::new(FeedState {
                parser: Processor::new(),
                end: None,
            }),
            last_offset: AtomicU64::new(0),
            scrollback: config.scrollback_lines,
            proxy,
            size: Mutex::new(GridSize {
                cols,
                rows,
                cell_width: 0.0,
                cell_height: 0.0,
            }),
            notifier,
            inject: Mutex::new(VecDeque::new()),
            diag: Mutex::new(Diagnostics::default()),
            counters,
        }))
    }

    pub fn id(&self) -> ModelId {
        self.0.id
    }

    /// Applies a chunk of the server's output stream that starts at byte `offset`. Bytes before
    /// [`last_offset`](Self::last_offset) were already applied and are skipped (replays after a
    /// reconnect may overlap). A gap (`offset > last_offset`) is applied as is: the bytes in
    /// between are lost, which the server avoids by sending `reset` instead. The first feed after
    /// [`new`](Self::new) or [`reset`](Self::reset) accepts any offset.
    pub fn feed(&self, offset: u64, bytes: &[u8]) {
        let end = offset + bytes.len() as u64;
        let mut fs = self.0.feed.lock().unwrap();
        let skip = match fs.end {
            Some(last) if end <= last => return,
            Some(last) if offset < last => (last - offset) as usize,
            _ => 0,
        };
        let data = &bytes[skip..];
        let t = Instant::now();
        {
            let mut term = self.0.term.lock();
            fs.parser.advance(&mut *term, data);
        }
        fs.end = Some(end);
        self.0.last_offset.store(end, Ordering::Release);
        drop(fs);
        let c = &self.0.counters;
        c.parse_ns
            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        c.bytes_in.fetch_add(data.len() as u64, Ordering::Relaxed);
        self.0.notifier.output();
    }

    /// Clears screen, scrollback, modes, title and parser state (the server replays from its ring
    /// start). The grid size is kept; the next `feed` starts fresh at whatever offset it carries.
    pub fn reset(&self) {
        let mut fs = self.0.feed.lock().unwrap();
        {
            let mut term = self.0.term.lock();
            let (cols, rows) = (term.columns() as u16, term.screen_lines() as u16);
            *term = new_term(self.0.scrollback, cols, rows, self.0.proxy.clone());
        }
        fs.parser = Processor::new();
        fs.end = None;
        self.0.last_offset.store(0, Ordering::Release);
        drop(fs);
        if !self.0.notifier.title.lock().unwrap().is_empty() {
            self.0.notifier.set_title(None);
        }
        self.0.notifier.output();
    }

    /// End offset of the stream bytes applied so far (0 after `new`/`reset`): the `from_offset`
    /// to resume from after a reconnect.
    pub fn last_offset(&self) -> u64 {
        self.0.last_offset.load(Ordering::Acquire)
    }

    /// Resizes the local grid only (the PTY is the server's; `TerminalView::on_resize` tells the
    /// app when to resize it). No-op if unchanged.
    pub fn resize(&self, cols: u16, rows: u16) {
        let (cw, ch) = {
            let s = self.0.size.lock().unwrap();
            (s.cell_width, s.cell_height)
        };
        self.resize_cells(cols, rows, cw, ch);
    }

    /// Widget layout: local grid resize plus the measured cell size.
    pub(crate) fn resize_cells(&self, cols: u16, rows: u16, cell_width: f32, cell_height: f32) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        {
            let mut size = self.0.size.lock().unwrap();
            size.cell_width = cell_width;
            size.cell_height = cell_height;
            if size.cols == cols && size.rows == rows {
                return;
            }
            size.cols = cols;
            size.rows = rows;
        }
        self.0
            .term
            .lock()
            .resize(TermSize::new(cols as usize, rows as usize));
        self.0.diag.lock().unwrap().resizes.push((cols, rows));
    }

    /// Current local grid size (and the cell size measured by the last widget layout).
    pub fn grid_size(&self) -> GridSize {
        *self.0.size.lock().unwrap()
    }

    /// Last title set with OSC 0/2 in the stream (empty if none). Local display only.
    pub fn title(&self) -> String {
        self.0.notifier.title.lock().unwrap().clone()
    }

    /// Bytes to send for pasting `text`: newlines become CR, wrapped in bracketed-paste markers
    /// when the program enabled them (`ESC[?2004h`).
    pub fn paste_bytes(&self, text: &str) -> Vec<u8> {
        let bracketed = self
            .0
            .term
            .lock()
            .mode()
            .contains(TermMode::BRACKETED_PASTE);
        let t = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            let t = t.replace("\x1b[201~", "");
            format!("\x1b[200~{t}\x1b[201~").into_bytes()
        } else {
            t.into_bytes()
        }
    }

    /// Notifications for this model (output, title, bell). Keyed by the model id, so batching
    /// one per live model is the intended use. Optional: an app that feeds from its own `update`
    /// redraws anyway.
    pub fn subscription(&self) -> Subscription<TermEvent> {
        Subscription::run_with(SubKey(self.0.notifier.clone()), |key| {
            use iced::futures::StreamExt;
            let (tx, rx) = mpsc::channel(64);
            key.0.attach(tx);
            let notifier = key.0.clone();
            rx.map(move |kind| TermEvent {
                id: notifier.id,
                kind: notifier.deliver(kind),
            })
        })
    }

    /// Re-arms the coalesced `Output`, `Title` and `Bell` notifications (a visible
    /// `TerminalView` also calls it on every draw).
    pub fn ack(&self) {
        self.0.notifier.ack();
    }

    /// Show the newest output (leave the scrollback), as typing does.
    pub fn scroll_to_bottom(&self) {
        self.lock_term().scroll_display(alacritty_terminal::grid::Scroll::Bottom);
    }

    /// Locks the terminal state (grid, modes, selection). Keep the guard short-lived: `feed`
    /// parses under the same lock.
    pub fn lock_term(&self) -> impl std::ops::DerefMut<Target = Term<EventProxy>> + '_ {
        self.0.term.lock()
    }

    /// Visible rows as text (wide-char spacers skipped, trailing spaces trimmed).
    pub fn visible_text(&self) -> Vec<String> {
        visible_rows(&self.0.term.lock())
            .into_iter()
            .map(|cells| {
                cells
                    .into_iter()
                    .map(|c| c.0)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
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
                    return Some((
                        r,
                        (0..n.len())
                            .map(|k| (cells[i + k].1, cells[i + k].2))
                            .collect(),
                    ));
                }
            }
        }
        None
    }

    /// Feed counters (bytes parsed, parse time, dropped query replies).
    pub fn counters(&self) -> &Counters {
        &self.0.counters
    }

    /// Test/benchmark instrumentation.
    pub fn diagnostics(&self) -> MutexGuard<'_, Diagnostics> {
        self.0.diag.lock().unwrap()
    }

    /// Test hook: queues a synthetic iced event that the model's `TerminalView` handles on its
    /// next `update` exactly like a real one (`pos` = widget-local position for mouse events).
    /// The widget drains the queue on its next runtime event (e.g. the redraw that follows any
    /// app message), so send a message after injecting.
    pub fn inject(&self, event: iced::Event, pos: Option<iced::Point>) {
        self.0
            .inject
            .lock()
            .unwrap()
            .push_back(Injected { event, pos });
    }

    pub(crate) fn mark_drawn(&self) {
        self.ack();
    }
}

fn new_term(scrollback: usize, cols: u16, rows: u16, proxy: EventProxy) -> Term<EventProxy> {
    let alac = AlacConfig {
        scrolling_history: scrollback,
        ..AlacConfig::default()
    };
    Term::new(alac, &TermSize::new(cols as usize, rows as usize), proxy)
}

/// Visible rows as (char, column, width) cells.
fn visible_rows(term: &Term<EventProxy>) -> Vec<Vec<(char, usize, usize)>> {
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let mut rows = vec![Vec::new(); term.screen_lines()];
    for cell in content.display_iter {
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let r = (cell.point.line.0 + offset) as usize;
        if r < rows.len() {
            let w = if cell.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> TermModel {
        TermModel::new(20, 4, &TermConfig::default())
    }

    fn history(m: &TermModel) -> usize {
        m.lock_term().grid().history_size()
    }

    #[test]
    fn feed_applies_and_tracks_offset() {
        let m = model();
        m.feed(0, b"hello");
        m.feed(5, b" world");
        assert_eq!(m.last_offset(), 11);
        assert_eq!(m.visible_text()[0], "hello world");
    }

    #[test]
    fn feed_skips_bytes_already_applied() {
        let m = model();
        m.feed(0, b"abc");
        m.feed(0, b"abc"); // full duplicate
        m.feed(1, b"bcdef"); // overlap: only "def" is new
        m.feed(2, b"cd"); // entirely old
        assert_eq!(m.visible_text()[0], "abcdef");
        assert_eq!(m.last_offset(), 6);
        assert_eq!(m.counters().bytes_in.load(Ordering::Relaxed), 6);
    }

    #[test]
    fn overlap_split_inside_escape_sequence() {
        let m = model();
        let s = b"x\x1b[31mred";
        m.feed(0, &s[..4]); // "x ESC [ 3"
        m.feed(2, &s[2..]); // replay from inside the CSI
        assert_eq!(m.visible_text()[0], "xred");
        assert_eq!(m.last_offset(), s.len() as u64);
    }

    #[test]
    fn first_feed_accepts_any_offset_and_gaps_apply() {
        let m = model();
        m.feed(1000, b"late");
        assert_eq!(m.last_offset(), 1004);
        m.feed(2000, b"!");
        assert_eq!(m.visible_text()[0], "late!");
        assert_eq!(m.last_offset(), 2001);
    }

    #[test]
    fn reset_clears_screen_scrollback_parser_and_offset() {
        let m = model();
        let mut off = 0;
        for i in 0..10 {
            let line = format!("line {i}\r\n");
            m.feed(off, line.as_bytes());
            off += line.len() as u64;
        }
        m.feed(off, b"\x1b]0;T\x07\x1b[1"); // title + a half-parsed CSI
        assert!(history(&m) > 0);
        assert_eq!(m.title(), "T");
        m.reset();
        assert_eq!(m.last_offset(), 0);
        assert_eq!(history(&m), 0);
        assert!(m.visible_text().iter().all(|l| l.is_empty()));
        assert_eq!(m.title(), "");
        // Parser state is fresh: "m" is printed, not taken as the end of the old CSI; offsets
        // restart wherever the server's replay starts.
        m.feed(500, b"mfresh");
        assert_eq!(m.visible_text()[0], "mfresh");
        assert_eq!(m.last_offset(), 506);
        m.feed(400, b"old");
        assert_eq!(m.visible_text()[0], "mfresh");
    }

    #[test]
    fn query_replies_are_dropped() {
        let m = model();
        m.feed(0, b"\x1b[6n\x1b[c\x1b]10;?\x07\x1b[14tok");
        assert_eq!(m.visible_text()[0], "ok");
        assert!(m.counters().dropped_replies.load(Ordering::Relaxed) >= 3);
    }

    #[test]
    fn resize_is_local_and_paste_follows_mode() {
        let m = model();
        m.resize(40, 10);
        assert_eq!((m.grid_size().cols, m.grid_size().rows), (40, 10));
        assert_eq!(m.lock_term().columns(), 40);
        assert_eq!(m.paste_bytes("a\nb\r\nc"), b"a\rb\rc");
        m.feed(0, b"\x1b[?2004h");
        assert_eq!(m.paste_bytes("x\ny\x1b[201~"), b"\x1b[200~x\ry\x1b[201~");
    }

    #[test]
    fn model_is_send_sync() {
        fn is<T: Send + Sync>() {}
        is::<TermModel>();
    }
}
