//! One off-screen CEF browser: shared state, CEF handlers and the [`WebView`] handle.

use cef::{rc::Rc as _, *};
use iced::{Color, Rectangle, Size};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering::*};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use crate::events::{ConsoleLevel, Favicon, RenderMode, ViewId, WebEvent, emit};
use crate::gpu::{self, PendingResize};
use crate::runtime::{self, LIVE_BROWSERS};

/// `cef::Browser` is a ref-counted, thread-safe CEF object; the Rust wrapper is just not marked so.
pub(crate) struct SendBrowser(pub Browser);
// SAFETY: CEF browser objects are ref-counted with atomic counts and CefBrowser/CefBrowserHost
// methods may be called from any browser-process thread (CEF posts to its UI thread internally).
unsafe impl Send for SendBrowser {}
// SAFETY: see above; the wrapper only holds the ref-counted pointer.
unsafe impl Sync for SendBrowser {}

/// Latest software frame from `on_paint` (BGRA premultiplied, top-down, tightly packed).
pub(crate) struct CpuFrame {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A copy of the frame currently drawn by the widget (see [`WebView::request_frame_dump`]).
#[derive(Debug, Clone)]
pub struct FrameDump {
    /// Frame size in physical pixels.
    pub width: u32,
    pub height: u32,
    /// Premultiplied BGRA, top-down, tightly packed.
    pub bgra: Vec<u8>,
    /// Window pixel of the frame's top-left texel when it was drawn.
    pub origin: (i32, i32),
    /// The widget's size in physical pixels when it was drawn.
    pub widget_px: (u32, u32),
    /// iced's scale factor when it was drawn.
    pub scale: f32,
}

/// Rendering/perf counters (reset by [`WebView::take_stats`]).
#[derive(Default)]
pub(crate) struct Counters {
    pub accel_paints: AtomicU64,
    pub cpu_paints: AtomicU64,
    pub imports: AtomicU64,
    pub import_ns: AtomicU64,
    pub import_max_ns: AtomicU64,
    pub import_errors: AtomicU64,
    pub cpu_copy_ns: AtomicU64,
    pub frames_shown: AtomicU64,
    /// Completed resizes (a frame of the new size reached the screen) and their lag.
    pub resizes: AtomicU64,
    pub resize_frames: AtomicU64,
    pub resize_frames_max: AtomicU64,
    pub resize_ns: AtomicU64,
    pub resize_max_ns: AtomicU64,
}

/// Snapshot of render counters since the last [`WebView::take_stats`].
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderStats {
    pub mode: Option<RenderMode>,
    /// CEF `on_accelerated_paint` callbacks.
    pub accel_paints: u64,
    /// CEF `on_paint` callbacks.
    pub cpu_paints: u64,
    /// Successful GPU imports and their average / max duration (ms).
    pub imports: u64,
    pub import_avg_ms: f64,
    pub import_max_ms: f64,
    pub import_errors: u64,
    /// Average `on_paint` copy time (ms).
    pub cpu_copy_avg_ms: f64,
    /// New frames bound for drawing by the iced pipeline.
    pub frames_shown: u64,
    /// Resizes whose new-size frame reached the screen, and how long that took: iced frames
    /// drawn and milliseconds from the frame that first showed the new bounds.
    pub resizes: u64,
    pub resize_lag_frames_avg: f64,
    pub resize_lag_frames_max: u64,
    pub resize_lag_ms_avg: f64,
    pub resize_lag_ms_max: f64,
    /// Size (physical px) of the frame on screen, and of the widget.
    pub frame_px: (u32, u32),
    pub widget_px: (u32, u32),
}

/// Input/IME state that must survive widget rebuilds.
#[derive(Default)]
pub(crate) struct InputState {
    pub focused: bool,
    /// Text of the composition currently shown in the page (CEF `ime_set_composition`).
    pub composing: Option<String>,
    /// Last composition caret rectangle from CEF (view DIP coordinates).
    pub ime_caret: Option<Rectangle>,
    /// Caret reported by the injected caret probe script (view DIP coordinates).
    pub probe_caret: Option<Rectangle>,
    /// Text CEF already committed itself (finish on click); the IME's late commit of it is dropped.
    pub drop_commit: Option<String>,
    /// Ask the window to disable the IME for one frame (resets the OS composition).
    pub ime_reset: bool,
}

pub(crate) struct ViewShared {
    pub id: ViewId,
    pub browser: Mutex<Option<SendBrowser>>,
    /// Page background: CEF's `background_color` and the fill where no frame pixels are yet
    /// (while a resize is in flight).
    pub background: Color,
    /// Bumped whenever a browser is (re)created; handlers of older browsers become inert.
    pub generation: AtomicU64,
    /// View size in DIP given to CEF (`view_rect`).
    pub width: AtomicU32,
    pub height: AtomicU32,
    pub scale_bits: AtomicU32,
    /// The widget's physical size (px) at its last draw; CEF's frame covers it.
    pub widget_px: AtomicU64,
    /// Size (px) of the frame on screen.
    pub frame_px: AtomicU64,
    /// Resizes requested from CEF whose frame has not reached the screen yet (oldest first).
    pub resize_pending: Mutex<Vec<PendingResize>>,
    /// The app hid the browser ([`WebView::set_hidden`]).
    pub app_hidden: AtomicBool,
    /// The widget shows a frozen, dimmed snapshot ([`crate::WebViewWidget::dimmed`]).
    pub frozen: AtomicBool,
    pub dump_requested: AtomicBool,
    pub dump: Mutex<Option<FrameDump>>,
    pub gpu_mode: AtomicBool,
    pub consecutive_import_failures: AtomicU32,
    pub gpu_frame: Mutex<Option<(iced::wgpu::Texture, u64)>>,
    pub cpu_frame: Mutex<Option<(CpuFrame, u64)>>,
    pub frame_seq: AtomicU64,
    pub drawn_seq: AtomicU64,
    pub waker: Mutex<Option<iced::advanced::shell::Waker>>,
    pub input: Mutex<InputState>,
    /// `CursorType` mapped to a small index (see `widget::interaction_from`).
    pub cursor: AtomicU8,
    pub url: Mutex<String>,
    pub title: Mutex<String>,
    pub loading: AtomicBool,
    pub pending_url: Mutex<Option<String>>,
    /// Bumped when the page names new icons; downloads for older lists are dropped.
    pub favicon_seq: AtomicU64,
    pub closed: AtomicBool,
    pub caret_probe: bool,
    /// Layout bounds of the widget at its last draw (iced layout coordinates, i.e. before any
    /// scrollable translation).
    pub last_bounds: Mutex<Option<Rectangle>>,
    pub counters: Counters,
    /// DevTools requests and the console log (`devtools.rs`).
    pub cdp: crate::devtools::CdpState,
}

impl ViewShared {
    pub fn host(&self) -> Option<BrowserHost> {
        self.browser
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|b| b.0.host())
    }

    pub fn browser(&self) -> Option<Browser> {
        self.browser.lock().unwrap().as_ref().map(|b| b.0.clone())
    }

    pub fn scale(&self) -> f32 {
        f32::from_bits(self.scale_bits.load(Relaxed))
    }

    pub fn logical_size(&self) -> (u32, u32) {
        (self.width.load(Relaxed), self.height.load(Relaxed))
    }

    /// Tells CEF whether it is hidden: by the app or while frozen under a modal.
    pub fn apply_hidden(&self) {
        let hidden = (self.app_hidden.load(SeqCst) || self.frozen.load(SeqCst))
            && self.cdp.awake.load(SeqCst) == 0;
        if let Some(host) = self.host() {
            host.was_hidden(hidden as _);
        }
    }

    pub fn mode(&self) -> RenderMode {
        if self.gpu_mode.load(Relaxed) {
            RenderMode::Gpu
        } else {
            RenderMode::Cpu
        }
    }

    /// Wake iced so the widget redraws with the newest frame.
    pub fn wake(&self) {
        if let Some(w) = self.waker.lock().unwrap().as_ref() {
            w.wake();
        }
    }

    fn new_frame(&self) {
        self.frame_seq.fetch_add(1, Release);
        self.wake();
    }

    /// Creates (or re-creates after a GPU fallback) the CEF browser.
    fn create_browser(self: &Arc<Self>, url: &str) -> bool {
        let generation = self.generation.fetch_add(1, SeqCst) + 1;
        let gpu = self.gpu_mode.load(SeqCst);
        let window_info = WindowInfo {
            windowless_rendering_enabled: true as _,
            shared_texture_enabled: gpu as _,
            ..Default::default()
        };
        let browser_settings = BrowserSettings {
            windowless_frame_rate: runtime::options().frame_rate as _,
            background_color: argb(self.background),
            ..Default::default()
        };
        let handler = Handler {
            shared: self.clone(),
            generation,
        };
        let mut client = ClientBuilder::new(
            RenderHandlerBuilder::new(handler.clone()),
            DisplayHandlerBuilder::new(handler.clone()),
            LoadHandlerBuilder::new(handler.clone()),
            LifeSpanHandlerBuilder::new(handler),
        );
        let created = browser_host_create_browser(
            Some(&window_info),
            Some(&mut client),
            Some(&url.into()),
            Some(&browser_settings),
            None,
            None,
        );
        created == 1
    }

    /// Too many failed GPU imports: recreate the browser with software frames.
    fn fall_back_to_cpu(self: &Arc<Self>, reason: &str) {
        if !self.gpu_mode.swap(false, SeqCst) {
            return;
        }
        tracing::warn!(
            view = self.id.0,
            reason,
            "GPU import failed; falling back to CPU frames"
        );
        let url = self.url.lock().unwrap().clone();
        let old = self.browser.lock().unwrap().take();
        if let Some(host) = old.as_ref().and_then(|b| b.0.host()) {
            host.close_browser(1);
        }
        emit(self.id, WebEvent::RenderModeChanged(RenderMode::Cpu));
        let url = if url.is_empty() {
            "about:blank".to_string()
        } else {
            url
        };
        self.create_browser(&url);
    }
}

fn argb(c: Color) -> u32 {
    let [r, g, b, _] = c.into_rgba8();
    // Opaque: CEF OSR treats a transparent background as "transparent page".
    0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

#[derive(Clone)]
struct Handler {
    shared: Arc<ViewShared>,
    generation: u64,
}

impl Handler {
    fn current(&self) -> bool {
        self.shared.generation.load(SeqCst) == self.generation
    }
}

fn s(v: Option<&CefString>) -> String {
    v.map(|s| s.to_string()).unwrap_or_default()
}

/// The strings of a CEF string list the caller does not own.
fn string_list(list: &mut CefStringList) -> Vec<String> {
    let raw: *mut cef::sys::_cef_string_list_t = list.into();
    if raw.is_null() {
        return Vec::new();
    }
    // SAFETY: `raw` is the live list CEF passed to this callback; it is only read here, and each
    // value is copied out of a zeroed `cef_string_t` that CEF fills (and that we never free: the
    // list keeps ownership of the characters).
    unsafe {
        let count = cef::sys::cef_string_list_size(raw);
        (0..count)
            .filter_map(|i| {
                let mut value: cef::sys::cef_string_t = std::mem::zeroed();
                (cef::sys::cef_string_list_value(raw, i, &mut value) > 0)
                    .then(|| CefString::from(std::ptr::from_ref(&value)).to_string())
            })
            .collect()
    }
}

/// Largest icon side asked for (px): the detail area draws icons at 14 DIP, up to 200 %.
const FAVICON_PX: u32 = 32;

/// Download the page icon `urls[at]` (CEF tries the page's own cookies and cache); a failure
/// moves to the next one, and no usable icon at all reports `None`.
fn download_favicon(h: &Handler, seq: u64, urls: Vec<String>, at: usize) {
    let Some(url) = urls.get(at).cloned() else {
        emit(h.shared.id, WebEvent::FaviconChanged(None));
        return;
    };
    let Some(host) = h.shared.host() else { return };
    let mut callback = FaviconDownload::new(h.clone(), seq, urls, at);
    host.download_image(Some(&url.as_str().into()), 1, FAVICON_PX, 0, Some(&mut callback));
}

/// A downloaded image as straight RGBA, if it has pixels.
fn favicon_rgba(image: &Image) -> Option<Favicon> {
    if image.is_empty() == 1 {
        return None;
    }
    let (mut w, mut h) = (0, 0);
    let bin = image.as_bitmap(1.0, ColorType::RGBA_8888, AlphaType::POSTMULTIPLIED, Some(&mut w), Some(&mut h))?;
    let (width, height) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    let mut rgba = vec![0; bin.size()];
    let len = bin.data(Some(&mut rgba), 0);
    (width > 0 && height > 0 && len == (width * height * 4) as usize).then_some(Favicon { width, height, rgba })
}

wrap_download_image_callback! {
    struct FaviconDownload {
        h: Handler,
        seq: u64,
        urls: Vec<String>,
        at: usize,
    }

    impl DownloadImageCallback {
        fn on_download_image_finished(
            &self,
            _image_url: Option<&CefString>,
            _http_status_code: ::std::os::raw::c_int,
            image: Option<&mut Image>,
        ) {
            // The page has moved on (new icons or a new browser): this answer is stale.
            if !self.h.current() || self.h.shared.favicon_seq.load(SeqCst) != self.seq {
                return;
            }
            match image.and_then(|i| favicon_rgba(i)) {
                Some(f) => emit(self.h.shared.id, WebEvent::FaviconChanged(Some(f))),
                None => download_favicon(&self.h, self.seq, self.urls.clone(), self.at + 1),
            }
        }
    }
}

/// JS injected after each main-frame load when the caret probe is on: reports the caret of the
/// focused text field so the IME candidate window opens at it before the first composition.
const CARET_PROBE_JS: &str = r#"(()=>{if(window.__ideCaret)return;window.__ideCaret=1;
const cv=document.createElement('canvas').getContext('2d');
function caret(){const e=document.activeElement;if(!e||!('selectionStart' in e)||e.selectionStart==null)return;
const r=e.getBoundingClientRect(),cs=getComputedStyle(e);cv.font=cs.font;
const before=e.value.slice(0,e.selectionStart).split('\n');const line=before.length-1;
const lh=parseFloat(cs.fontSize)*1.25;
const x=r.left+parseFloat(cs.paddingLeft)+parseFloat(cs.borderLeftWidth)+cv.measureText(before[line]).width-e.scrollLeft;
const y=e.tagName=='INPUT'?r.top+(r.height-lh)/2:r.top+parseFloat(cs.paddingTop)+parseFloat(cs.borderTopWidth)+line*lh-e.scrollTop;
console.debug('__MENINX_CARET__ '+Math.round(x)+' '+Math.round(y)+' '+Math.round(lh));}
for(const t of ['focusin','click','keyup','input','selectionchange'])document.addEventListener(t,()=>setTimeout(caret,0),true);})()"#;
const CARET_PREFIX: &str = "__MENINX_CARET__ ";

wrap_render_handler! {
    struct RenderHandlerBuilder {
        h: Handler,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let (w, h) = self.h.shared.logical_size();
                rect.x = 0;
                rect.y = 0;
                rect.width = w.max(1) as i32;
                rect.height = h.max(1) as i32;
            }
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, screen_info: Option<&mut ScreenInfo>) -> ::std::os::raw::c_int {
            if let Some(info) = screen_info {
                info.device_scale_factor = self.h.shared.scale();
                return 1;
            }
            0
        }

        fn on_accelerated_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            info: Option<&AcceleratedPaintInfo>,
        ) {
            let sh = &self.h.shared;
            let Some(info) = info else { return };
            if type_ != PaintElementType::VIEW || !self.h.current() {
                return;
            }
            sh.counters.accel_paints.fetch_add(1, Relaxed);
            let t = Instant::now();
            match gpu::import(info) {
                Ok(Some(texture)) => {
                    let ns = t.elapsed().as_nanos() as u64;
                    sh.counters.imports.fetch_add(1, Relaxed);
                    sh.counters.import_ns.fetch_add(ns, Relaxed);
                    sh.counters.import_max_ns.fetch_max(ns, Relaxed);
                    sh.consecutive_import_failures.store(0, Relaxed);
                    let seq = sh.frame_seq.load(Acquire) + 1;
                    *sh.gpu_frame.lock().unwrap() = Some((texture, seq));
                    sh.new_frame();
                }
                // iced's device is not known yet (no frame drawn); CEF repaints soon.
                Ok(None) => sh.wake(),
                Err(e) => {
                    sh.counters.import_errors.fetch_add(1, Relaxed);
                    let n = sh.consecutive_import_failures.fetch_add(1, Relaxed) + 1;
                    if n <= 3 {
                        tracing::warn!(view = sh.id.0, "accelerated import failed: {e}");
                    }
                    if n >= 3 {
                        sh.fall_back_to_cpu(&e);
                    }
                }
            }
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            let sh = &self.h.shared;
            if buffer.is_null() || width <= 0 || height <= 0 || type_ != PaintElementType::VIEW || !self.h.current() {
                return;
            }
            let t = Instant::now();
            let len = width as usize * height as usize * 4;
            // SAFETY: CEF guarantees `buffer` holds width*height*4 bytes of BGRA for the
            // duration of this callback; we copy it out before returning.
            let src = unsafe { std::slice::from_raw_parts(buffer, len) };
            {
                let seq = sh.frame_seq.load(Acquire) + 1;
                let mut slot = sh.cpu_frame.lock().unwrap();
                match &mut *slot {
                    Some((f, s)) if f.bgra.len() == len => {
                        f.bgra.copy_from_slice(src);
                        f.width = width as u32;
                        f.height = height as u32;
                        *s = seq;
                    }
                    _ => *slot = Some((CpuFrame { bgra: src.to_vec(), width: width as u32, height: height as u32 }, seq)),
                }
            }
            sh.counters.cpu_paints.fetch_add(1, Relaxed);
            sh.counters.cpu_copy_ns.fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
            sh.new_frame();
        }

        fn on_ime_composition_range_changed(
            &self,
            _browser: Option<&mut Browser>,
            _selected_range: Option<&Range>,
            character_bounds: Option<&[Rect]>,
        ) {
            let Some(bounds) = character_bounds else { return };
            // Caret = right edge of the last composed character (or the first rect if empty).
            let r = bounds.last().or(bounds.first());
            if let Some(r) = r
                && r.height > 0 {
                    let x = (r.x + r.width) as f32;
                    self.h.shared.input.lock().unwrap().ime_caret =
                        Some(Rectangle { x, y: r.y as f32, width: 1.0, height: r.height as f32 });
                    self.h.shared.wake();
                }
        }
    }
}

wrap_display_handler! {
    struct DisplayHandlerBuilder {
        h: Handler,
    }

    impl DisplayHandler {
        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if !self.h.current() || frame.map(|f| f.is_main() != 1).unwrap_or(false) {
                return;
            }
            let url = s(url);
            *self.h.shared.url.lock().unwrap() = url.clone();
            emit(self.h.shared.id, WebEvent::UrlChanged(url));
        }

        fn on_favicon_urlchange(&self, _browser: Option<&mut Browser>, icon_urls: Option<&mut CefStringList>) {
            if !self.h.current() {
                return;
            }
            let urls = icon_urls.map(string_list).unwrap_or_default();
            let seq = self.h.shared.favicon_seq.fetch_add(1, SeqCst) + 1;
            download_favicon(&self.h, seq, urls, 0);
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            if !self.h.current() {
                return;
            }
            let title = s(title);
            *self.h.shared.title.lock().unwrap() = title.clone();
            emit(self.h.shared.id, WebEvent::TitleChanged(title));
        }

        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            level: LogSeverity,
            message: Option<&CefString>,
            source: Option<&CefString>,
            line: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            if !self.h.current() {
                return 1;
            }
            let message = s(message);
            if let Some(rest) = message.strip_prefix(CARET_PREFIX) {
                let v: Vec<f32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                if v.len() == 3 {
                    self.h.shared.input.lock().unwrap().probe_caret =
                        Some(Rectangle { x: v[0], y: v[1], width: 1.0, height: v[2] });
                    self.h.shared.wake();
                }
                return 1;
            }
            let level = if level == LogSeverity::ERROR || level == LogSeverity::FATAL {
                ConsoleLevel::Error
            } else if level == LogSeverity::WARNING {
                ConsoleLevel::Warning
            } else if level == LogSeverity::INFO || level == LogSeverity::DEFAULT {
                ConsoleLevel::Info
            } else {
                ConsoleLevel::Debug
            };
            let url = self.h.shared.url.lock().unwrap().clone();
            self.h.shared.cdp.log(crate::devtools::ConsoleEntry {
                level, message: message.clone(), source: s(source), line, url,
            });
            emit(self.h.shared.id, WebEvent::Console { level, message, source: s(source), line });
            1 // handled: don't also print to CEF's log/stdout
        }

        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: CursorHandle,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> ::std::os::raw::c_int {
            self.h.shared.cursor.store(crate::widget::cursor_index(type_), Relaxed);
            1
        }
    }
}

wrap_load_handler! {
    struct LoadHandlerBuilder {
        h: Handler,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            if !self.h.current() {
                return;
            }
            self.h.shared.loading.store(is_loading == 1, Relaxed);
            emit(self.h.shared.id, WebEvent::LoadingStateChanged {
                is_loading: is_loading == 1,
                can_go_back: can_go_back == 1,
                can_go_forward: can_go_forward == 1,
            });
        }

        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: ::std::os::raw::c_int) {
            if !self.h.current() {
                return;
            }
            let Some(frame) = frame else { return };
            if frame.is_main() != 1 {
                return;
            }
            if self.h.shared.caret_probe {
                frame.execute_java_script(Some(&CARET_PROBE_JS.into()), Some(&"meninx-web://caret-probe".into()), 0);
            }
            emit(self.h.shared.id, WebEvent::LoadEnd { http_status: http_status_code });
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            if !self.h.current() || frame.map(|f| f.is_main() != 1).unwrap_or(true) {
                return;
            }
            emit(self.h.shared.id, WebEvent::LoadError { url: s(failed_url), error: s(error_text) });
        }
    }
}

wrap_life_span_handler! {
    struct LifeSpanHandlerBuilder {
        h: Handler,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            // A popup would open an unparented native window; hand the URL to the app instead.
            emit(self.h.shared.id, WebEvent::NewWindowRequested(s(target_url)));
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            LIVE_BROWSERS.fetch_add(1, SeqCst);
            let sh = &self.h.shared;
            if !self.h.current() || sh.closed.load(SeqCst) {
                if let Some(host) = browser.host() {
                    host.close_browser(1);
                }
                return;
            }
            *sh.browser.lock().unwrap() = Some(SendBrowser(browser.clone()));
            if let Some(host) = browser.host() {
                host.was_resized();
                if sh.app_hidden.load(SeqCst) || sh.frozen.load(SeqCst) {
                    host.was_hidden(1);
                }
                if sh.input.lock().unwrap().focused {
                    host.set_focus(1);
                }
            }
            if let Some(url) = sh.pending_url.lock().unwrap().take()
                && let Some(frame) = browser.main_frame() {
                    frame.load_url(Some(&url.as_str().into()));
                }
            emit(sh.id, WebEvent::Created);
            emit(sh.id, WebEvent::RenderModeChanged(sh.mode()));
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            LIVE_BROWSERS.fetch_sub(1, SeqCst);
            if self.h.current() && self.h.shared.closed.load(SeqCst) {
                // Breaks the ViewShared -> Browser -> Client -> ViewShared cycle.
                *self.h.shared.browser.lock().unwrap() = None;
                emit(self.h.shared.id, WebEvent::Closed);
            }
        }
    }
}

wrap_client! {
    struct ClientBuilder {
        render: RenderHandler,
        display: DisplayHandler,
        load: LoadHandler,
        life_span: LifeSpanHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<cef::RenderHandler> {
            Some(self.render.clone())
        }
        fn display_handler(&self) -> Option<cef::DisplayHandler> {
            Some(self.display.clone())
        }
        fn load_handler(&self) -> Option<cef::LoadHandler> {
            Some(self.load.clone())
        }
        fn life_span_handler(&self) -> Option<cef::LifeSpanHandler> {
            Some(self.life_span.clone())
        }
    }
}

// ---------------------------------------------------------------- registry

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static REGISTRY: Mutex<Vec<Weak<ViewShared>>> = Mutex::new(Vec::new());

/// The view showing the browser with CEF identifier `id`.
pub(crate) fn view_of_browser(id: i32) -> Option<ViewId> {
    REGISTRY
        .lock()
        .unwrap()
        .iter()
        .filter_map(|w| w.upgrade())
        .find(|v| {
            v.browser
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|b| b.0.identifier() == id)
        })
        .map(|v| v.id)
}

/// Closes every live browser (used by `WebRuntime::shutdown`).
pub(crate) fn close_all() {
    let views: Vec<_> = REGISTRY
        .lock()
        .unwrap()
        .drain(..)
        .filter_map(|w| w.upgrade())
        .collect();
    for v in views {
        close_shared(&v);
    }
}

fn close_shared(sh: &ViewShared) {
    if sh.closed.swap(true, SeqCst) {
        return;
    }
    if let Some(host) = sh.host() {
        host.close_browser(1);
    }
    *sh.gpu_frame.lock().unwrap() = None;
    *sh.cpu_frame.lock().unwrap() = None;
}

// ---------------------------------------------------------------- public handle

/// Options for [`WebView::with_options`].
#[derive(Debug, Clone)]
pub struct ViewOptions {
    /// Initial size in logical pixels (the widget resizes the view to its layout bounds).
    pub size: Size,
    /// Initial device scale factor (the widget updates it from iced's viewport).
    pub scale_factor: f32,
    /// Inject a tiny script after each load that reports the caret of the focused text field
    /// (via a swallowed `console.debug`), so the IME window opens at the caret. Default: on.
    pub caret_probe: bool,
    /// Page background (opaque): what CEF paints before/behind the page, and the fill for the
    /// part of the widget a frame does not cover yet while a resize is in flight. Default white.
    pub background: Color,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            size: Size::new(800.0, 600.0),
            scale_factor: 1.0,
            caret_probe: true,
            background: Color::WHITE,
        }
    }
}

struct Owner(Arc<ViewShared>);

impl Drop for Owner {
    fn drop(&mut self) {
        close_shared(&self.0);
    }
}

/// Handle to one off-screen browser. Cheap to clone; the browser closes when the last clone is
/// dropped or [`WebView::close`] is called. Show it with [`crate::WebViewWidget`].
#[derive(Clone)]
pub struct WebView {
    owner: Arc<Owner>,
}

impl std::fmt::Debug for WebView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebView").field("id", &self.id()).finish()
    }
}

impl WebView {
    /// Creates a browser loading `url` with the given logical size and scale factor.
    /// Requires an initialised [`crate::WebRuntime`]. The CEF browser is created asynchronously;
    /// [`crate::WebEvent::Created`] is emitted when it exists (navigation before that is queued).
    pub fn new(
        _runtime: &crate::WebRuntime,
        url: &str,
        size: Size,
        scale_factor: f32,
    ) -> anyhow::Result<Self> {
        Self::with_options(
            _runtime,
            url,
            ViewOptions {
                size,
                scale_factor,
                ..Default::default()
            },
        )
    }

    /// Like [`WebView::new`] with all options.
    pub fn with_options(
        _runtime: &crate::WebRuntime,
        url: &str,
        options: ViewOptions,
    ) -> anyhow::Result<Self> {
        let id = ViewId(NEXT_ID.fetch_add(1, Relaxed));
        let gpu = !runtime::options().force_cpu;
        let shared = Arc::new(ViewShared {
            id,
            browser: Mutex::new(None),
            background: Color {
                a: 1.0,
                ..options.background
            },
            generation: AtomicU64::new(0),
            width: AtomicU32::new(options.size.width.round().max(1.0) as u32),
            height: AtomicU32::new(options.size.height.round().max(1.0) as u32),
            scale_bits: AtomicU32::new(options.scale_factor.max(0.25).to_bits()),
            widget_px: AtomicU64::new(0),
            frame_px: AtomicU64::new(0),
            resize_pending: Mutex::new(Vec::new()),
            app_hidden: AtomicBool::new(false),
            frozen: AtomicBool::new(false),
            dump_requested: AtomicBool::new(false),
            dump: Mutex::new(None),
            gpu_mode: AtomicBool::new(gpu),
            consecutive_import_failures: AtomicU32::new(0),
            gpu_frame: Mutex::new(None),
            cpu_frame: Mutex::new(None),
            frame_seq: AtomicU64::new(0),
            drawn_seq: AtomicU64::new(0),
            waker: Mutex::new(None),
            input: Mutex::new(InputState::default()),
            cursor: AtomicU8::new(0),
            url: Mutex::new(url.to_string()),
            title: Mutex::new(String::new()),
            loading: AtomicBool::new(false),
            pending_url: Mutex::new(None),
            favicon_seq: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            caret_probe: options.caret_probe,
            last_bounds: Mutex::new(None),
            counters: Counters::default(),
            cdp: Default::default(),
        });
        if !shared.create_browser(url) {
            anyhow::bail!("browser_host_create_browser failed");
        }
        REGISTRY.lock().unwrap().push(Arc::downgrade(&shared));
        Ok(Self {
            owner: Arc::new(Owner(shared)),
        })
    }

    pub(crate) fn shared(&self) -> &Arc<ViewShared> {
        &self.owner.0
    }

    pub fn id(&self) -> ViewId {
        self.shared().id
    }

    /// Loads `url` in the main frame.
    pub fn navigate(&self, url: &str) {
        let sh = self.shared();
        *sh.url.lock().unwrap() = url.to_string();
        match sh.browser().and_then(|b| b.main_frame()) {
            Some(frame) => frame.load_url(Some(&url.into())),
            None => *sh.pending_url.lock().unwrap() = Some(url.to_string()),
        }
    }

    pub fn reload(&self) {
        if let Some(b) = self.shared().browser() {
            b.reload();
        }
    }

    pub fn stop(&self) {
        if let Some(b) = self.shared().browser() {
            b.stop_load();
        }
    }

    pub fn go_back(&self) {
        if let Some(b) = self.shared().browser() {
            b.go_back();
        }
    }

    pub fn go_forward(&self) {
        if let Some(b) = self.shared().browser() {
            b.go_forward();
        }
    }

    /// Runs JavaScript in the main frame.
    pub fn execute_js(&self, code: &str) {
        if let Some(frame) = self.shared().browser().and_then(|b| b.main_frame()) {
            frame.execute_java_script(Some(&code.into()), Some(&"meninx-web://execute".into()), 0);
        }
    }

    /// Resizes the view (logical pixels). The widget does this automatically from its layout.
    pub fn resize(&self, size: Size) {
        let sh = self.shared();
        let (w, h) = (
            size.width.round().max(1.0) as u32,
            size.height.round().max(1.0) as u32,
        );
        let old_w = sh.width.swap(w, Relaxed);
        let old_h = sh.height.swap(h, Relaxed);
        if (old_w, old_h) != (w, h)
            && let Some(host) = sh.host()
        {
            host.was_resized();
        }
    }

    /// Sets the device scale factor. The widget does this automatically from iced's viewport.
    pub fn set_scale_factor(&self, scale: f32) {
        let sh = self.shared();
        let scale = scale.max(0.25);
        if sh.scale_bits.swap(scale.to_bits(), Relaxed) != scale.to_bits()
            && let Some(host) = sh.host()
        {
            host.notify_screen_info_changed();
            host.was_resized();
        }
    }

    /// Gives or removes keyboard focus (the widget also focuses on click and blurs on outside
    /// clicks). Removing focus finishes an active IME composition (like Chrome).
    pub fn set_focus(&self, focused: bool) {
        crate::widget::set_focus(self.shared(), focused);
    }

    pub fn is_focused(&self) -> bool {
        self.shared().input.lock().unwrap().focused
    }

    /// Hides/shows the browser (stops painting while hidden, e.g. for an inactive tab). A view
    /// frozen by [`crate::WebViewWidget::dimmed`] stays hidden to CEF until it is unfrozen.
    pub fn set_hidden(&self, hidden: bool) {
        let sh = self.shared();
        sh.app_hidden.store(hidden, SeqCst);
        sh.apply_hidden();
    }

    /// Asks the widget to copy the frame it draws next back to the CPU (test/diagnostics; it
    /// stalls that frame until the GPU copy is done). Fetch it with [`WebView::take_frame_dump`].
    pub fn request_frame_dump(&self) {
        let sh = self.shared();
        *sh.dump.lock().unwrap() = None;
        sh.dump_requested.store(true, SeqCst);
        sh.wake();
    }

    /// The frame copied after [`WebView::request_frame_dump`], once available.
    pub fn take_frame_dump(&self) -> Option<FrameDump> {
        self.shared().dump.lock().unwrap().take()
    }

    /// DevTools protocol access to the page (the agent bridge's browser tools).
    pub fn devtools(&self) -> crate::DevTools {
        crate::DevTools(self.shared().clone())
    }

    /// Opens Chrome DevTools in a separate native window.
    pub fn show_dev_tools(&self) {
        if let Some(host) = self.shared().host() {
            let info = WindowInfo::default();
            host.show_dev_tools(Some(&info), None, None, None);
        }
    }

    /// Closes the browser now ([`crate::WebEvent::Closed`] follows).
    pub fn close(&self) {
        close_shared(self.shared());
    }

    pub fn url(&self) -> String {
        self.shared().url.lock().unwrap().clone()
    }

    pub fn title(&self) -> String {
        self.shared().title.lock().unwrap().clone()
    }

    pub fn is_loading(&self) -> bool {
        self.shared().loading.load(Relaxed)
    }

    /// `true` once the CEF browser exists.
    pub fn is_created(&self) -> bool {
        self.shared().browser.lock().unwrap().is_some()
    }

    /// Layout bounds of the widget showing this view at its last draw (iced layout coordinates,
    /// before scrollable translation). `None` until drawn.
    pub fn last_bounds(&self) -> Option<Rectangle> {
        *self.shared().last_bounds.lock().unwrap()
    }

    /// Caret rectangle (view coordinates) the IME window is placed at: the last composition
    /// bounds from CEF, else the caret-probe position.
    pub fn ime_caret(&self) -> Option<Rectangle> {
        let input = self.shared().input.lock().unwrap();
        if input.composing.is_some() {
            input.ime_caret.or(input.probe_caret)
        } else {
            input.probe_caret.or(input.ime_caret)
        }
    }

    /// Text of the IME composition currently shown in the page, if any.
    pub fn composition(&self) -> Option<String> {
        self.shared().input.lock().unwrap().composing.clone()
    }

    pub fn render_mode(&self) -> RenderMode {
        self.shared().mode()
    }

    /// Returns and resets the render counters.
    pub fn take_stats(&self) -> RenderStats {
        let sh = self.shared();
        let c = &sh.counters;
        let resizes = c.resizes.swap(0, Relaxed);
        let resize_frames = c.resize_frames.swap(0, Relaxed);
        let resize_ns = c.resize_ns.swap(0, Relaxed);
        let imports = c.imports.swap(0, Relaxed);
        let import_ns = c.import_ns.swap(0, Relaxed);
        let cpu_paints = c.cpu_paints.swap(0, Relaxed);
        let cpu_ns = c.cpu_copy_ns.swap(0, Relaxed);
        RenderStats {
            mode: Some(self.render_mode()),
            accel_paints: c.accel_paints.swap(0, Relaxed),
            cpu_paints,
            imports,
            import_avg_ms: if imports > 0 {
                import_ns as f64 / imports as f64 / 1e6
            } else {
                0.0
            },
            import_max_ms: c.import_max_ns.swap(0, Relaxed) as f64 / 1e6,
            import_errors: c.import_errors.swap(0, Relaxed),
            cpu_copy_avg_ms: if cpu_paints > 0 {
                cpu_ns as f64 / cpu_paints as f64 / 1e6
            } else {
                0.0
            },
            frames_shown: c.frames_shown.swap(0, Relaxed),
            resizes,
            resize_lag_frames_avg: if resizes > 0 {
                resize_frames as f64 / resizes as f64
            } else {
                0.0
            },
            resize_lag_frames_max: c.resize_frames_max.swap(0, Relaxed),
            resize_lag_ms_avg: if resizes > 0 {
                resize_ns as f64 / resizes as f64 / 1e6
            } else {
                0.0
            },
            resize_lag_ms_max: c.resize_max_ns.swap(0, Relaxed) as f64 / 1e6,
            frame_px: unpack(sh.frame_px.load(Relaxed)),
            widget_px: unpack(sh.widget_px.load(Relaxed)),
        }
    }
}

pub(crate) fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}
