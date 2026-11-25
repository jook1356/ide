//! CEF runtime and an iced widget that shows one off-screen-rendered browser.
//!
//! ```ignore
//! fn main() -> anyhow::Result<()> {
//!     if let Some(code) = meninx_web::maybe_run_subprocess() {
//!         std::process::exit(code); // CEF helper process
//!     }
//!     let runtime = meninx_web::initialize(meninx_web::RuntimeSettings::new(data_dir))?;
//!     // ... create views with WebView::new(&runtime, url, size, scale) inside the iced app,
//!     //     show them with WebViewWidget::new(&view), listen with meninx_web::events() ...
//!     iced::application(..).backend(meninx_web::backend()).run()?;
//!     runtime.shutdown();
//!     Ok(())
//! }
//! ```
//!
//! Frames: CEF accelerated OSR (a D3D11 shared texture on Windows, an IOSurface on macOS, a
//! dmabuf on Linux) is imported with cef-rs's
//! `SharedTextureHandle::import_texture` into iced's own wgpu device (captured from the shader
//! pipeline). iced must run on the matching API for that ([`backend`]: DX12 / Metal / Vulkan); if
//! the import
//! fails three times in a row the view re-creates its browser with software frames (`on_paint`
//! + `queue.write_texture`) and emits [`WebEvent::RenderModeChanged`].
//!
//! Presentation is pixel-exact: the widget snaps its bounds to the physical pixel grid, sizes
//! CEF so the frame has exactly that many pixels at iced's scale factor, and copies texels 1:1
//! (no filtering). During a resize the last frame keeps its size (no stretching) until CEF
//! delivers the new one. [`WebViewWidget::dimmed`] freezes the view as a dimmed snapshot (for
//! modal dialogs).

mod caption;
pub mod cdp;
mod devtools;
mod events;
mod gpu;
mod input;
mod owner;
mod placement;
mod runtime;
mod scheme;
pub mod testing;
mod view;
mod widget;

pub use caption::enable_snap_layout;
pub use devtools::{Awake, ConsoleEntry, DevTools};
pub use events::{ConsoleLevel, RenderMode, ViewId, WebEvent, WebViewEvent, events};
pub use owner::{set_window_owner, window_owner};
pub use placement::{Placement, place_window, window_placement};
pub use scheme::{
    Responder, SchemeHandler, SchemeRequest, SchemeResponse, register_custom_schemes,
    set_scheme_handler,
};
pub use runtime::{RuntimeSettings, WebRuntime, initialize, maybe_run_subprocess};
pub use view::{FrameDump, RenderStats, ViewOptions, WebView};
pub use widget::{DEFAULT_DIM, WebViewWidget, web_view};

/// CEF log severity, for [`RuntimeSettings::log_severity`].
pub use cef::LogSeverity;

/// The iced backend CEF's shared textures can be imported into on this OS: D3D11 textures on
/// a D3D12 device (Windows), IOSurfaces on Metal (macOS), dmabufs on Vulkan (Linux). When the
/// import fails anyway, a view falls back to software frames by itself.
pub fn backend() -> iced::Backend {
    use iced::backend::Api;
    iced::Backend::Hardware(if cfg!(windows) {
        Api::DirectX12
    } else if cfg!(target_os = "macos") {
        Api::Metal
    } else {
        Api::Vulkan
    })
}
