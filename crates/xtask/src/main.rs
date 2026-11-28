//! `cargo app` / `cargo app-debug` (aliases in `.cargo/config.toml`): build the app and its
//! server, then the static Linux servers the app installs on SSH hosts
//! (`docs/remote-install.md`) into `<target>/<profile>/remote/linux-<arch>/meninx-server`.
//!
//! The Linux builds need zig (on `PATH`, or `<toolchain>/zig/zig.exe` beside `CARGO_HOME`) and
//! `cargo-zigbuild`; without them, or when they fail, the app build still counts (a warning
//! says what is left out). `--no-remote` skips them.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// (Rust target, folder under `remote/`).
const REMOTE: [(&str, &str); 2] = [
    ("x86_64-unknown-linux-musl", "linux-x86_64"),
    ("aarch64-unknown-linux-musl", "linux-aarch64"),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("app") => app(args.iter().any(|a| a == "--release"), args.iter().any(|a| a == "--no-remote")),
        _ => {
            eprintln!("usage: cargo xtask app [--release] [--no-remote]  (aliases: cargo app, cargo app-debug)");
            ExitCode::FAILURE
        }
    }
}

fn app(release: bool, no_remote: bool) -> ExitCode {
    let mut build = cargo();
    build.args(["build", "-p", "meninx-app", "-p", "meninx-server"]);
    if release {
        build.arg("--release");
    }
    if !run(build) {
        return ExitCode::FAILURE;
    }
    if no_remote {
        return ExitCode::SUCCESS;
    }
    let out = target_dir().join(if release { "release" } else { "debug" }).join("remote");
    if let Err(why) = remote_servers(&out) {
        eprintln!("warning: Linux servers for SSH installs not built: {why} (docs/remote-install.md)");
    }
    ExitCode::SUCCESS
}

/// Build and copy the Linux servers into `out/<folder>/meninx-server`.
fn remote_servers(out: &Path) -> Result<(), String> {
    let zig = find_zig().ok_or("zig not found (PATH, or <toolchain>/zig beside CARGO_HOME)")?;
    let path = std::env::join_paths(
        std::iter::once(zig.parent().unwrap_or(Path::new(".")).to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
    )
    .map_err(|e| e.to_string())?;
    let mut failed = Vec::new();
    for (triple, folder) in REMOTE {
        let mut c = cargo();
        c.env("PATH", &path)
            .args(["zigbuild", "--profile", "remote", "-p", "meninx-server", "--target", triple]);
        if !run(c) {
            failed.push(triple);
            continue;
        }
        let from = target_dir().join(triple).join("remote").join("meninx-server");
        let dir = out.join(folder);
        std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::copy(&from, dir.join("meninx-server")))
            .map_err(|e| format!("copying {}: {e}", from.display()))?;
        println!("Linux server for SSH installs: {}", dir.join("meninx-server").display());
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("cargo zigbuild failed for {} (is cargo-zigbuild installed?)", failed.join(", ")))
    }
}

fn find_zig() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "zig.exe" } else { "zig" };
    let on_path = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|d| d.join(exe))
        .find(|p| p.is_file());
    on_path.or_else(|| {
        let home = PathBuf::from(std::env::var_os("CARGO_HOME")?);
        let p = home.parent()?.join("zig").join(exe);
        p.is_file().then_some(p)
    })
}

/// The cargo that runs this task (`CARGO`, set by `cargo run`).
fn cargo() -> Command {
    let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    c.current_dir(workspace());
    c
}

fn run(mut c: Command) -> bool {
    match c.status() {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!("error: {e}");
            false
        }
    }
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => workspace().join("target"),
    }
}
