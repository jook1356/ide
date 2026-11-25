//! meninx-web example: header + web view (inside a scrollable) + modal toggle.
//!
//! Run: `WGPU_BACKEND=dx12 cargo run -p meninx-web --example web --release`
//! Scripted mode (in-app event injection, no OS input):
//!   `MENINX_WEB_SCRIPT=basic|ime|perf|resize|sharpness|all` (+ `MENINX_WEB_EVIDENCE=<dir>` for
//!   screenshots, `MENINX_WEB_FORCE_CPU=1` for the software-frame path,
//!   `MENINX_WEB_LEGACY_PRESENT=1` to reproduce the pre-fix stretched/bilinear presentation).
//! `MENINX_WEB_URL=<address>` opens another start page (its favicon is logged).

use iced::keyboard::{self, Key, Location, Modifiers, key};
use iced::widget::{button, column, container, opaque, row, scrollable, space, stack, text};
use iced::{
    Color, Element, Event, Fill, Length, Point, Size, Subscription, Task, Widget as _, mouse,
    window,
};
use meninx_web::testing::{self, Injector};
use meninx_web::{RenderMode, WebEvent, WebView, WebViewEvent, WebViewWidget};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>meninx-web test page</title><style>
body{margin:0;font:16px "Segoe UI","Malgun Gothic",sans-serif;background:linear-gradient(135deg,#fffbe6,#dcf0ff);height:1400px}
#inp{position:absolute;left:20px;top:60px;width:320px;height:34px;font-size:18px;box-sizing:border-box}
#btn{position:absolute;left:20px;top:110px;width:180px;height:36px;font-size:16px}
#ta{position:absolute;left:20px;top:160px;width:320px;height:70px;font-size:16px}
#info{position:absolute;left:20px;top:245px;white-space:pre;font:13px Consolas,"Malgun Gothic",monospace}
.spin{position:absolute;left:420px;top:60px;width:90px;height:90px;background:linear-gradient(red,blue);animation:r 1.5s linear infinite}
.bar{position:absolute;left:0;top:430px;height:80px;width:100%;background:repeating-linear-gradient(90deg,#f80 0 40px,#0a7 40px 80px);animation:s 2s linear infinite}
#low{position:absolute;left:20px;top:900px}
@keyframes r{to{transform:rotate(360deg)}}
@keyframes s{from{background-position:0 0}to{background-position:160px 0}}
</style></head><body>
<h3 style="margin:0;position:absolute;left:20px;top:16px">meninx-web test page</h3>
<input id="inp" placeholder="type here / 한글 입력">
<button id="btn">Click me</button>
<textarea id="ta" placeholder="textarea"></textarea>
<div id="info"></div>
<div class="spin"></div><div class="bar"></div>
<div id="low">bottom of the page</div>
<script>
let n=0;const info=document.getElementById('info');const lines=[];
function L(s){console.log(s);lines.unshift(s);lines.length=Math.min(lines.length,9);info.textContent=lines.join('\n');}
const btn=document.getElementById('btn');
btn.onclick=()=>{n++;btn.textContent='Clicked '+n+' times';L('CLICK btn '+n)};
document.addEventListener('mousedown',e=>L('MOUSEDOWN '+e.clientX+','+e.clientY+' '+e.target.tagName+(e.target.id?'#'+e.target.id:'')));
for(const id of ['inp','ta']){const e=document.getElementById(id);
 e.addEventListener('input',x=>L('INPUT '+id+' '+JSON.stringify(e.value)+(x.isComposing?' composing':'')));
 for(const t of ['compositionstart','compositionupdate','compositionend'])e.addEventListener(t,x=>L('COMPOSITION '+t+' '+JSON.stringify(x.data)));
 e.addEventListener('focus',()=>L('FOCUS '+id));}
document.addEventListener('keydown',e=>L('KEYDOWN key='+e.key+' code='+e.code));
let st;addEventListener('scroll',()=>{clearTimeout(st);st=setTimeout(()=>L('SCROLL '+Math.round(scrollY)),150)});
let f=0,t0=performance.now();function tick(t){f++;if(t-t0>=2000){console.log('FPS '+Math.round(f*1000/(t-t0)));f=0;t0=t}requestAnimationFrame(tick)}requestAnimationFrame(tick);
L('READY');
</script></body></html>"#;

fn data_url(html: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::from("data:text/html;charset=utf-8;base64,");
    for c in html.as_bytes().chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn main() -> anyhow::Result<()> {
    if let Some(code) = meninx_web::maybe_run_subprocess() {
        std::process::exit(code);
    }
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();
    let data = std::env::var("MENINX_WEB_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("meninx-web-example"));
    let runtime = meninx_web::initialize(meninx_web::RuntimeSettings::new(data))?;
    let rt = runtime.clone();
    let result = iced::application(move || App::boot(&rt), App::update, App::view)
        .title("meninx-web example")
        .subscription(App::subscription)
        .window_size((1100.0, 820.0))
        .backend(meninx_web::backend())
        .run();
    println!("[app] iced exited: {result:?}");
    runtime.shutdown();
    println!("[app] shutdown complete");
    Ok(())
}

#[derive(Debug, Clone)]
enum Message {
    Web(WebViewEvent),
    Tick,
    ToggleModal,
    Back,
    Forward,
    Reload,
    ListScrolled(f32),
    WindowId(Option<window::Id>),
    Shot(String, window::Screenshot),
}

struct App {
    web: WebView,
    modal: bool,
    scroll_y: f32,
    title: String,
    url: String,
    loading: bool,
    page_fps: String,
    status: String,
    last_log: String,
    log: Vec<(Instant, String)>,
    stats_t0: Instant,
    window: Option<window::Id>,
    script: Option<Script>,
    /// Fixed web view width (resize script; simulates dragging a split divider).
    web_width: Option<f32>,
    last_shot: Option<window::Screenshot>,
}

impl App {
    fn boot(rt: &meninx_web::WebRuntime) -> (Self, Task<Message>) {
        // `MENINX_WEB_URL` starts at another page (e.g. one with a favicon).
        let start = std::env::var("MENINX_WEB_URL").unwrap_or_else(|_| data_url(PAGE));
        let web = WebView::new(rt, &start, Size::new(800.0, 560.0), 1.0)
            .expect("create web view");
        let script = std::env::var("MENINX_WEB_SCRIPT")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| Script::new(&s));
        (
            Self {
                web,
                modal: false,
                scroll_y: 0.0,
                title: String::new(),
                url: String::new(),
                loading: false,
                page_fps: "?".into(),
                status: "starting".into(),
                last_log: String::new(),
                log: Vec::new(),
                stats_t0: Instant::now(),
                window: None,
                script,
                web_width: None,
                last_shot: None,
            },
            window::latest().map(Message::WindowId),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Web(WebViewEvent { event, .. }) => match event {
                WebEvent::TitleChanged(t) => self.title = t.chars().take(60).collect(),
                WebEvent::UrlChanged(u) => self.url = u.chars().take(60).collect(),
                WebEvent::LoadingStateChanged { is_loading, .. } => self.loading = is_loading,
                WebEvent::Console { message, .. } => {
                    if let Some(fps) = message.strip_prefix("FPS ") {
                        self.page_fps = fps.to_string();
                    } else {
                        println!("[page] {message}");
                        self.last_log = message.clone();
                        self.log.push((Instant::now(), message));
                    }
                }
                WebEvent::RenderModeChanged(m) => println!("[app] render mode {m:?}"),
                WebEvent::FaviconChanged(f) => match f {
                    Some(f) => println!(
                        "[app] favicon {}x{} first pixel {:?}",
                        f.width,
                        f.height,
                        &f.rgba[..4.min(f.rgba.len())]
                    ),
                    None => println!("[app] favicon none"),
                },
                other => println!("[app] web event {other:?}"),
            },
            Message::Tick => {
                // Scripts read the counters themselves (Measure, ResizeStats).
                let measuring = self.script.is_some();
                if !measuring && self.stats_t0.elapsed() >= Duration::from_secs(2) {
                    let el = self.stats_t0.elapsed().as_secs_f64();
                    let s = self.web.take_stats();
                    self.status = format!(
                        "mode {:?} | accel paints {:.1}/s, imports {:.1}/s avg {:.2} ms max {:.2} ms, errors {} | cpu paints {:.1}/s copy {:.2} ms | frames shown {:.1}/s | page rAF {} fps",
                        s.mode.unwrap_or(RenderMode::Cpu),
                        s.accel_paints as f64 / el,
                        s.imports as f64 / el,
                        s.import_avg_ms,
                        s.import_max_ms,
                        s.import_errors,
                        s.cpu_paints as f64 / el,
                        s.cpu_copy_avg_ms,
                        s.frames_shown as f64 / el,
                        self.page_fps,
                    );
                    println!("[stats] {}", self.status);
                    self.stats_t0 = Instant::now();
                }
                if let Some(mut script) = self.script.take() {
                    let task = script.step(self);
                    if !script.done {
                        self.script = Some(script);
                    }
                    return task;
                }
            }
            Message::ToggleModal => self.modal = !self.modal,
            Message::Back => self.web.go_back(),
            Message::Forward => self.web.go_forward(),
            Message::Reload => self.web.reload(),
            Message::ListScrolled(y) => self.scroll_y = y,
            Message::WindowId(id) => self.window = id,
            Message::Shot(name, shot) => {
                let dir = evidence_dir();
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join(format!("{name}.png"));
                match save_png(&path, &shot) {
                    Ok(()) => println!(
                        "[script] screenshot {} ({}x{})",
                        path.display(),
                        shot.size.width,
                        shot.size.height
                    ),
                    Err(e) => println!("[script] screenshot {name} failed: {e}"),
                }
                self.last_shot = Some(shot);
                if let Some(s) = &mut self.script {
                    s.waiting_shot = false;
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            meninx_web::events().map(Message::Web),
            iced::time::every(Duration::from_millis(if self.script.is_some() {
                16
            } else {
                250
            }))
            .map(|_| Message::Tick),
        ])
    }

    fn view(&self) -> Element<'_, Message> {
        let header = column![
            row![
                text("meninx-web example").size(18),
                space().width(Fill),
                button("Back").on_press(Message::Back),
                button("Forward").on_press(Message::Forward),
                button("Reload").on_press(Message::Reload),
                button(if self.modal {
                    "Close modal"
                } else {
                    "Open modal"
                })
                .on_press(Message::ToggleModal),
            ]
            .spacing(8),
            text(format!(
                "{} | {} | {} | focused {} | list scroll {:.0}",
                if self.title.is_empty() {
                    "(no title)"
                } else {
                    &self.title
                },
                self.url,
                if self.loading { "loading" } else { "idle" },
                self.web.is_focused(),
                self.scroll_y
            ))
            .size(12),
            text(&self.status).size(12),
        ]
        .spacing(4)
        .padding(8);

        let filler = |label: &'static str, h: f32| {
            container(text(label).size(14))
                .padding(12)
                .width(Fill)
                .height(h)
                .style(container::rounded_box)
        };
        let web: Element<'_, Message> = WebViewWidget::new(&self.web)
            .width(self.web_width.map(Length::Fixed).unwrap_or(Length::Fill))
            .height(560)
            // Under the modal: a dimmed, frozen snapshot of the page (no input reaches it).
            .dimmed(self.modal)
            .into();
        let list = scrollable(
            column![
                filler(
                    "Content above the web view (scroll the list to test clipping).",
                    120.0
                ),
                container(web).padding(8).style(container::rounded_box),
                filler("Content below the web view.", 400.0),
            ]
            .spacing(10)
            .padding(10),
        )
        .smooth_scroll(false)
        .on_scroll(|s| Message::ListScrolled(s.viewport.absolute_offset().y))
        .height(Fill);

        let footer =
            container(text(format!("last page log: {}", self.last_log)).size(12)).padding(6);
        let base = column![header, list, footer];

        // Keep the tree shape stable (always a stack) so toggling the modal keeps widget state
        // such as the list scroll offset.
        let overlay: Element<'_, Message> = if self.modal {
            opaque(
                container(
                    container(
                        column![
                            text("Modal overlay").size(28),
                            text("Drawn above the web view; clicks must not reach the page."),
                            button("Close").on_press(Message::ToggleModal)
                        ]
                        .spacing(12),
                    )
                    .padding(30)
                    .width(460)
                    .style(|_| container::background(Color::from_rgb8(250, 230, 120))),
                )
                .center(Fill)
                .style(|_| {
                    container::background(Color {
                        a: 0.55,
                        ..Color::BLACK
                    })
                }),
            )
            .into()
        } else {
            space().boxed()
        };
        let content = stack![base, overlay].boxed();
        Injector::new(content).into()
    }

    /// Window coordinates of a page point (CSS px), honouring the list scroll offset.
    fn page_to_window(&self, x: f32, y: f32) -> Option<Point> {
        let b = self.web.last_bounds()?;
        Some(Point::new(b.x + x, b.y + y - self.scroll_y))
    }
}

fn evidence_dir() -> PathBuf {
    std::env::var("MENINX_WEB_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("meninx-web-evidence"))
}

fn save_png(path: &std::path::Path, shot: &window::Screenshot) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, shot.size.width, shot.size.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&shot.rgba)?;
    Ok(())
}

// ------------------------------------------------------------------ test script

#[derive(Debug, Clone)]
enum Step {
    Wait(u64),
    /// Wait until a page log line (since the last Mark) contains the text.
    WaitFor(&'static str, u64),
    Mark,
    /// Check a page log line since the last Mark contains the text (waits up to 2 s).
    Expect(String),
    /// Check that no page log line since the last Mark contains the text (after a short wait).
    ExpectNone(&'static str),
    ClickPage(f32, f32),
    ClickWindow(f32, f32),
    Type(&'static str),
    Key(key::Named, key::Code),
    WheelPage(f32, f32, f32),
    /// Wheel over the list padding next to the web view (scrolls the list, not the page).
    WheelList(f32),
    Preedit(&'static str),
    Commit(&'static str),
    ImeClosed,
    ExpectScrollAtLeast(f32),
    ExpectScrollEq,
    PrintCaret,
    ToggleModal,
    Shot(&'static str),
    Measure(u64),
    /// Fix the web view width (logical px; `None` = fill), like dragging a split divider.
    SetWidth(Option<f32>),
    /// Reset the render counters.
    ResetStats,
    /// Print the frame and widget sizes (px) as of the last drawn frame.
    Sizes,
    /// `WebView::set_hidden` (CEF stops producing frames while hidden).
    Hidden(bool),
    /// Print the resize lag stats; checks the frame on screen matches the widget's pixel size.
    ResizeStats,
    /// Copy the frame the widget draws back to the CPU (waits for it).
    Dump,
    /// Compare the last screenshot with the dumped frame pixel by pixel over a page rect (CSS
    /// px, x0 y0 x1 y1); saves 4x crops of both as `<name>_screen.png` / `<name>_frame.png`.
    Compare(&'static str, [f32; 4]),
    /// Mean brightness of a page rect (CSS px) in the last screenshot, remembered under a name.
    Brightness(&'static str, [f32; 4]),
    /// Check brightness `a` < `b` * ratio (e.g. dimmed snapshot is darker than the live page)
    /// and `a` > `min` (the page is still visible, not replaced by a plain panel).
    ExpectDarker(&'static str, &'static str, f32, f32),
    Exit,
}

struct Script {
    steps: Vec<Step>,
    i: usize,
    started: Option<Instant>,
    mark: Instant,
    scroll_mark: f32,
    waiting_shot: bool,
    measure_t0: Option<Instant>,
    pass: u32,
    fail: u32,
    done: bool,
    dump: Option<meninx_web::FrameDump>,
    dump_pending: bool,
    brightness: Vec<(&'static str, f32)>,
}

fn basic() -> Vec<Step> {
    use Step::*;
    vec![
        WaitFor("READY", 20_000),
        Wait(1500),
        Shot("01_initial"),
        Mark,
        ClickPage(100.0, 77.0),
        Expect("MOUSEDOWN 100,77 INPUT#inp".into()),
        Expect("FOCUS inp".into()),
        Type("hello iced"),
        Expect(r#"INPUT inp "hello iced""#.into()),
        Key(key::Named::Backspace, key::Code::Backspace),
        Expect(r#"INPUT inp "hello ice""#.into()),
        Expect("KEYDOWN key=Backspace code=Backspace".into()),
        Mark,
        ClickPage(110.0, 128.0),
        Expect("MOUSEDOWN 110,128 BUTTON#btn".into()),
        Expect("CLICK btn 1".into()),
        // Scroll the list (wheel over the padding beside the view) and click again.
        WheelList(-2.0),
        Wait(400),
        ExpectScrollAtLeast(60.0),
        // The input (page y 77) is now scrolled under the header: a click there must not reach it.
        Mark,
        ClickPage(60.0, 77.0),
        ExpectNone("MOUSEDOWN"),
        // The textarea is still visible: the click maps to the right page coordinates.
        Mark,
        ClickPage(60.0, 190.0),
        Expect("MOUSEDOWN 60,190 TEXTAREA#ta".into()),
        Expect("FOCUS ta".into()),
        Type("after scroll"),
        Key(key::Named::Enter, key::Code::Enter),
        Type("line 2"),
        Expect(r#"INPUT ta "after scroll\nline 2""#.into()),
        Wait(300),
        Shot("02_scrolled_clipped"),
        // Wheel over the page scrolls the page, not the list.
        Mark,
        WheelPage(300.0, 350.0, -3.0),
        WaitFor("SCROLL ", 3000),
        ExpectScrollEq,
        WheelPage(300.0, 350.0, 3.0),
        Wait(400),
        // Modal: drawn above the page, clicks do not reach it.
        ToggleModal,
        Wait(400),
        Mark,
        ClickPage(110.0, 128.0),
        ExpectNone("MOUSEDOWN"),
        Shot("03_modal_overlay"),
        ToggleModal,
        Wait(300),
    ]
}

fn ime() -> Vec<Step> {
    use Step::*;
    vec![
        WaitFor("READY", 20_000),
        Wait(1000),
        Mark,
        ClickPage(300.0, 77.0),
        Expect("FOCUS inp".into()),
        Wait(300),
        PrintCaret,
        Mark,
        Preedit("ㅎ"),
        Expect(r#"COMPOSITION compositionstart """#.into()),
        Preedit("하"),
        Preedit("한"),
        Expect(r#"COMPOSITION compositionupdate "한""#.into()),
        Wait(200),
        PrintCaret,
        Commit("한"),
        Expect(r#"COMPOSITION compositionend "한""#.into()),
        Expect(r#"INPUT inp "한""#.into()),
        Preedit("ㄱ"),
        Preedit("그"),
        Preedit("글"),
        Commit("글"),
        Expect(r#"INPUT inp "한글""#.into()),
        // Composition visible in the page.
        Mark,
        Preedit("테"),
        Expect(r#"COMPOSITION compositionupdate "테""#.into()),
        Wait(400),
        PrintCaret,
        Shot("04_ime_composing"),
        // Click inside the page during composition: CEF commits it; the IME's late commit is dropped.
        Mark,
        ClickPage(330.0, 77.0),
        Expect(r#"COMPOSITION compositionend "테""#.into()),
        Commit("테"),
        Wait(400),
        ExpectNone("테테"),
        Mark,
        Key(key::Named::End, key::Code::End),
        Type(" ok"),
        // Proves the late commit was dropped (no duplicated "테").
        Expect(r#"INPUT inp "한글테 ok""#.into()),
        // Cancel: preedit then empty preedit.
        Mark,
        Preedit("ㅋ"),
        Preedit(""),
        Expect(r#"COMPOSITION compositionend """#.into()),
        // Blur during composition (click outside the web view) finishes it.
        Mark,
        Preedit("ㅁ"),
        Expect(r#"COMPOSITION compositionupdate "ㅁ""#.into()),
        ClickWindow(400.0, 20.0),
        Expect(r#"COMPOSITION compositionend "ㅁ""#.into()),
        Expect(r#"INPUT inp "한글테 okㅁ""#.into()),
        // IME closed while composing finishes too.
        ClickPage(330.0, 77.0),
        Mark,
        Preedit("ㅂ"),
        ImeClosed,
        Expect(r#"COMPOSITION compositionend "ㅂ""#.into()),
        Wait(400),
        Shot("05_ime_done"),
    ]
}

fn perf() -> Vec<Step> {
    use Step::*;
    vec![WaitFor("READY", 20_000), Wait(3000), Measure(12_000)]
}

/// Static text region of the test page (header, input, button, textarea; CSS px).
const TEXT_RECT: [f32; 4] = [10.0, 8.0, 360.0, 240.0];

fn sharpness() -> Vec<Step> {
    use Step::*;
    vec![
        WaitFor("READY", 20_000),
        Wait(1500),
        Dump,
        Shot("s1_sharpness"),
        Compare("s1_text", TEXT_RECT),
        // Dimmed snapshot under the modal: darker, but still the page (not a plain panel).
        Brightness("live", [10.0, 260.0, 400.0, 420.0]),
        ToggleModal,
        Wait(500),
        Mark,
        ClickPage(110.0, 128.0),
        ExpectNone("MOUSEDOWN"),
        Shot("s2_modal_dimmed"),
        Brightness("modal", [10.0, 260.0, 400.0, 420.0]),
        ExpectDarker("modal", "live", 0.6, 0.02),
        ToggleModal,
        Wait(500),
        Mark,
        ClickPage(110.0, 128.0),
        Expect("CLICK btn".into()),
        Shot("s3_modal_closed"),
    ]
}

fn resize() -> Vec<Step> {
    use Step::*;
    let mut v = vec![
        WaitFor("READY", 20_000),
        Wait(1000),
        SetWidth(Some(1000.0)),
        Wait(1000),
        Shot("r0_wide"),
        // A big width change while CEF is paused (hidden), so its frame of the new size cannot
        // arrive: shows deterministically how a resize in flight is presented (the real lag is
        // ~3 frames, measured below).
        Hidden(true),
        SetWidth(Some(700.0)),
        Wait(150),
        Shot("r1_shrink_in_flight"),
        Sizes,
        Hidden(false),
        Wait(1000),
        Shot("r2_shrunk"),
        Hidden(true),
        SetWidth(Some(1000.0)),
        Wait(150),
        Shot("r3_grow_in_flight"),
        Sizes,
        Hidden(false),
        Wait(1000),
        ResetStats,
    ];
    // Divider drag: 3 logical px per tick (~every frame) from 1000 down to 640 and back.
    let mut w = 1000.0;
    while w > 640.0 {
        w -= 3.0;
        v.push(SetWidth(Some(w)));
    }
    v.push(Shot("r4_drag_in_flight"));
    while w < 1000.0 {
        w += 3.0;
        v.push(SetWidth(Some(w)));
    }
    v.extend([
        Wait(800),
        ResizeStats,
        Shot("r5_after_drag"),
        SetWidth(None),
        Wait(500),
    ]);
    // Fractional widths (scale 1.5): the frame must still match the widget pixel for pixel.
    for w in [801.3, 802.7, 777.77] {
        v.extend([
            SetWidth(Some(w)),
            Wait(600),
            Dump,
            Shot("r6_fractional"),
            Compare("r6_fractional", TEXT_RECT),
        ]);
    }
    v.extend([SetWidth(None), Wait(400), ResizeStats]);
    v
}

impl Script {
    fn new(name: &str) -> Self {
        let mut steps = match name {
            "basic" => basic(),
            "ime" => ime(),
            "perf" => perf(),
            "sharpness" => sharpness(),
            "resize" => resize(),
            _ => {
                let mut s = basic();
                s.extend(ime().into_iter().skip(1));
                s.extend(sharpness().into_iter().skip(1));
                s.extend(resize().into_iter().skip(1));
                s.extend(perf().into_iter().skip(1));
                s
            }
        };
        steps.push(Step::Exit);
        println!("[script] {name}: {} steps", steps.len());
        Self {
            steps,
            i: 0,
            started: None,
            mark: Instant::now(),
            scroll_mark: 0.0,
            waiting_shot: false,
            measure_t0: None,
            pass: 0,
            fail: 0,
            done: false,
            dump: None,
            dump_pending: false,
            brightness: Vec::new(),
        }
    }

    fn seen(&self, app: &App, needle: &str) -> bool {
        app.log
            .iter()
            .any(|(t, l)| *t >= self.mark && l.contains(needle))
    }

    fn check(&mut self, ok: bool, what: &str) {
        if ok {
            self.pass += 1;
            println!("[script] PASS {what}");
        } else {
            self.fail += 1;
            println!("[script] FAIL {what}");
        }
    }

    fn step(&mut self, app: &mut App) -> Task<Message> {
        if self.waiting_shot || testing::pending() > 0 {
            return Task::none();
        }
        let Some(step) = self.steps.get(self.i).cloned() else {
            self.done = true;
            return Task::none();
        };
        let started = *self.started.get_or_insert_with(Instant::now);
        let elapsed = started.elapsed();
        let mut next = true;
        let mut task = Task::none();
        match step {
            Step::Wait(ms) => next = elapsed >= Duration::from_millis(ms),
            Step::WaitFor(needle, ms) => {
                if self.seen(app, needle) {
                    println!("[script] saw {needle:?} after {} ms", elapsed.as_millis());
                } else if elapsed >= Duration::from_millis(ms) {
                    self.check(false, &format!("timeout waiting for {needle:?}"));
                } else {
                    next = false;
                }
            }
            Step::Mark => {
                self.mark = Instant::now();
                self.scroll_mark = app.scroll_y;
            }
            Step::Expect(needle) => {
                if self.seen(app, &needle) {
                    self.check(true, &format!("page logged {needle:?}"));
                } else if elapsed >= Duration::from_secs(2) {
                    self.check(false, &format!("page logged {needle:?}"));
                } else {
                    next = false;
                }
            }
            Step::ExpectNone(needle) => {
                if elapsed < Duration::from_millis(800) {
                    next = false;
                } else {
                    let ok = !self.seen(app, needle);
                    self.check(ok, &format!("page did NOT log {needle:?}"));
                }
            }
            Step::ClickPage(x, y) => match app.page_to_window(x, y) {
                Some(p) => {
                    println!(
                        "[script] click page ({x},{y}) -> window ({:.1},{:.1}) list scroll {:.0}",
                        p.x, p.y, app.scroll_y
                    );
                    testing::click(p);
                }
                None => next = elapsed > Duration::from_secs(5),
            },
            Step::ClickWindow(x, y) => testing::click(Point::new(x, y)),
            Step::Type(s) => {
                for c in s.chars() {
                    let k = Key::Character(c.to_string().into());
                    let physical = code_for(c)
                        .map(key::Physical::Code)
                        .unwrap_or(key::Physical::Unidentified(key::NativeCode::Unidentified));
                    testing::inject(
                        Event::Keyboard(keyboard::Event::KeyPressed {
                            key: k.clone(),
                            modified_key: k.clone(),
                            physical_key: physical,
                            location: Location::Standard,
                            modifiers: Modifiers::empty(),
                            text: Some(c.to_string().into()),
                            repeat: false,
                        }),
                        None,
                    );
                    testing::inject(
                        Event::Keyboard(keyboard::Event::KeyReleased {
                            key: k.clone(),
                            modified_key: k,
                            physical_key: physical,
                            location: Location::Standard,
                            modifiers: Modifiers::empty(),
                        }),
                        None,
                    );
                }
            }
            Step::Key(named, code) => {
                let k = Key::Named(named);
                testing::inject(
                    Event::Keyboard(keyboard::Event::KeyPressed {
                        key: k.clone(),
                        modified_key: k.clone(),
                        physical_key: key::Physical::Code(code),
                        location: Location::Standard,
                        modifiers: Modifiers::empty(),
                        text: None,
                        repeat: false,
                    }),
                    None,
                );
                testing::inject(
                    Event::Keyboard(keyboard::Event::KeyReleased {
                        key: k.clone(),
                        modified_key: k,
                        physical_key: key::Physical::Code(code),
                        location: Location::Standard,
                        modifiers: Modifiers::empty(),
                    }),
                    None,
                );
            }
            Step::WheelPage(x, y, lines) => {
                if let Some(p) = app.page_to_window(x, y) {
                    testing::inject(
                        Event::Mouse(mouse::Event::CursorMoved { position: p }),
                        Some(p),
                    );
                    testing::inject(
                        Event::Mouse(mouse::Event::WheelScrolled {
                            delta: mouse::ScrollDelta::Lines { x: 0.0, y: lines },
                        }),
                        Some(p),
                    );
                }
            }
            Step::WheelList(lines) => {
                if let Some(b) = app.web.last_bounds() {
                    // Container padding left of the web view, inside the scrollable.
                    let p = Point::new(b.x - 4.0, b.y + 200.0 - app.scroll_y);
                    testing::inject(
                        Event::Mouse(mouse::Event::CursorMoved { position: p }),
                        Some(p),
                    );
                    testing::inject(
                        Event::Mouse(mouse::Event::WheelScrolled {
                            delta: mouse::ScrollDelta::Lines { x: 0.0, y: lines },
                        }),
                        Some(p),
                    );
                }
            }
            Step::Preedit(s) => {
                let n = s.len();
                let sel = if n == 0 { None } else { Some(n..n) };
                testing::inject(
                    Event::InputMethod(iced::advanced::input_method::Event::Preedit(
                        s.to_string(),
                        sel,
                    )),
                    None,
                );
            }
            Step::Commit(s) => testing::inject(
                Event::InputMethod(iced::advanced::input_method::Event::Commit(s.to_string())),
                None,
            ),
            Step::ImeClosed => testing::inject(
                Event::InputMethod(iced::advanced::input_method::Event::Closed),
                None,
            ),
            Step::ExpectScrollAtLeast(min) => {
                let y = app.scroll_y;
                self.check(
                    y >= min,
                    &format!("list scrolled by wheel: scroll_y {y:.0} >= {min}"),
                );
            }
            Step::ExpectScrollEq => {
                let y = app.scroll_y;
                self.check(
                    (y - self.scroll_mark).abs() < 0.5,
                    &format!(
                        "list did not scroll on page wheel ({:.0} -> {y:.0})",
                        self.scroll_mark
                    ),
                );
            }
            Step::PrintCaret => {
                println!(
                    "[script] IME caret (view coords) {:?}, composition {:?}, widget bounds {:?}",
                    app.web.ime_caret(),
                    app.web.composition(),
                    app.web.last_bounds()
                );
            }
            Step::ToggleModal => app.modal = !app.modal,
            Step::Shot(name) => {
                if let Some(id) = app.window {
                    self.waiting_shot = true;
                    task = window::screenshot(id).map(move |s| Message::Shot(name.to_string(), s));
                }
            }
            Step::Measure(ms) => match self.measure_t0 {
                None => {
                    let _ = app.web.take_stats();
                    println!("[script] measuring for {ms} ms (MEASURE_START)");
                    self.measure_t0 = Some(Instant::now());
                    next = false;
                }
                Some(t0) => {
                    if t0.elapsed() < Duration::from_millis(ms) {
                        next = false;
                    } else {
                        let el = t0.elapsed().as_secs_f64();
                        let s = app.web.take_stats();
                        println!(
                            "[measure] {:.1} s mode {:?}: accel paints {:.1}/s, imports {:.1}/s, import avg {:.2} ms max {:.2} ms, errors {}, cpu paints {:.1}/s (copy {:.2} ms), frames shown {:.1}/s, page rAF {} fps (MEASURE_END)",
                            el,
                            s.mode,
                            s.accel_paints as f64 / el,
                            s.imports as f64 / el,
                            s.import_avg_ms,
                            s.import_max_ms,
                            s.import_errors,
                            s.cpu_paints as f64 / el,
                            s.cpu_copy_avg_ms,
                            s.frames_shown as f64 / el,
                            app.page_fps
                        );
                        self.measure_t0 = None;
                    }
                }
            },
            Step::SetWidth(w) => app.web_width = w,
            Step::Hidden(h) => app.web.set_hidden(h),
            Step::Sizes => {
                let s = app.web.take_stats();
                println!(
                    "[resize] frame {:?} px, widget {:?} px",
                    s.frame_px, s.widget_px
                );
            }
            Step::ResetStats => {
                let _ = app.web.take_stats();
            }
            Step::ResizeStats => {
                let s = app.web.take_stats();
                println!(
                    "[resize] {} resizes reached the screen: lag avg {:.2} frames (max {}), avg {:.1} ms (max {:.1} ms); frame {:?} px, widget {:?} px",
                    s.resizes,
                    s.resize_lag_frames_avg,
                    s.resize_lag_frames_max,
                    s.resize_lag_ms_avg,
                    s.resize_lag_ms_max,
                    s.frame_px,
                    s.widget_px
                );
                let (f, w) = (s.frame_px, s.widget_px);
                self.check(
                    f.0 >= w.0 && f.1 >= w.1 && f.0 - w.0 <= 1 && f.1 - w.1 <= 1,
                    &format!("frame {f:?} covers widget {w:?} with <= 1 px overflow"),
                );
            }
            Step::Dump => {
                if !self.dump_pending {
                    self.dump = None;
                    self.dump_pending = true;
                    app.web.request_frame_dump();
                    next = false;
                } else {
                    match app.web.take_frame_dump() {
                        Some(d) => {
                            println!(
                                "[script] frame dump {}x{} at window px {:?}, widget {:?} px, scale {}",
                                d.width, d.height, d.origin, d.widget_px, d.scale
                            );
                            self.dump = Some(d);
                            self.dump_pending = false;
                        }
                        None if elapsed > Duration::from_secs(3) => {
                            self.dump_pending = false;
                            self.check(false, "frame dump arrived");
                        }
                        None => next = false,
                    }
                }
            }
            Step::Compare(name, css) => match (&app.last_shot, &self.dump) {
                (Some(shot), Some(d)) => {
                    let r = compare(shot, d, css, name);
                    println!(
                        "[sharpness] {name}: {} of {} px differ (max channel diff {}), mean abs diff {:.3}",
                        r.0, r.1, r.2, r.3
                    );
                    self.check(
                        r.1 > 0 && r.0 == 0,
                        &format!("{name}: screen == CEF frame, pixel for pixel"),
                    );
                }
                _ => self.check(false, &format!("{name}: have screenshot and frame dump")),
            },
            Step::Brightness(name, css) => {
                if let (Some(shot), Some(b)) = (&app.last_shot, app.web.last_bounds()) {
                    let s = shot.scale_factor;
                    let x0 = ((b.x + css[0]) * s) as u32;
                    let y0 = ((b.y - app.scroll_y + css[1]) * s) as u32;
                    let x1 = ((b.x + css[2]) * s) as u32;
                    let y1 = ((b.y - app.scroll_y + css[3]) * s) as u32;
                    let (mut sum, mut n) = (0f64, 0f64);
                    for y in y0..y1.min(shot.size.height) {
                        for x in x0..x1.min(shot.size.width) {
                            let i = ((y * shot.size.width + x) * 4) as usize;
                            sum += (shot.rgba[i] as f64
                                + shot.rgba[i + 1] as f64
                                + shot.rgba[i + 2] as f64)
                                / 765.0;
                            n += 1.0;
                        }
                    }
                    let mean = (sum / n.max(1.0)) as f32;
                    println!("[script] brightness {name}: {mean:.3}");
                    self.brightness.push((name, mean));
                }
            }
            Step::ExpectDarker(a, b, ratio, min) => {
                let get = |n: &str| self.brightness.iter().rev().find(|x| x.0 == n).map(|x| x.1);
                let (va, vb) = (get(a).unwrap_or(-1.0), get(b).unwrap_or(-1.0));
                self.check(
                    va >= min && va < vb * ratio,
                    &format!("{a} ({va:.3}) darker than {b} ({vb:.3}) x {ratio} and > {min}"),
                );
            }
            Step::Exit => {
                println!("[script] DONE: {} passed, {} failed", self.pass, self.fail);
                self.done = true;
                return iced::exit();
            }
        }
        if next {
            self.i += 1;
            self.started = None;
        }
        task
    }
}

/// Compares the screenshot with the frame dump over a page rect (CSS px) and saves 4x crops.
/// Returns (differing pixels, compared pixels, max channel diff, mean abs channel diff).
fn compare(
    shot: &window::Screenshot,
    d: &meninx_web::FrameDump,
    css: [f32; 4],
    name: &str,
) -> (u64, u64, u8, f64) {
    let s = d.scale;
    let (fx0, fy0) = ((css[0] * s) as u32, (css[1] * s) as u32);
    let fx1 = ((css[2] * s) as u32).min(d.width).min(d.widget_px.0);
    let fy1 = ((css[3] * s) as u32).min(d.height).min(d.widget_px.1);
    let (mut differ, mut total, mut max, mut sum) = (0u64, 0u64, 0u8, 0u64);
    let (w, h) = (fx1.saturating_sub(fx0), fy1.saturating_sub(fy0));
    let mut screen = vec![0u8; (w * h * 4) as usize];
    let mut frame = vec![0u8; (w * h * 4) as usize];
    for fy in fy0..fy1 {
        for fx in fx0..fx1 {
            let (sx, sy) = (d.origin.0 + fx as i32, d.origin.1 + fy as i32);
            if sx < 0 || sy < 0 || sx as u32 >= shot.size.width || sy as u32 >= shot.size.height {
                continue;
            }
            let si = ((sy as u32 * shot.size.width + sx as u32) * 4) as usize;
            let fi = ((fy * d.width + fx) * 4) as usize;
            let sp = [shot.rgba[si], shot.rgba[si + 1], shot.rgba[si + 2]];
            let fp = [d.bgra[fi + 2], d.bgra[fi + 1], d.bgra[fi]];
            let o = (((fy - fy0) * w + (fx - fx0)) * 4) as usize;
            screen[o..o + 4].copy_from_slice(&[sp[0], sp[1], sp[2], 255]);
            frame[o..o + 4].copy_from_slice(&[fp[0], fp[1], fp[2], 255]);
            let m = (0..3).map(|c| sp[c].abs_diff(fp[c])).max().unwrap();
            sum += (0..3).map(|c| sp[c].abs_diff(fp[c]) as u64).sum::<u64>();
            total += 1;
            if m > 0 {
                differ += 1;
                max = max.max(m);
            }
        }
    }
    let dir = evidence_dir();
    for (suffix, px) in [("screen", &screen), ("frame", &frame)] {
        let path = dir.join(format!("{name}_{suffix}.png"));
        if let Err(e) = save_scaled(&path, w, h, px, 4) {
            println!("[script] saving {} failed: {e}", path.display());
        }
    }
    (differ, total, max, sum as f64 / (total.max(1) * 3) as f64)
}

/// Saves an RGBA image upscaled `k` times (nearest neighbour) so single pixels stay visible.
fn save_scaled(path: &std::path::Path, w: u32, h: u32, rgba: &[u8], k: u32) -> anyhow::Result<()> {
    let mut out = Vec::with_capacity((w * h * k * k * 4) as usize);
    for y in 0..h * k {
        for x in 0..w * k {
            let i = (((y / k) * w + x / k) * 4) as usize;
            out.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w * k, h * k);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&out)?;
    Ok(())
}

fn code_for(c: char) -> Option<key::Code> {
    use key::Code::*;
    Some(match c.to_ascii_lowercase() {
        'a' => KeyA,
        'b' => KeyB,
        'c' => KeyC,
        'd' => KeyD,
        'e' => KeyE,
        'f' => KeyF,
        'g' => KeyG,
        'h' => KeyH,
        'i' => KeyI,
        'j' => KeyJ,
        'k' => KeyK,
        'l' => KeyL,
        'm' => KeyM,
        'n' => KeyN,
        'o' => KeyO,
        'p' => KeyP,
        'q' => KeyQ,
        'r' => KeyR,
        's' => KeyS,
        't' => KeyT,
        'u' => KeyU,
        'v' => KeyV,
        'w' => KeyW,
        'x' => KeyX,
        'y' => KeyY,
        'z' => KeyZ,
        '0' => Digit0,
        '1' => Digit1,
        '2' => Digit2,
        '3' => Digit3,
        '4' => Digit4,
        '5' => Digit5,
        '6' => Digit6,
        '7' => Digit7,
        '8' => Digit8,
        '9' => Digit9,
        ' ' => Space,
        '+' => Equal,
        '-' => Minus,
        '.' => Period,
        ',' => Comma,
        _ => return None,
    })
}
