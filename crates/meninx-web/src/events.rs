//! Browser events delivered to iced through [`events`].

use iced::Subscription;
use iced::futures::channel::mpsc;
use iced::futures::stream::{self, StreamExt as _};
use std::sync::Mutex;

/// Identifies one [`crate::WebView`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewId(pub u64);

/// How a view's frames reach the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// CEF shared D3D11 texture imported into iced's wgpu device (no CPU pixel copies).
    Gpu,
    /// CEF software `on_paint` BGRA frames uploaded with `queue.write_texture`.
    Cpu,
}

/// Console message severity (mirrors CEF `LogSeverity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleLevel {
    Debug,
    Info,
    Warning,
    Error,
}

/// Something that happened in a browser.
#[derive(Debug, Clone, PartialEq)]
pub enum WebEvent {
    /// The CEF browser exists; input and navigation now take effect.
    Created,
    TitleChanged(String),
    UrlChanged(String),
    LoadingStateChanged {
        is_loading: bool,
        can_go_back: bool,
        can_go_forward: bool,
    },
    /// Main frame finished loading.
    LoadEnd {
        http_status: i32,
    },
    LoadError {
        url: String,
        error: String,
    },
    Console {
        level: ConsoleLevel,
        message: String,
        source: String,
        line: i32,
    },
    /// The page asked for a new window (`target=_blank`, `window.open`). Popups are blocked;
    /// the app decides what to do with the URL (e.g. `navigate` or open a new tab).
    NewWindowRequested(String),
    /// The render path changed (e.g. GPU import failed and the view fell back to CPU frames).
    RenderModeChanged(RenderMode),
    /// The browser was closed (after [`crate::WebView::close`] or when the last handle dropped).
    Closed,
}

/// A [`WebEvent`] tagged with the view it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct WebViewEvent {
    pub view: ViewId,
    pub event: WebEvent,
}

struct Hub {
    subscribers: Vec<mpsc::UnboundedSender<WebViewEvent>>,
    /// Events emitted while nobody listened yet (bounded).
    backlog: Vec<WebViewEvent>,
}

static HUB: Mutex<Hub> = Mutex::new(Hub {
    subscribers: Vec::new(),
    backlog: Vec::new(),
});
const BACKLOG_MAX: usize = 256;

pub(crate) fn emit(view: ViewId, event: WebEvent) {
    let ev = WebViewEvent { view, event };
    let mut hub = HUB.lock().unwrap();
    hub.subscribers.retain(|tx| !tx.is_closed());
    if hub.subscribers.is_empty() {
        if hub.backlog.len() < BACKLOG_MAX {
            hub.backlog.push(ev);
        }
        return;
    }
    for tx in &hub.subscribers {
        let _ = tx.unbounded_send(ev.clone());
    }
}

fn subscribe() -> mpsc::UnboundedReceiver<WebViewEvent> {
    let (tx, rx) = mpsc::unbounded();
    let mut hub = HUB.lock().unwrap();
    for ev in hub.backlog.drain(..) {
        let _ = tx.unbounded_send(ev);
    }
    hub.subscribers.push(tx);
    rx
}

/// Events of every [`crate::WebView`] (title, URL, loading state, console, ...).
///
/// ```ignore
/// fn subscription(&self) -> Subscription<Message> {
///     meninx_web::events().map(Message::Web)
/// }
/// ```
pub fn events() -> Subscription<WebViewEvent> {
    Subscription::run(|| stream::once(async { subscribe() }).flatten())
}
