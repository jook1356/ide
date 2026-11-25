//! Custom URL schemes answered by the app (e.g. `meninx-widget://`): the app names them before
//! CEF starts ([`register_custom_schemes`], every process needs the list) and answers their
//! requests with a [`SchemeHandler`] ([`set_scheme_handler`]). Requests are answered
//! asynchronously: the handler gets a [`Responder`] and may answer from any thread later.
//!
//! The schemes are registered as standard, secure, CORS- and fetch-enabled: a page served
//! from one has its own origin (`<scheme>://<host>`), relative URLs, `localStorage` and
//! `fetch()` to its own origin.

use cef::{rc::Rc as _, *};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::events::ViewId;

static SCHEMES: OnceLock<Vec<String>> = OnceLock::new();
static HANDLERS: Mutex<Option<HashMap<String, SchemeHandler>>> = Mutex::new(None);

/// Name the app's custom schemes. Call first in `main`, before
/// [`crate::maybe_run_subprocess`]: CEF's helper processes (renderers) need the same list.
/// Later calls are ignored.
pub fn register_custom_schemes(names: &[&str]) {
    let _ = SCHEMES.set(names.iter().map(|s| s.to_ascii_lowercase()).collect());
}

/// The answering function of a scheme. Called on a CEF thread: it must not block; answer
/// through the [`Responder`] (now or later, from any thread).
pub type SchemeHandler = Arc<dyn Fn(SchemeRequest, Responder) + Send + Sync>;

/// Set (or replace) the handler of a scheme named in [`register_custom_schemes`]. Requests of
/// a scheme without a handler get a 404.
pub fn set_scheme_handler(scheme: &str, handler: SchemeHandler) {
    HANDLERS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(scheme.to_ascii_lowercase(), handler);
}

/// A request for a custom scheme URL.
#[derive(Debug, Clone)]
pub struct SchemeRequest {
    /// The [`crate::WebView`] whose page asked (`None`: not one of ours, e.g. DevTools).
    pub view: Option<ViewId>,
    pub url: String,
    /// `GET`, `POST`, ...
    pub method: String,
    /// The request body (bytes elements of the upload data).
    pub body: Vec<u8>,
    /// URL of the frame that asked (the page's own URL for its subresources and `fetch()`;
    /// for a navigation, the frame's URL before it).
    pub frame_url: String,
}

/// The answer to a [`SchemeRequest`].
#[derive(Debug, Clone)]
pub struct SchemeResponse {
    pub status: u16,
    /// Media type, e.g. `text/html; charset=utf-8`.
    pub mime: String,
    pub body: Vec<u8>,
}

impl SchemeResponse {
    pub fn new(status: u16, mime: impl Into<String>, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            mime: mime.into(),
            body: body.into(),
        }
    }

    /// A plain-text answer.
    pub fn text(status: u16, text: impl Into<String>) -> Self {
        Self::new(status, "text/plain; charset=utf-8", text.into().into_bytes())
    }
}

type Deliver = Box<dyn FnOnce(SchemeResponse) + Send>;

struct ResponderInner(Mutex<Option<Deliver>>);

impl Drop for ResponderInner {
    fn drop(&mut self) {
        // Never answered: do not leave the page waiting.
        if let Some(d) = self.0.lock().unwrap().take() {
            d(SchemeResponse::text(503, "no answer"));
        }
    }
}

/// Answers one [`SchemeRequest`] (the first answer counts; dropping every clone without an
/// answer answers 503).
#[derive(Clone)]
pub struct Responder(Arc<ResponderInner>);

impl std::fmt::Debug for Responder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Responder")
    }
}

impl Responder {
    fn new(deliver: impl FnOnce(SchemeResponse) + Send + 'static) -> Self {
        Responder(Arc::new(ResponderInner(Mutex::new(Some(Box::new(deliver))))))
    }

    /// For tests of handlers: a responder that hands its answer to `f`.
    pub fn testing(f: impl FnOnce(SchemeResponse) + Send + 'static) -> Self {
        Self::new(f)
    }

    pub fn respond(&self, r: SchemeResponse) {
        if let Some(d) = self.0.0.lock().unwrap().take() {
            d(r);
        }
    }
}

/// Register the schemes with CEF (`CefApp::OnRegisterCustomSchemes`, every process).
pub(crate) fn on_register(registrar: &mut SchemeRegistrar) {
    let opts = SchemeOptions::STANDARD.get_raw()
        | SchemeOptions::SECURE.get_raw()
        | SchemeOptions::CORS_ENABLED.get_raw()
        | SchemeOptions::FETCH_ENABLED.get_raw();
    for name in SCHEMES.get().into_iter().flatten() {
        registrar.add_custom_scheme(Some(&name.as_str().into()), opts);
    }
}

/// Install the handler factories (browser process, after `cef::initialize`).
pub(crate) fn install_factories() {
    for name in SCHEMES.get().into_iter().flatten() {
        let mut f = FactoryBuilder::new(name.clone());
        register_scheme_handler_factory(Some(&name.as_str().into()), None, Some(&mut f));
    }
}

/// `cef::Callback` is ref-counted and thread-safe in CEF; the Rust wrapper is just not marked so.
struct SendCallback(Callback);
// SAFETY: CefCallback is a ref-counted CEF object with atomic counts; Continue()/Cancel() may
// be called on any thread (CEF posts to the right one).
unsafe impl Send for SendCallback {}

/// What a resource handler will send: the response and how much of its body was read.
#[derive(Default)]
struct Answer {
    response: Option<SchemeResponse>,
    offset: usize,
}

fn read_body(request: &Request) -> Vec<u8> {
    let mut out = Vec::new();
    let Some(post) = request.post_data() else {
        return out;
    };
    // cef-rs fetches as many elements as the vector holds.
    let mut elements: Vec<Option<PostDataElement>> = (0..post.element_count()).map(|_| None).collect();
    post.elements(Some(&mut elements));
    for e in elements.into_iter().flatten() {
        let n = e.bytes_count();
        if n == 0 {
            continue;
        }
        let start = out.len();
        out.resize(start + n, 0);
        let got = e.bytes(n, out[start..].as_mut_ptr());
        out.truncate(start + got);
    }
    out
}

wrap_scheme_handler_factory! {
    struct FactoryBuilder {
        scheme: String,
    }

    impl SchemeHandlerFactory {
        fn create(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let request = request?;
            let req = SchemeRequest {
                view: browser.and_then(|b| crate::view::view_of_browser(b.identifier())),
                url: CefString::from(&request.url()).to_string(),
                method: CefString::from(&request.method()).to_string(),
                body: read_body(request),
                frame_url: frame.map(|f| CefString::from(&f.url()).to_string()).unwrap_or_default(),
            };
            let handler = HANDLERS.lock().unwrap().as_ref().and_then(|h| h.get(&self.scheme).cloned());
            Some(ResourceHandlerBuilder::new(Arc::new(Mutex::new(Answer::default())), Arc::new(Mutex::new(Some((req, handler))))))
        }
    }
}

type Pending = Option<(SchemeRequest, Option<SchemeHandler>)>;

wrap_resource_handler! {
    struct ResourceHandlerBuilder {
        answer: Arc<Mutex<Answer>>,
        pending: Arc<Mutex<Pending>>,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut ::std::os::raw::c_int>,
            callback: Option<&mut Callback>,
        ) -> ::std::os::raw::c_int {
            let Some((req, handler)) = self.pending.lock().unwrap().take() else {
                return 0;
            };
            let Some(handle_request) = handle_request else {
                return 0;
            };
            let (Some(callback), Some(handler)) = (callback, handler) else {
                // No handler: answered at once.
                self.answer.lock().unwrap().response = Some(SchemeResponse::text(404, "not found"));
                *handle_request = 1;
                return 1;
            };
            // Answered later: CEF waits for `Continue()`.
            *handle_request = 0;
            let answer = self.answer.clone();
            let cb = SendCallback(callback.clone());
            let responder = Responder::new(move |r| {
                answer.lock().unwrap().response = Some(r);
                cb.0.cont();
            });
            handler(req, responder);
            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            let a = self.answer.lock().unwrap();
            let r = a.response.clone().unwrap_or_else(|| SchemeResponse::text(500, "no answer"));
            if let Some(resp) = response {
                resp.set_status(i32::from(r.status));
                // CEF takes the bare media type and the charset apart.
                let mut parts = r.mime.split(';');
                let mime = parts.next().unwrap_or("").trim();
                resp.set_mime_type(Some(&mime.into()));
                if let Some(cs) = parts.find_map(|p| p.trim().strip_prefix("charset=")) {
                    resp.set_charset(Some(&cs.trim_matches('"').into()));
                }
                // Widgets change while they are open: never cache.
                resp.set_header_by_name(Some(&"Cache-Control".into()), Some(&"no-store".into()), 1);
            }
            if let Some(len) = response_length {
                *len = r.body.len() as i64;
            }
        }

        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: ::std::os::raw::c_int,
            bytes_read: Option<&mut ::std::os::raw::c_int>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> ::std::os::raw::c_int {
            let mut a = self.answer.lock().unwrap();
            let Answer { response, offset } = &mut *a;
            let body = response.as_ref().map(|r| r.body.as_slice()).unwrap_or_default();
            let left = body.len().saturating_sub(*offset);
            let n = left.min(bytes_to_read.max(0) as usize);
            if n > 0 && !data_out.is_null() {
                // SAFETY: CEF hands a buffer of `bytes_to_read` writable bytes; `n` is at most that,
                // and the source range is inside `body`.
                unsafe { std::ptr::copy_nonoverlapping(body[*offset..].as_ptr(), data_out, n) };
            }
            *offset += n;
            if let Some(br) = bytes_read {
                *br = n as i32;
            }
            // 0 with bytes_read 0: the response is complete.
            i32::from(n > 0)
        }

        fn cancel(&self) {
            self.pending.lock().unwrap().take();
        }
    }
}
