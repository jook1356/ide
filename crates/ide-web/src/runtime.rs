//! CEF process bootstrap: the helper-subprocess hook, runtime initialisation and shutdown.
//!
//! CEF runs with `multi_threaded_message_loop`: it owns its own UI thread, so iced's event loop is
//! never re-entered (calling `do_message_loop_work()` from iced's `update()` hangs, see spike 001b).
//! All CEF callbacks therefore arrive on the CEF UI thread and only touch `Mutex`/atomic state.

use anyhow::{Context as _, Result, bail};
use cef::{args::Args, rc::Rc as _, *};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::view;

/// Set once [`initialize`] succeeded; cleared by [`WebRuntime::shutdown`].
static INITIALIZED: AtomicBool = AtomicBool::new(false);
/// Number of live CEF browsers (incremented in `on_after_created`, decremented in `on_before_close`).
pub(crate) static LIVE_BROWSERS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
struct IdeApp;

wrap_app! {
    struct AppBuilder {
        app: IdeApp,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&cef::CefStringUtf16>,
            command_line: Option<&mut cef::CommandLine>,
        ) {
            let Some(command_line) = command_line else { return };
            for switch in ["no-startup-window", "noerrdialogs", "hide-crash-restore-bubble", "use-mock-keychain"] {
                command_line.append_switch(Some(&switch.into()));
            }
        }
    }
}

fn new_app() -> cef::App {
    AppBuilder::new(IdeApp)
}

/// Must be the first call in `main`.
///
/// Returns `Some(exit_code)` when this process is a CEF helper subprocess (renderer, GPU,
/// utility...). The caller must then return that code immediately without creating any window.
/// Returns `None` in the browser (main) process.
pub fn maybe_run_subprocess() -> Option<i32> {
    // Pins the CEF API version; must precede any other CEF call.
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = Args::new();
    let is_browser = args
        .as_cmd_line()
        .map(|cmd| cmd.has_switch(Some(&CefString::from("type"))) != 1)
        .unwrap_or(true);
    let mut app = new_app();
    let code = execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if is_browser {
        // -1 = "this is the browser process, continue".
        None
    } else {
        Some(code.max(0))
    }
}

/// Settings for [`initialize`].
#[derive(Debug, Clone)]
pub struct RuntimeSettings {
    /// App data directory. CEF's root cache goes to `<data_dir>/cef`, the profile cache to
    /// `<data_dir>/cef/default` and the log to `<data_dir>/cef/cef.log`.
    pub data_dir: PathBuf,
    /// Enables Chrome DevTools remote debugging on this port (`http://127.0.0.1:<port>`).
    pub remote_debugging_port: Option<u16>,
    /// Skip the shared-texture (GPU) path and always use CEF's software `on_paint` frames.
    /// Also forced by the env var `IDE_WEB_FORCE_CPU=1`.
    pub force_cpu: bool,
    /// Windowless frame rate requested from CEF (1..=60).
    pub frame_rate: u32,
    /// CEF log severity (`LogSeverity::WARNING` by default).
    pub log_severity: LogSeverity,
}

impl RuntimeSettings {
    /// Defaults with the given data directory.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            remote_debugging_port: None,
            force_cpu: false,
            frame_rate: 60,
            log_severity: LogSeverity::WARNING,
        }
    }
}

/// Runtime-wide options the views read.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeOptions {
    pub force_cpu: bool,
    pub frame_rate: u32,
}

pub(crate) static OPTIONS: std::sync::OnceLock<RuntimeOptions> = std::sync::OnceLock::new();

/// The initialised CEF runtime (a cheap, clonable handle). Create views with
/// [`crate::WebView::new`]; call [`WebRuntime::shutdown`] after the iced event loop returned
/// (it also runs when the last handle is dropped).
#[derive(Debug, Clone)]
pub struct WebRuntime {
    inner: std::sync::Arc<RuntimeInner>,
}

#[derive(Debug)]
struct RuntimeInner {
    shut_down: AtomicBool,
}

/// Initialises CEF in the browser process (call after [`maybe_run_subprocess`] returned `None`,
/// on the main thread, before starting iced).
pub fn initialize(settings: RuntimeSettings) -> Result<WebRuntime> {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        bail!("ide_web::initialize called twice");
    }
    let root = settings.data_dir.join("cef");
    let cache = root.join("default");
    std::fs::create_dir_all(&cache).with_context(|| format!("creating {}", cache.display()))?;
    let path_str = |p: &std::path::Path| CefString::from(p.to_string_lossy().as_ref());

    let force_cpu = settings.force_cpu
        || std::env::var("IDE_WEB_FORCE_CPU").is_ok_and(|v| !v.is_empty() && v != "0");
    let _ = OPTIONS.set(RuntimeOptions {
        force_cpu,
        frame_rate: settings.frame_rate.clamp(1, 60),
    });

    let cef_settings = Settings {
        windowless_rendering_enabled: true as _,
        // CEF owns its UI thread; iced's loop is never re-entered.
        multi_threaded_message_loop: true as _,
        no_sandbox: true as _,
        root_cache_path: path_str(&root),
        cache_path: path_str(&cache),
        log_file: path_str(&root.join("cef.log")),
        log_severity: settings.log_severity,
        remote_debugging_port: settings.remote_debugging_port.map(i32::from).unwrap_or(0),
        ..Default::default()
    };
    let args = Args::new();
    let mut app = new_app();
    let ok = cef::initialize(
        Some(args.as_main_args()),
        Some(&cef_settings),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if ok != 1 {
        INITIALIZED.store(false, Ordering::SeqCst);
        bail!(
            "cef::initialize failed (code {ok}); is another instance using {}?",
            root.display()
        );
    }
    tracing::info!(force_cpu, root = %root.display(), "CEF initialised");
    Ok(WebRuntime {
        inner: std::sync::Arc::new(RuntimeInner {
            shut_down: AtomicBool::new(false),
        }),
    })
}

impl WebRuntime {
    /// Closes every browser, waits (up to 3 s) for CEF to release them and shuts CEF down.
    /// Must run on the thread that called [`initialize`].
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }
}

impl RuntimeInner {
    fn shutdown(&self) {
        if self.shut_down.swap(true, Ordering::SeqCst) {
            return;
        }
        view::close_all();
        let t = Instant::now();
        while LIVE_BROWSERS.load(Ordering::SeqCst) > 0 && t.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Give CEF a moment to run the final close tasks on its UI thread.
        std::thread::sleep(Duration::from_millis(100));
        cef::shutdown();
        INITIALIZED.store(false, Ordering::SeqCst);
        tracing::info!(left = LIVE_BROWSERS.load(Ordering::SeqCst), "CEF shut down");
    }
}

impl Drop for RuntimeInner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn options() -> RuntimeOptions {
    OPTIONS.get().copied().unwrap_or(RuntimeOptions {
        force_cpu: false,
        frame_rate: 60,
    })
}
