//! `ide`: sessions of coding agents (Claude Code / Codex / shell), each in its own git worktree
//! and terminal, with a per-session web detail area.
//!
//! Run: `WGPU_BACKEND=dx12 ide` (CEF's `libcef.dll` must be on `PATH`). The data dir is
//! `IDE_DATA_DIR` or the platform data dir. `IDE_APP_SCRIPT=1` runs the in-app test script
//! (see `script.rs`).

// No console window for release builds (logs still reach redirected stdout/stderr).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod script;
mod style;
mod ui_state;
mod view;

use anyhow::Context as _;

fn main() -> anyhow::Result<()> {
    // CEF helper processes re-run this executable: they must exit here.
    if let Some(code) = ide_web::maybe_run_subprocess() {
        std::process::exit(code);
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn,naga=warn".into()),
        )
        .init();

    let paths = ide_core::Paths::from_env()?;
    std::fs::create_dir_all(paths.root())
        .with_context(|| format!("creating {}", paths.root().display()))?;
    tracing::info!(data = %paths.root().display(), "starting");
    let runtime = ide_web::initialize(ide_web::RuntimeSettings::new(paths.root().join("web")))?;

    let rt = runtime.clone();
    let boot_paths = paths.clone();
    let result = iced::application(
        move || app::App::boot(boot_paths.clone(), rt.clone()),
        app::App::update,
        app::App::view,
    )
    .title(app::App::title)
    .theme(app::App::theme)
    .subscription(app::App::subscription)
    .window_size((1440.0, 880.0))
    .exit_on_close_request(false)
    .backend(ide_web::backend())
    .run();
    runtime.shutdown();
    result.context("iced")?;
    Ok(())
}
