# AGENTS.md

Rules for humans and coding agents working in this repo. Read docs/architecture.md first.

## Build environment (Windows)
- Rust stable (edition 2024), MSVC build tools (vcvars64), Ninja and **CMake 4.x**. The VS-bundled
  CMake 3.31 crashes (0xc0000409) on non-ASCII paths when building the CEF wrapper.
- CEF binaries: set `CEF_PATH` to an exported CEF 156 directory
  (`cargo run -p export-cef-dir -- --force <dir>` from tauri-apps/cef-rs) and put it on `PATH`
  so `libcef.dll` loads at run time.
- Keep `CARGO_TARGET_DIR` short (e.g. `C:/t/ide`); deep target paths overflow the Windows path limit
  in the CEF wrapper build (`cl.exe C1083`).
- Force DX12 for CEF texture sharing: `WGPU_BACKEND=dx12`.
- Build/check: `cargo build --release -p ide-app`, `cargo test -p ide-core`, `cargo clippy --workspace`.
- Run: `WGPU_BACKEND=dx12 <target>/release/ide.exe` (`IDE_DATA_DIR` overrides the data dir).
- App end-to-end test (in-app, no OS input): `IDE_APP_SCRIPT=1 IDE_DATA_DIR=<scratch>
  IDE_APP_TEST_REPO=<scratch repo> IDE_APP_EVIDENCE=<dir>` (+ `IDE_APP_SCRIPT_CLAUDE=1` to also open a
  Claude first-run screen, `IDE_APP_SCRIPT_PAUSE=1` to hold key screens for an external capture);
  results in `<evidence>/script.log`. iced's `window::screenshot` (this iced rev) leaves
  `text_input` values blank; capture the real window to check those.

## Conventions
- `ide-core` stays UI-free and unit-tested; put logic there, not in widgets.
- Never enable the `cef` crate's default `sandbox` feature.
- No `unsafe` outside `ide-web`'s texture import and FFI glue; comment every `unsafe` block.
- Tests and test scripts must inject input inside the app (iced events, writing to the PTY). Never
  drive the real mouse/keyboard (SetCursorPos, SendInput, SendKeys, ...): it hits whatever window the
  user is using.
- Never send prompts to `claude`/`codex` from tests: they run on paid subscriptions. Opening the TUI,
  screenshots and exiting are fine.
- Commit messages: imperative, one topic per commit.
