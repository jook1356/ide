//! Example/test-only stand-in for the server's terminal session: a child in a local ConPTY
//! (portable-pty) whose output is streamed into a [`TermModel`] with byte offsets, exactly as the
//! client does with server chunks. Like the server, it keeps its own hidden alacritty `Term` that
//! sees every output byte and answers terminal queries (DSR/DA/cursor position, colour and size
//! requests) by writing the replies to the PTY; the `TermModel` drops them.
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use ide_term::alacritty_terminal::event::{Event as AlacEvent, EventListener, WindowSize};
use ide_term::alacritty_terminal::term::test::TermSize;
use ide_term::alacritty_terminal::term::{Config as AlacConfig, Term};
use ide_term::alacritty_terminal::vte::ansi::{self, Processor};
use ide_term::{Palette, TermConfig, TermModel};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// What to run in a terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct TermSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

impl TermSpec {
    pub fn new(program: impl Into<String>) -> Self {
        TermSpec {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            cols: 120,
            rows: 30,
        }
    }
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }
    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }
}

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// Listener of the hidden answering Term: writes query replies to the PTY.
#[derive(Clone)]
struct Answerer {
    writer: Writer,
    palette: Arc<Palette>,
    size: Arc<Mutex<(u16, u16)>>,
    replies: Arc<AtomicU64>,
}

impl Answerer {
    fn reply(&self, s: &str) {
        self.replies.fetch_add(1, Ordering::Relaxed);
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(s.as_bytes());
        let _ = w.flush();
    }
}

impl EventListener for Answerer {
    fn send_event(&self, event: AlacEvent) {
        match event {
            AlacEvent::PtyWrite(s) => self.reply(&s),
            AlacEvent::ColorRequest(index, fmt) => {
                let p = &self.palette;
                let c = match index {
                    0..=255 => p.indexed(index as u8),
                    256 => p.foreground,
                    257 => p.background,
                    _ => p.cursor,
                };
                let [r, g, b, _] = c.into_rgba8();
                self.reply(&fmt(ansi::Rgb { r, g, b }));
            }
            AlacEvent::TextAreaSizeRequest(fmt) => {
                let (cols, rows) = *self.size.lock().unwrap();
                // Like the server, the adapter only knows the grid size, not the pixel size.
                self.reply(&fmt(WindowSize {
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

/// A child in a local PTY, streamed into `model`.
pub struct LocalPty {
    pub model: TermModel,
    writer: Writer,
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    size: Arc<Mutex<(u16, u16)>>,
    answer_term: Arc<Mutex<Term<Answerer>>>,
    exit: Arc<Mutex<Option<Option<u32>>>>,
    /// Query replies written by the hidden Term.
    pub replies: Arc<AtomicU64>,
    /// Every byte sent with [`write`](Self::write) while `Some` (test log).
    pub write_log: Mutex<Option<Vec<u8>>>,
}

impl LocalPty {
    pub fn spawn(spec: &TermSpec, config: &TermConfig) -> anyhow::Result<LocalPty> {
        let (cols, rows) = (spec.cols.max(2), spec.rows.max(1));
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
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
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawn {}", spec.program))?;
        drop(pair.slave);
        let pid = child.process_id();
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader().context("pty reader")?;
        let writer: Writer = Arc::new(Mutex::new(pair.master.take_writer().context("pty writer")?));
        let master = Arc::new(Mutex::new(Some(pair.master)));

        let model = TermModel::new(cols, rows, config);
        let size = Arc::new(Mutex::new((cols, rows)));
        let replies = Arc::new(AtomicU64::new(0));
        let answerer = Answerer {
            writer: writer.clone(),
            palette: Arc::new(config.palette.clone()),
            size: size.clone(),
            replies: replies.clone(),
        };
        let alac = AlacConfig {
            scrolling_history: 0,
            ..AlacConfig::default()
        };
        let answer_term = Arc::new(Mutex::new(Term::new(
            alac,
            &TermSize::new(cols as usize, rows as usize),
            answerer,
        )));

        // Answer thread: the hidden Term parses the same stream on its own thread, so it does not
        // slow down the model's feed.
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        {
            let term = answer_term.clone();
            std::thread::Builder::new()
                .name("pty-answer".into())
                .spawn(move || {
                    let mut parser: Processor = Processor::new();
                    while let Ok(chunk) = rx.recv() {
                        parser.advance(&mut *term.lock().unwrap(), &chunk);
                    }
                })?;
        }

        // Reader: PTY bytes -> (offset, bytes) -> model.feed, like stream chunks from the server.
        let reader_done = Arc::new(AtomicBool::new(false));
        {
            let model = model.clone();
            let reader_done = reader_done.clone();
            std::thread::Builder::new()
                .name("pty-reader".into())
                .spawn(move || {
                    let mut buf = vec![0u8; 1 << 16];
                    let mut offset = 0u64;
                    loop {
                        let n = match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        let _ = tx.send(buf[..n].to_vec());
                        model.feed(offset, &buf[..n]);
                        offset += n as u64;
                    }
                    reader_done.store(true, Ordering::Release);
                })?;
        }

        // Waiter: child exit -> close the pseudoconsole (ConPTY keeps the output pipe open until
        // then) -> wait for the reader to drain -> exit status.
        let exit = Arc::new(Mutex::new(None));
        {
            let exit = exit.clone();
            let master = master.clone();
            std::thread::Builder::new()
                .name("pty-wait".into())
                .spawn(move || {
                    let code = child.wait().ok().map(|s| s.exit_code());
                    std::thread::sleep(Duration::from_millis(50));
                    drop(master.lock().unwrap().take());
                    let t = Instant::now();
                    while !reader_done.load(Ordering::Acquire)
                        && t.elapsed() < Duration::from_secs(2)
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    *exit.lock().unwrap() = Some(code);
                })?;
        }

        Ok(LocalPty {
            model,
            writer,
            master,
            killer: Mutex::new(killer),
            pid,
            size,
            answer_term,
            exit,
            replies,
            write_log: Mutex::new(None),
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Input bytes (what `TerminalView::on_input` emits, or a test's direct write).
    pub fn write(&self, bytes: &[u8]) {
        if let Some(log) = self.write_log.lock().unwrap().as_mut() {
            log.extend_from_slice(bytes);
        }
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    /// PTY + answering Term resize (what `TerminalView::on_resize` emits). No-op if unchanged.
    pub fn resize(&self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        {
            let mut s = self.size.lock().unwrap();
            if *s == (cols, rows) {
                return;
            }
            *s = (cols, rows);
        }
        self.answer_term
            .lock()
            .unwrap()
            .resize(TermSize::new(cols as usize, rows as usize));
        if let Some(m) = self.master.lock().unwrap().as_ref() {
            let _ = m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    pub fn kill(&self) {
        let _ = self.killer.lock().unwrap().kill();
    }

    /// `None` while running; `Some(code)` once the child exited and its output was fed.
    pub fn exit_status(&self) -> Option<Option<u32>> {
        *self.exit.lock().unwrap()
    }
}

impl Drop for LocalPty {
    fn drop(&mut self) {
        if self.exit_status().is_none() {
            self.kill();
        }
    }
}
