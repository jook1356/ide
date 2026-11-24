//! DevTools protocol access to one view's browser ([`DevTools`]), for the agent bridge's browser
//! tools (`docs/browser-control.md`): CDP requests go through `send_dev_tools_message` on CEF's
//! UI thread, answers come back through a message observer registered per browser. Input sent
//! this way (`Input.dispatch*Event`) stays inside the browser: it is not OS input.

use cef::*;
use iced::futures::channel::oneshot;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering::*};
use std::sync::{Arc, Mutex, Weak};

use crate::cdp;
use crate::events::ConsoleLevel;
use crate::view::ViewShared;

/// How many console messages a view keeps.
const CONSOLE_KEEP: usize = 300;

static NEXT_MESSAGE: AtomicI32 = AtomicI32::new(1);

type Answer = Result<Value, String>;

/// A console message of a page (`console.*`, uncaught errors), kept per view.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleEntry {
    pub level: ConsoleLevel,
    pub message: String,
    pub source: String,
    pub line: i32,
    /// The page's address when it was logged.
    pub url: String,
}

/// `cef::Registration` keeps the observer registered while it lives.
pub(crate) struct SendRegistration(#[allow(dead_code)] Registration);
// SAFETY: a CEF registration is a ref-counted CEF object (atomic counts); dropping the last
// reference removes the observer, which CEF does on its UI thread itself. It is only moved and
// dropped here, never used otherwise.
unsafe impl Send for SendRegistration {}
// SAFETY: see above; no method is called through a shared reference.
unsafe impl Sync for SendRegistration {}

/// Per-view DevTools state (in [`ViewShared`]).
#[derive(Default)]
pub(crate) struct CdpState {
    /// Requests waiting for their answer, by message id.
    pending: Arc<Mutex<HashMap<i32, oneshot::Sender<Answer>>>>,
    /// The observer's registration and the browser generation it was made for.
    registration: Mutex<Option<(u64, SendRegistration)>>,
    /// Recent console messages, oldest first.
    console: Mutex<VecDeque<ConsoleEntry>>,
    /// Live [`Awake`] guards: the page paints although the app hid it.
    pub(crate) awake: AtomicU32,
}

impl CdpState {
    pub(crate) fn log(&self, entry: ConsoleEntry) {
        let mut c = self.console.lock().unwrap();
        if c.len() >= CONSOLE_KEEP {
            c.pop_front();
        }
        c.push_back(entry);
    }

    fn fail(&self, id: i32, why: &str) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
            let _ = tx.send(Err(why.to_owned()));
        }
    }
}

wrap_dev_tools_message_observer! {
    struct Observer {
        pending: Weak<Mutex<HashMap<i32, oneshot::Sender<Answer>>>>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            let Some(pending) = self.pending.upgrade() else { return };
            let tx = pending.lock().unwrap().remove(&message_id);
            if let Some(tx) = tx {
                let _ = tx.send(cdp::answer(success == 1, result.unwrap_or_default()));
            }
        }
    }
}

type Job = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

wrap_task! {
    struct UiTask {
        job: Job,
    }

    impl Task {
        fn execute(&self) {
            if let Some(job) = self.job.lock().unwrap().take() {
                job();
            }
        }
    }
}

/// Runs `job` on CEF's UI thread (now when already there).
fn on_ui(job: impl FnOnce() + Send + 'static) {
    if currently_on(ThreadId::UI) == 1 {
        job();
        return;
    }
    let mut task = UiTask::new(Arc::new(Mutex::new(Some(Box::new(job)))));
    post_task(ThreadId::UI, Some(&mut task));
}

/// DevTools protocol access to a [`crate::WebView`]'s page (from [`crate::WebView::devtools`]).
/// It does not keep the browser open: calls fail once the view is closed.
#[derive(Clone)]
pub struct DevTools(pub(crate) Arc<ViewShared>);

impl DevTools {
    /// Sends one CDP request (`method`, `params` object or null) and resolves to its `result`
    /// object or the protocol's error. Never resolves only when CEF never answers: callers put
    /// a timeout around it.
    pub fn call(
        &self,
        method: &str,
        params: Value,
    ) -> impl std::future::Future<Output = Answer> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        let id = NEXT_MESSAGE.fetch_add(1, Relaxed);
        {
            let mut p = self.0.cdp.pending.lock().unwrap();
            // Callers that gave up (timeouts) leave their entries behind.
            p.retain(|_, tx| !tx.is_canceled());
            p.insert(id, tx);
        }
        let sh = self.0.clone();
        let msg = cdp::request(id, method, &params);
        on_ui(move || {
            if sh.closed.load(SeqCst) {
                return sh.cdp.fail(id, "the page is closed");
            }
            let Some(host) = sh.host() else {
                return sh.cdp.fail(id, "the page is not ready yet");
            };
            let generation = sh.generation.load(SeqCst);
            {
                let mut reg = sh.cdp.registration.lock().unwrap();
                if reg.as_ref().map(|r| r.0) != Some(generation) {
                    let mut observer = Observer::new(Arc::downgrade(&sh.cdp.pending));
                    *reg = host
                        .add_dev_tools_message_observer(Some(&mut observer))
                        .map(|r| (generation, SendRegistration(r)));
                }
            }
            if host.send_dev_tools_message(Some(&msg)) != 1 {
                sh.cdp.fail(id, "DevTools did not take the request");
            }
        });
        async move { rx.await.unwrap_or_else(|_| Err("the page went away".into())) }
    }

    /// `Page.captureScreenshot` as PNG (base64). A hidden view (another tab is shown) paints
    /// while the capture runs.
    pub fn screenshot(&self) -> impl std::future::Future<Output = Answer> + Send + 'static {
        let awake = self.awake();
        let call = self.call(
            "Page.captureScreenshot",
            serde_json::json!({"format": "png", "captureBeyondViewport": false}),
        );
        async move {
            let r = call.await;
            drop(awake);
            r
        }
    }

    /// Keeps the page painting and taking input while the guard lives, also when it is hidden
    /// (another tab is shown, a dialog dims it): an agent acts on it or captures it.
    pub fn awake(&self) -> Awake {
        self.0.cdp.awake.fetch_add(1, SeqCst);
        self.0.apply_hidden();
        if let Some(host) = self.0.host() {
            host.invalidate(PaintElementType::VIEW);
        }
        Awake(self.0.clone())
    }

    /// The page's recent console messages, oldest first.
    pub fn console(&self) -> Vec<ConsoleEntry> {
        self.0.cdp.console.lock().unwrap().iter().cloned().collect()
    }

    /// The page's address (what it shows or is loading).
    pub fn url(&self) -> String {
        self.0.url.lock().unwrap().clone()
    }

    pub fn is_loading(&self) -> bool {
        self.0.loading.load(Relaxed)
    }

    /// `true` once the CEF browser exists (calls before that fail).
    pub fn is_created(&self) -> bool {
        self.0.browser.lock().unwrap().is_some()
    }
}

/// From [`DevTools::awake`]: the page may hide again when the last one is dropped.
pub struct Awake(Arc<ViewShared>);

impl Drop for Awake {
    fn drop(&mut self) {
        self.0.cdp.awake.fetch_sub(1, SeqCst);
        self.0.apply_hidden();
    }
}
