# Meninx

A native Rust desktop IDE for coding agents. Each session runs an agent CLI (Claude Code, Codex, or a
shell) in its own terminal and git worktree; web content (previews, mods, browser tabs) runs in an
embedded Chromium (CEF) only where needed.

- UI: [iced](https://github.com/iced-rs/iced) (master pinned, wgpu 30)
- Terminal: `portable-pty` (ConPTY) + `alacritty_terminal`
- Web: CEF 156 via [`cef`](https://crates.io/crates/cef) (tauri-apps/cef-rs), off-screen rendered into iced

See [docs/architecture.md](docs/architecture.md) for the design and MVP scope, and
[AGENTS.md](AGENTS.md) for build and contribution rules.
