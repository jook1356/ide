//! CEF runtime and an iced widget that shows one off-screen-rendered browser.
//!
//! ```ignore
//! fn main() -> anyhow::Result<()> {
//!     if let Some(code) = ide_web::maybe_run_subprocess() {
//!         std::process::exit(code); // CEF helper process
//!     }
//!     let runtime = ide_web::initialize(ide_web::RuntimeSettings::new(data_dir))?;
//!     // ... create views with WebView::new(&runtime, url, size, scale) inside the iced app,
//!     //     show them with WebViewWidget::new(&view), listen with ide_web::events() ...
//!     iced::application(..).backend(ide_web::backend()).run()?;
//!     runtime.shutdown();
//!     Ok(())
//! }
//! ```
//!
//! Frames: CEF accelerated OSR (D3D11 shared texture) is imported with cef-rs's
//! `SharedTextureHandle::import_texture` into iced's own wgpu device (captured from the shader
//! pipeline). iced must run on DX12 for that ([`backend`] or `WGPU_BACKEND=dx12`); if the import
//! fails three times in a row the view re-creates its browser with software frames (`on_paint`
//! + `queue.write_texture`) and emits [`WebEvent::RenderModeChanged`].

mod events;
mod gpu;
mod input;
mod runtime;
pub mod testing;
mod view;
mod widget;

pub use events::{ConsoleLevel, RenderMode, ViewId, WebEvent, WebViewEvent, events};
pub use runtime::{RuntimeSettings, WebRuntime, initialize, maybe_run_subprocess};
pub use view::{RenderStats, ViewOptions, WebView};
pub use widget::{WebViewWidget, web_view};

/// CEF log severity, for [`RuntimeSettings::log_severity`].
pub use cef::LogSeverity;

/// The iced backend the GPU path needs (D3D11 shared textures are opened on a D3D12 device).
pub fn backend() -> iced::Backend {
    iced::Backend::Hardware(iced::backend::Api::DirectX12)
}
