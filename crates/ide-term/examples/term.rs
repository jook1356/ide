//! Interactive terminal example + in-app test scripts.
//!
//! `cargo run -p ide-term --example term --release` opens one window with `cmd.exe`
//! (override with `IDE_TERM_SHELL`).
//!
//! The terminal runs through `support/local_pty.rs`, a local stand-in for the server: it owns the
//! PTY, answers terminal queries with its own hidden Term, and streams (offset, bytes) chunks into
//! the `TermModel`; the widget's `on_input`/`on_resize` messages go back to it.
//!
//! Test scripts: `IDE_TERM_SCRIPT=shell|bar|flood|flood-warm|select|ime` drives everything in-app (bytes
//! written to the PTY, synthetic iced events injected into the widget through `TermModel::inject`, iced
//! `window::screenshot` evidence). No OS input is used. Output goes to `IDE_TERM_EVIDENCE`
//! (default `./evidence-term`): `<script>.log` and `<script>_<step>.png`. The flood script runs
//! `python $IDE_TERM_FLOOD 50000` (default `flood.py`).
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iced::advanced::clipboard;
use iced::advanced::input_method;
use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::widget::{column, container, text};
use iced::{Event, Fill, Point, Size, Subscription, Task, Widget, mouse, window};
use ide_term::{TermConfig, TermEvent, TermEventKind, TermModel, TerminalView, ViewEvent};

#[path = "support/local_pty.rs"]
mod local_pty;
use local_pty::{LocalPty, TermSpec};

static LOG: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
static T0: OnceLock<Instant> = OnceLock::new();

fn log(s: &str) {
    let t = T0.get_or_init(Instant::now).elapsed().as_secs_f32();
    let line = format!("[{t:8.3}] {s}\n");
    eprint!("{line}");
    if let Some(f) = LOG.get() {
        let _ = f.lock().unwrap().write_all(line.as_bytes());
    }
}

fn evidence_dir() -> String {
    std::env::var("IDE_TERM_EVIDENCE").unwrap_or_else(|_| "evidence-term".into())
}

#[derive(Debug, Clone)]
enum Message {
    Term(TermEvent),
    View(ViewEvent),
    Input(Vec<u8>),
    Resize(u16, u16),
    Tick,
    WindowId(Option<window::Id>),
    Shot(String, window::Screenshot),
}

#[derive(Clone)]
enum Step {
    Wait(u64),
    WaitText(&'static str, u64),
    Write(String),
    WriteTo(usize, String),
    Type(&'static str),
    Key(Named, Modifiers),
    Ctrl(char),
    CtrlShift(char),
    Ime(input_method::Event),
    /// Simulates the runtime's answer to the widget's clipboard read request.
    ClipboardText(&'static str),
    Wheel(f32),
    Shot(&'static str),
    Resize(f32, f32),
    DumpGrid(&'static str),
    StartMeasure,
    ArmFlood(String),
    WaitFlood(u64),
    StopMeasure(&'static str),
    DragSelect { from: (&'static str, usize, f32), to: (&'static str, usize, f32) },
    DoubleClick(&'static str, usize),
    LogCopies,
    LogWrites(&'static str),
    StartWriteLog,
    Switch(usize),
    WaitExit(u64),
    Exit,
}

struct App {
    config: TermConfig,
    sessions: Vec<LocalPty>,
    active: usize,
    mode: String,
    script: Vec<Step>,
    pc: usize,
    step_start: Instant,
    window: Option<window::Id>,
    status: String,
    pending: usize,
    measure_start: Option<Instant>,
    flood_armed: Option<Instant>,
    exited: Vec<(u64, Option<u32>)>,
}

fn key_event(key: Key, mods: Modifiers, text: Option<&str>) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified),
        location: keyboard::Location::Standard,
        modifiers: mods,
        text: text.map(Into::into),
        repeat: false,
    })
}

fn script(mode: &str) -> Vec<Step> {
    use Step::*;
    let flood = std::env::var("IDE_TERM_FLOOD").unwrap_or_else(|_| "flood.py".into());
    let flood_cmd = format!("python {flood} 50000\r");
    match mode {
        "shell" => vec![
            WaitText(">", 15000),
            Wait(500),
            Shot("01_prompt"),
            Type("title ide-term-title-test"),
            Key(Named::Enter, Modifiers::empty()),
            Write("dir\r".into()),
            Wait(1500),
            Write("python -c \"import sys;[print(f'\\x1b[3{i}mfg{i}\\x1b[0m \\x1b[4{i}m bg{i} \\x1b[0m \\x1b[1;9{i}mbright{i}\\x1b[0m \\x1b[2mdim\\x1b[0m \\x1b[3mitalic\\x1b[0m \\x1b[7minverse\\x1b[0m \\x1b[38;2;{i*30};{255-i*30};128mtruecolor\\x1b[0m 한글{i} 🚀 ┌─┐█▓▒░') for i in range(8)]\"\r".into()),
            Wait(1500),
            Shot("02_dir_colors"),
            Write("for /L %i in (1,1,150) do @echo scrollback line %i\r".into()),
            WaitText("scrollback line 150", 15000),
            Wait(500),
            Wheel(5.0),
            Wait(400),
            DumpGrid("after wheel up 5 notches (15 lines)"),
            Shot("03_scrolled_up"),
            Wheel(-5.0),
            Wait(400),
            Resize(700.0, 420.0),
            Wait(1200),
            Write("cls\r".into()),
            Wait(500),
            Write("mode con\r".into()),
            Wait(1500),
            DumpGrid("after resize 700x420 + mode con"),
            Shot("04_resized_mode_con"),
            Resize(1000.0, 700.0),
            Wait(1200),
            Write("mode con\r".into()),
            Wait(1500),
            DumpGrid("after resize 1000x700 + mode con"),
            Shot("05_resized_back"),
            // Second session runs in the background while session 0 is shown.
            WriteTo(1, "for /L %i in (1,1,300) do @echo background line %i\r".into()),
            Wait(2500),
            Switch(1),
            WaitText("background line 300", 10000),
            Wait(500),
            DumpGrid("background session after switch"),
            Shot("06_background_session"),
            Switch(0),
            Wait(500),
            Write("exit\r".into()),
            WaitExit(5000),
            Exit,
        ],
        // Foreground quads over a background run: a progress bar (`█` cells on a coloured track, as
        // Claude Code's usage meter draws it) and underline / strike on a background.
        "bar" => vec![
            WaitText(">", 15000),
            Write("cls\r".into()),
            Wait(500),
            Write("powershell -NoProfile -c \"$e=[char]27; $b=[string][char]0x2588; Write-Host ($e+'[38;5;75;48;5;239m'+($b*10)+(' '*30)+$e+'[0m 25% used'); Write-Host ($e+'[4;48;5;239munderline on bg'+$e+'[0m '+$e+'[9;48;5;239mstrike on bg'+$e+'[0m')\"\r".into()),
            Wait(4000), // the echoed command already contains the marker text
            Shot("01_bar"),
            Write("exit\r".into()),
            WaitExit(5000),
            Exit,
        ],
        "flood" | "flood-warm" => vec![
            WaitText(">", 15000),
            Wait(800),
            // flood-warm: show 60 flood lines before measuring (first use of glyphs/fonts is costly)
            Write(if mode == "flood-warm" { format!("python {flood} 60\r") } else { "\r".into() }),
            Wait(800),
            Write("cls\r".into()),
            Wait(800),
            StartMeasure,
            ArmFlood(flood_cmd),
            WaitFlood(600_000),
            StopMeasure("flood"),
            Wait(300),
            Shot("01_flood_done"),
            DumpGrid("after flood"),
            StartMeasure,
            Wait(3000),
            StopMeasure("idle-after-flood"),
            Wheel(10.0),
            Wait(500),
            Shot("02_flood_scrolled_up"),
            Exit,
        ],
        "select" => vec![
            WaitText(">", 15000),
            Wait(500),
            Write("cls\r".into()),
            Wait(500),
            Write("python -c \"print('sel-1 alpha 한글 선택 beta   \\nsel-2 둘째 줄 wide 글자 끝\\nsel-3 third 셋째 줄 end   ')\"\r".into()),
            WaitText("sel-3 third", 10000),
            Wait(500),
            // start on the right half of '한', end on the left half of '째' in row 3
            DragSelect { from: ("한글", 0, 0.75), to: ("셋째", 1, 0.25) },
            Wait(400),
            Shot("01_selection_highlight"),
            Ctrl('c'),
            Wait(300),
            // left half of '한' to past the end of row 3 (trailing spaces trimmed)
            DragSelect { from: ("한글", 0, 0.25), to: ("end", 2, 6.0) },
            Wait(400),
            Shot("02_selection2_highlight"),
            CtrlShift('c'),
            Wait(300),
            DoubleClick("alpha", 2),
            Wait(400),
            Shot("03_double_click_word"),
            Ctrl('c'),
            Wait(300),
            LogCopies,
            // Ctrl+C without a selection must send 0x03 to the PTY.
            StartWriteLog,
            Ctrl('c'),
            Wait(300),
            LogWrites("ctrl+c without selection"),
            // Paste, plain (cmd does not enable bracketed paste)
            Write("cls\r".into()),
            Wait(400),
            StartWriteLog,
            Ctrl('v'),
            ClipboardText("echo pasted-plain\nX"),
            Wait(600),
            LogWrites("ctrl+v plain"),
            Key(Named::Escape, Modifiers::empty()),
            Wait(300),
            // Paste into a program that enabled bracketed paste (ESC[?2004h)
            Write("python -c \"import sys;sys.stdout.write('\\x1b[?2004hBP> ');sys.stdout.flush();s=input();print();print('GOT', repr(s))\"\r".into()),
            WaitText("BP>", 10000),
            Wait(400),
            StartWriteLog,
            CtrlShift('v'),
            ClipboardText("bp-line1\nbp-line2"),
            Wait(400),
            LogWrites("ctrl+shift+v bracketed"),
            Key(Named::Enter, Modifiers::empty()),
            Wait(800),
            DumpGrid("after paste"),
            Shot("04_paste"),
            Exit,
        ],
        "ime" => {
            use input_method::Event as I;
            let p = |s: &str| Ime(I::Preedit(s.to_string(), Some(s.len()..s.len())));
            vec![
                WaitText(">", 15000),
                Wait(500),
                Write("cls\r".into()),
                Wait(500),
                Write("python -c \"s=input('IME> ');print('GOT', repr(s), s.encode('utf-8').hex())\"\r".into()),
                WaitText("IME>", 10000),
                Wait(300),
                StartWriteLog,
                Ime(I::Opened),
                p("ㅎ"),
                Wait(300),
                p("하"),
                Wait(300),
                p("한"),
                Wait(400),
                Shot("01_preedit_han"),
                // Backspace during composition must not reach the PTY.
                Key(Named::Backspace, Modifiers::empty()),
                p("하"),
                Wait(300),
                Shot("02_preedit_after_backspace"),
                p("한"),
                Ime(I::Preedit(String::new(), None)),
                Ime(I::Commit("한".into())),
                p("ㄱ"),
                Wait(300),
                p("그"),
                Wait(300),
                p("글"),
                Wait(400),
                Shot("03_committed_han_preedit_geul"),
                Ime(I::Preedit(String::new(), None)),
                Ime(I::Commit("글".into())),
                Key(Named::Enter, Modifiers::empty()),
                WaitText("GOT", 10000),
                Wait(300),
                LogWrites("IME"),
                DumpGrid("after IME"),
                Shot("04_pty_received"),
                Exit,
            ]
        }
        _ => vec![],
    }
}

impl App {
    fn boot() -> (App, Task<Message>) {
        let mode = std::env::var("IDE_TERM_SCRIPT").unwrap_or_default();
        if !mode.is_empty() {
            let dir = evidence_dir();
            let _ = std::fs::create_dir_all(&dir);
            let f = OpenOptions::new().create(true).write(true).truncate(true).open(format!("{dir}/{mode}.log")).unwrap();
            let _ = LOG.set(Mutex::new(f));
        }
        let shell = std::env::var("IDE_TERM_SHELL").unwrap_or_else(|_| "cmd.exe".into());
        let config = TermConfig::default();
        let mut spec = TermSpec::new(&shell).env("PYTHONIOENCODING", "utf-8");
        if let Ok(dir) = std::env::current_dir() {
            spec = spec.cwd(dir);
        }
        let n = if mode == "shell" { 2 } else { 1 };
        let sessions: Vec<_> = (0..n).map(|_| LocalPty::spawn(&spec, &config).expect("spawn")).collect();
        for s in &sessions {
            log(&format!("mode={mode:?} shell={shell} model {} pid {:?}", s.model.id(), s.pid()));
        }
        let app = App {
            config,
            sessions,
            active: 0,
            script: script(&mode),
            mode,
            pc: 0,
            step_start: Instant::now(),
            window: None,
            status: String::new(),
            pending: 0,
            measure_start: None,
            flood_armed: None,
            exited: Vec::new(),
        };
        (app, window::latest().map(Message::WindowId))
    }

    fn pty(&self) -> &LocalPty {
        &self.sessions[self.active]
    }

    fn s(&self) -> &TermModel {
        &self.pty().model
    }

    fn inject(&self, event: Event, pos: Option<Point>) {
        self.s().inject(event, pos);
    }

    fn cell_point(&self, row: usize, col: usize, frac: f32) -> Point {
        let g = self.s().grid_size();
        let pad = self.config.padding;
        Point::new(pad + (col as f32 + frac) * g.cell_width, pad + (row as f32 + 0.5) * g.cell_height)
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Term(ev) => {
                if let Some(s) = self.sessions.iter().find(|s| s.model.id() == ev.id) {
                    s.model.ack();
                }
                match &ev.kind {
                    TermEventKind::Output => {}
                    other => log(&format!("model {} event {other:?}", ev.id)),
                }
                Task::none()
            }
            Message::Input(bytes) => {
                self.pty().write(&bytes);
                Task::none()
            }
            Message::Resize(cols, rows) => {
                log(&format!("on_resize {cols}x{rows} -> pty of model {}", self.s().id()));
                self.pty().resize(cols, rows);
                Task::none()
            }
            Message::View(ev) => {
                log(&format!("view event {ev:?}"));
                Task::none()
            }
            Message::WindowId(id) => {
                self.window = id;
                Task::none()
            }
            Message::Shot(name, shot) => {
                let path = format!("{}/{}_{}.png", evidence_dir(), self.mode, name);
                let f = std::fs::File::create(&path).unwrap();
                let mut enc = png::Encoder::new(std::io::BufWriter::new(f), shot.size.width, shot.size.height);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                enc.write_header().unwrap().write_image_data(&shot.rgba).unwrap();
                log(&format!("screenshot {path} {}x{} scale {}", shot.size.width, shot.size.height, shot.scale_factor));
                self.pending -= 1;
                Task::none()
            }
            Message::Tick => {
                for s in &self.sessions {
                    let id = s.model.id();
                    if let Some(code) = s.exit_status()
                        && !self.exited.iter().any(|e| e.0 == id)
                    {
                        log(&format!("model {id} EXITED code {code:?}"));
                        self.exited.push((id, code));
                    }
                }
                if self.mode.is_empty() && self.exited.len() == self.sessions.len() {
                    return iced::exit();
                }
                let g = self.s().grid_size();
                self.status = format!(
                    "{} session {}/{}  grid {}x{}  in {:.1} MB  title {:?}",
                    if self.mode.is_empty() { "interactive" } else { &self.mode },
                    self.active + 1,
                    self.sessions.len(),
                    g.cols,
                    g.rows,
                    self.s().counters().bytes_in.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6,
                    self.s().title()
                );
                self.run_script()
            }
        }
    }

    fn run_script(&mut self) -> Task<Message> {
        let mut tasks = vec![];
        loop {
            if self.pending > 0 {
                return Task::batch(tasks);
            }
            let Some(step) = self.script.get(self.pc).cloned() else { return Task::batch(tasks) };
            let elapsed = self.step_start.elapsed();
            let mut advance = true;
            match step {
                Step::Wait(ms) => advance = elapsed >= Duration::from_millis(ms),
                Step::WaitText(t, ms) => {
                    let found = self.s().visible_text().iter().any(|l| l.contains(t));
                    advance = found || elapsed >= Duration::from_millis(ms);
                    if advance && !found {
                        log(&format!("TIMEOUT waiting for {t:?}"));
                    }
                }
                Step::Write(s) => {
                    log(&format!("pty write {s:?}"));
                    self.pty().write(s.as_bytes());
                }
                Step::WriteTo(i, s) => {
                    log(&format!("pty write to session index {i}: {s:?}"));
                    self.sessions[i].write(s.as_bytes());
                }
                Step::Type(s) => {
                    for ch in s.chars() {
                        let t = ch.to_string();
                        self.inject(key_event(Key::Character(t.as_str().into()), Modifiers::empty(), Some(&t)), None);
                    }
                }
                Step::Key(n, m) => self.inject(key_event(Key::Named(n), m, None), None),
                Step::Ctrl(c) => {
                    let t = c.to_string();
                    self.inject(key_event(Key::Character(t.as_str().into()), Modifiers::CTRL, None), None);
                }
                Step::CtrlShift(c) => {
                    let t = c.to_string();
                    self.inject(key_event(Key::Character(t.as_str().into()), Modifiers::CTRL | Modifiers::SHIFT, None), None);
                }
                Step::Ime(ev) => self.inject(Event::InputMethod(ev), None),
                Step::ClipboardText(t) => {
                    log(&format!("clipboard read answered with {t:?}"));
                    self.inject(
                        Event::Clipboard(clipboard::Event::Read(Ok(Arc::new(clipboard::Content::Text(t.to_string()))))),
                        None,
                    );
                }
                Step::Wheel(y) => {
                    let p = self.cell_point(5, 10, 0.5);
                    self.inject(Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Lines { x: 0.0, y } }), Some(p));
                }
                Step::Shot(name) => {
                    if let Some(id) = self.window {
                        self.pending += 1;
                        let name = name.to_string();
                        tasks.push(window::screenshot(id).map(move |s| Message::Shot(name.clone(), s)));
                    }
                }
                Step::Resize(w, h) => {
                    if let Some(id) = self.window {
                        log(&format!("window resize to {w}x{h}"));
                        tasks.push(window::resize(id, Size::new(w, h)));
                    }
                }
                Step::DumpGrid(what) => {
                    let g = self.s().grid_size();
                    let off = self.s().lock_term().grid().display_offset();
                    let mut s = format!(
                        "GRID {}x{} cell {:.3}x{:.3} display_offset {off} ({what}) resizes={:?}\n",
                        g.cols,
                        g.rows,
                        g.cell_width,
                        g.cell_height,
                        self.s().diagnostics().resizes
                    );
                    for (i, l) in self.s().visible_text().iter().enumerate() {
                        s.push_str(&format!("  {i:02}|{l}\n"));
                    }
                    log(&s);
                }
                Step::StartMeasure => {
                    let mut d = self.s().diagnostics();
                    d.measure = true;
                    d.frames.clear();
                    drop(d);
                    self.measure_start = Some(Instant::now());
                    self.s().counters().parse_ns.store(0, std::sync::atomic::Ordering::Relaxed);
                    self.s().counters().bytes_in.store(0, std::sync::atomic::Ordering::Relaxed);
                }
                Step::ArmFlood(cmd) => {
                    {
                        let mut d = self.s().diagnostics();
                        d.probe = Some("FLOOD DONE".into());
                        d.probe_hit = None;
                    }
                    self.flood_armed = Some(Instant::now());
                    log(&format!("flood start: pty write {cmd:?}"));
                    self.pty().write(cmd.as_bytes());
                }
                Step::WaitFlood(ms) => {
                    advance = self.s().diagnostics().probe_hit.is_some() || elapsed >= Duration::from_millis(ms);
                }
                Step::StopMeasure(what) => {
                    let mut d = self.s().diagnostics();
                    d.measure = false;
                    let wall = self.measure_start.unwrap().elapsed().as_secs_f64();
                    let frames = std::mem::take(&mut d.frames);
                    let flood = match (self.flood_armed, d.probe_hit) {
                        (Some(a), Some(h)) => Some(h.duration_since(a)),
                        _ => None,
                    };
                    drop(d);
                    let t_start = self.measure_start.unwrap();
                    let long: Vec<String> = frames
                        .windows(2)
                        .filter(|w| w[1].0.duration_since(w[0].0).as_millis() > 40)
                        .map(|w| format!("{:.0}ms@{:.0}", w[1].0.duration_since(w[0].0).as_secs_f32() * 1000.0, w[0].0.duration_since(t_start).as_secs_f32() * 1000.0))
                        .collect();
                    log(&format!("long frame gaps (gap@start_ms): {long:?}"));
                    let mut iv: Vec<f32> = frames.windows(2).map(|w| w[1].0.duration_since(w[0].0).as_secs_f32() * 1000.0).collect();
                    iv.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let mut dr: Vec<f32> = frames.iter().map(|f| f.1).collect();
                    dr.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let pct = |v: &Vec<f32>, p: f32| if v.is_empty() { 0.0 } else { v[((v.len() - 1) as f32 * p) as usize] };
                    let c = self.s().counters();
                    let bytes = c.bytes_in.load(std::sync::atomic::Ordering::Relaxed);
                    let parse = c.parse_ns.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e9;
                    let replies = self.pty().replies.load(std::sync::atomic::Ordering::Relaxed);
                    let dropped = c.dropped_replies.load(std::sync::atomic::Ordering::Relaxed);
                    log(&format!("query replies: adapter answered {replies}, model dropped {dropped}"));
                    log(&format!(
                        "STATS[{what}] wall {wall:.2}s flood_done {flood:?} frames {} avg_fps {:.1} frame_interval p50 {:.1} p95 {:.1} max {:.1} ms | draw() p50 {:.2} p95 {:.2} max {:.2} ms | pty bytes {:.2} MB, parse {parse:.3}s",
                        frames.len(),
                        frames.len() as f64 / wall,
                        pct(&iv, 0.5),
                        pct(&iv, 0.95),
                        iv.last().copied().unwrap_or(0.0),
                        pct(&dr, 0.5),
                        pct(&dr, 0.95),
                        dr.last().copied().unwrap_or(0.0),
                        bytes as f64 / 1e6,
                    ));
                }
                Step::DragSelect { from, to } => match (self.s().find(from.0), self.s().find(to.0)) {
                    (Some((ra, ca)), Some((rb, cb))) => {
                        let (c0, w0) = ca[from.1];
                        let (c1, w1) = cb[to.1];
                        let p0 = self.cell_point(ra, c0, from.2 * w0 as f32);
                        let p1 = self.cell_point(rb, c1, to.2 * w1 as f32);
                        log(&format!("drag from row {ra} col {c0} {p0:?} to row {rb} col {c1} {p1:?}"));
                        self.inject(Event::Mouse(mouse::Event::CursorMoved { position: p0 }), Some(p0));
                        self.inject(Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)), Some(p0));
                        for i in 1..=8 {
                            let f = i as f32 / 8.0;
                            let p = Point::new(p0.x + (p1.x - p0.x) * f, p0.y + (p1.y - p0.y) * f);
                            self.inject(Event::Mouse(mouse::Event::CursorMoved { position: p }), Some(p));
                        }
                        self.inject(Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)), Some(p1));
                    }
                    _ => log("DragSelect: text not found"),
                },
                Step::DoubleClick(word, k) => match self.s().find(word) {
                    Some((r, cells)) => {
                        let (c, _) = cells[k];
                        let p = self.cell_point(r, c, 0.5);
                        log(&format!("double click on {word:?} row {r} col {c} {p:?}"));
                        for _ in 0..2 {
                            self.inject(Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)), Some(p));
                            self.inject(Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)), Some(p));
                        }
                    }
                    None => log("DoubleClick: text not found"),
                },
                Step::LogCopies => {
                    let copies = self.s().diagnostics().copies.clone();
                    for (i, c) in copies.iter().enumerate() {
                        log(&format!("COPY #{} (string passed to the clipboard write) = {c:?}", i + 1));
                    }
                }
                Step::StartWriteLog => *self.pty().write_log.lock().unwrap() = Some(Vec::new()),
                Step::LogWrites(what) => {
                    let w = self.pty().write_log.lock().unwrap().take().unwrap_or_default();
                    log(&format!(
                        "PTY WRITES ({what}): {:?}\n  hex: {}",
                        String::from_utf8_lossy(&w),
                        w.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
                    ));
                }
                Step::Switch(i) => {
                    log(&format!("switch view to session index {i} (model id {})", self.sessions[i].model.id()));
                    self.active = i;
                }
                Step::WaitExit(ms) => {
                    let id = self.s().id();
                    let done = self.exited.iter().any(|e| e.0 == id);
                    advance = done || elapsed >= Duration::from_millis(ms);
                    if advance {
                        log(&format!("wait exit: exited={done} exit_status={:?}", self.pty().exit_status()));
                    }
                }
                Step::Exit => {
                    let rects = self.s().diagnostics().ime_rects.clone();
                    if !rects.is_empty() {
                        log(&format!("IME cursor areas: {rects:?}"));
                    }
                    for s in &self.sessions {
                        log(&format!(
                            "model {} query replies: adapter answered {}, model dropped {}",
                            s.model.id(),
                            s.replies.load(std::sync::atomic::Ordering::Relaxed),
                            s.model.counters().dropped_replies.load(std::sync::atomic::Ordering::Relaxed)
                        ));
                    }
                    log("exit: killing children");
                    for s in &self.sessions {
                        s.kill();
                    }
                    self.pc += 1;
                    tasks.push(iced::exit());
                    return Task::batch(tasks);
                }
            }
            if !advance {
                return Task::batch(tasks);
            }
            self.pc += 1;
            self.step_start = Instant::now();
        }
    }

    fn view(&self) -> impl Widget<Message> + '_ {
        column![
            TerminalView::new(self.s(), &self.config)
                .on_input(Message::Input)
                .on_resize(Message::Resize)
                .on_event(Message::View),
            container(text(self.status.clone()).size(12).font(iced::Font::MONOSPACE))
                .height(STATUS_H)
                .padding([2, 6])
                .style(|_| container::Style {
                    background: Some(iced::Color::from_rgb8(0x2d, 0x2d, 0x30).into()),
                    text_color: Some(iced::Color::from_rgb8(0xaa, 0xaa, 0xaa)),
                    ..Default::default()
                })
                .width(Fill),
        ]
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs: Vec<_> = self.sessions.iter().map(|s| s.model.subscription().map(Message::Term)).collect();
        let scripted = !self.mode.is_empty();
        if !scripted || self.pc < self.script.len() {
            subs.push(iced::time::every(Duration::from_millis(if scripted { 30 } else { 500 })).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }
}

const STATUS_H: f32 = 20.0;

fn main() -> iced::Result {
    T0.get_or_init(Instant::now);
    iced::application(App::boot, App::update, App::view)
        .title("ide-term example")
        .subscription(App::subscription)
        .window_size((1000.0, 700.0))
        .run()
}
