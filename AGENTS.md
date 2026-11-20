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
  results in `<evidence>/script.log` (`IDE_APP_SCRIPT_PAUSE_MS` lengthens the pauses). The script
  runs in English and has one Korean pass (`ko_*` screenshots). iced's `window::screenshot` (this iced rev) leaves
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

## UI text and languages
- No hard-coded user-visible strings in UI code: labels, buttons, tooltips, placeholders, dialog
  text, status words and error messages go through `ide_i18n::tr::*` (or `crate::i18n` helpers in
  `ide-app`). Not translated: the window title / brand `ide`, icon glyphs, user data (session and
  profile names, paths), terminal and web content, log lines and `Display` of errors.
- Add a string: add the message to `crates/ide-i18n/locales/en/ide.ftl` **and every other locale**
  with the same variables, then call the generated `tr::<snake_case_id>(...)`. The build lists missing
  or extra keys and variable mismatches; `cargo test -p ide-i18n` flags messages no code uses (remove
  them from every locale).
- Variables: plural counts use a selector (`{ $count -> [one] ... *[other] ... }`) or `NUMBER($n)` in
  English and become `i64`; everything else is `&str`. No message references (`{ other-message }`).
- Korean: short nouns for buttons/labels (`새 세션`, `취소`, `저장`), polite 해요체 for messages
  (`세션이 아직 없어요.`). Never put a particle right after a variable (`{ $name }을(를)`): rephrase
  (`“{ $name }” 프로필을 ...`) so the particle follows a fixed word.
- `ide-core` errors: add the variant to `ErrorMessage` in `ide-core/src/error.rs`, an `err-*` message
  to every locale, and the mapping in `ide-app/src/i18n.rs` (the match is exhaustive).
- Add a language: create `crates/ide-i18n/locales/<code>/ide.ftl` (copy `en`, translate every message,
  set `language-name` to the language's own name). `Lang`, the settings picker and OS-locale
  negotiation pick it up automatically.
