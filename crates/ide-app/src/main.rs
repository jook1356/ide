//! `ide`: sessions of coding agents (Claude Code / Codex / shell), each in its own git worktree
//! and terminal, with a per-session web detail area.
//!
//! The app is a client of `ide-server` (see `docs/server-architecture.md`): it starts
//! `ide-server --stdio` from its own folder (or `IDE_SERVER_BIN`), which connects to the per-user
//! daemon (starting it if needed). Sessions keep running in the daemon when the app closes.
//!
//! Run: `WGPU_BACKEND=dx12 ide` (CEF's `libcef.dll` must be on `PATH`). The data dir is
//! `IDE_DATA_DIR` or the platform data dir (also passed to the server). Settings -> Server
//! chooses local or SSH (`ssh <host> ide-server --stdio`); `IDE_REMOTE=ssh:<host>` overrides it.
//! `IDE_APP_SCRIPT=1` runs the in-app test script (see `script.rs`).

// No console window for release builds (logs still reach redirected stdout/stderr).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod branch_badge;
mod browser;
mod changes_actions;
mod changes_view;
mod composer;
mod detail;
mod detail_rail;
mod detail_view;
mod detail_window;
mod explore_actions;
mod footer;
mod footer_view;
mod frame;
mod history;
mod i18n;
mod icons;
mod agent_bridge;
mod item_menu;
mod uterm;
mod script;
mod scripts_view;
mod session_menu;
mod session_sort;
mod profiles_view;
mod skills_ui;
mod style;
mod ui_state;
mod view;

use std::path::PathBuf;

use anyhow::Context as _;

/// Environment variable overriding the data dir (shared with `ide-server`).
const DATA_DIR_ENV: &str = "IDE_DATA_DIR";
/// `ssh:<host>`: use a remote server over SSH instead of the local one.
const REMOTE_ENV: &str = "IDE_REMOTE";

/// The app's own data dir (`ui.json`, CEF profile): `IDE_DATA_DIR` or the platform data dir.
fn data_dir() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let dirs = directories::ProjectDirs::from("dev", "ide", "ide")
        .context("cannot determine the app data directory (set IDE_DATA_DIR)")?;
    Ok(dirs.data_dir().to_path_buf())
}

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

    let data = data_dir()?;
    std::fs::create_dir_all(&data).with_context(|| format!("creating {}", data.display()))?;
    tracing::info!(data = %data.display(), "starting");
    // The server setting lives in ui.json (Settings -> Server); `IDE_REMOTE` overrides it.
    let remote = std::env::var(REMOTE_ENV).ok().filter(|v| !v.is_empty());
    let runtime = ide_web::initialize(ide_web::RuntimeSettings::new(data.join("web")))?;

    let rt = runtime.clone();
    let boot_data = data.clone();
    // A daemon (several windows): the app window is opened by `App::boot`, the detail area's
    // own window by `detail_window.rs`. Closing the app window ends the daemon (`App::exit_app`).
    let result = iced::daemon(
        move || app::App::boot(boot_data.clone(), rt.clone(), remote.clone()),
        app::App::update,
        app::App::view_window,
    )
    .title(app::App::title)
    .theme(app::App::theme)
    .subscription(app::App::subscription)
    .settings(iced::Settings {
        backend: ide_web::backend(),
        ..iced::Settings::default()
    })
    .run();
    runtime.shutdown();
    result.context("iced")?;
    Ok(())
}
