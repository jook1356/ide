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
- Build: **always build the app and the server together** (`ide.exe` starts `ide-server.exe` from
  its own folder): `cargo build --release -p ide-app -p ide-server` (alias `cargo app`; debug:
  `cargo app-debug`). `IDE_SERVER_BIN` points the app at another server binary.
- Check: `cargo clippy --workspace --all-targets`, `cargo test --workspace` with
  `IDE_TEST_TMP=<dir inside the workspace>` (server e2e tests spawn real `ide-server` daemons there).
- Run: `WGPU_BACKEND=dx12 <target>/release/ide.exe`. `IDE_DATA_DIR` overrides the data dir of the app
  *and* the server (each data dir gets its own daemon, pipe `\\.\pipe\ide-server-<user>-<hash>`).
  The app attaches to a running daemon or starts one (detached, no console window); closing the
  window keeps sessions running unless the user chooses otherwise (close dialog / Settings). The
  daemon exits after 10 min without clients and running sessions (`IDE_SERVER_IDLE_EXIT_SECS`, 0 =
  never). The settings page (rail -> 설정 / Settings) selects local or SSH; `IDE_REMOTE=ssh:<host>` overrides it.
- Test daemons: kill only your own, by PID (`<data>/ide-server.pid`), or end a script run with the
  app's stop-server action; never `taskkill /IM ide-server.exe` (it hits other data dirs' daemons).
- App end-to-end test (in-app, no OS input): `IDE_APP_SCRIPT=1 IDE_DATA_DIR=<scratch>
  IDE_APP_TEST_REPO=<scratch repo> IDE_APP_TEST_REPO2=<second scratch repo> IDE_APP_EVIDENCE=<dir>` (+ `IDE_APP_SCRIPT_CLAUDE=1` to also open a
  Claude first-run screen, `IDE_APP_SCRIPT_PAUSE=1` to hold key screens for an external capture, or a comma-separated list of
  pause-name prefixes such as `ko_rail` to hold only those);
  results in `<evidence>/script.log` (`IDE_APP_SCRIPT_PAUSE_MS` lengthens the pauses;
  `IDE_APP_SCRIPT_ONLY=p2` runs only startup + the phase 2 file/changes/search checks + server
  stop). The phase 2 part creates its fixtures in the test repo on first use (TS import, Korean
  file name, binary file) and registers the repo on the repos page; a multi-repo pass (in Korean)
  registers the second repo, makes both a configuration's default repos, creates a two-worktree
  session, switches Changes/Explore between repos, detaches/attaches, and checks delete cleanup. A base-branch pass (Korean) gives the test repo a local bare `origin` (`<data>/script-origin.git`), saves a fixed base and a `release/*` rule on the repos page, creates a session with 기준 브랜치 최신화 from the rule's pick, changes the session's base, and checks that an unreachable origin fails that repo with 최신화 and works without it; registering the plain folder `<data>/not-a-repo` shows Desk's git init question (English: cancelled, Korean: initialised). Other script sessions turn 최신화 off (their fixture commits are local only). Unset an inherited `IDE_APP_SCRIPT_CLAUDE` unless a Claude first-run check is wanted. The script
  runs against a real local daemon (started from the data dir, stopped at the end through the close
  dialog), in English with one Korean pass (`ko_*` screenshots). iced's `window::screenshot` (this
  iced rev) leaves `text_input` values blank; capture the real window to check those. Do not block
  the UI thread in script steps (slow `tasklist` calls once ended in a wgpu `Surface::configure`
  panic): run external probes on a helper thread.

## Conventions
- `ide-core` stays UI-free and unit-tested; put logic there, not in widgets. Only `ide-server` uses
  it: the app (`ide-app`) is a client and never spawns agents, runs git or touches project files;
  everything goes through `ide_client::Client`.
- Never enable the `cef` crate's default `sandbox` feature.
- No `unsafe` outside `ide-web`'s texture import and FFI glue; comment every `unsafe` block.
- Tests and test scripts must inject input inside the app (iced events, writing to the PTY). Never
  drive the real mouse/keyboard (SetCursorPos, SendInput, SendKeys, ...): it hits whatever window the
  user is using.
- Never send prompts to `claude`/`codex` from tests: they run on paid subscriptions. Opening the TUI,
  screenshots and exiting are fine. Opt-in real-CLI checks of the generated provider flags (scratch
  account dirs, nothing typed, no login): `IDE_REAL_CLI=1 cargo test -p ide-agents real_cli --
  --ignored` and `IDE_REAL_CLI=1 cargo test -p ide-server --test real_cli -- --ignored`.
- Provider-specific code (flags, env vars, files of an agent CLI) lives only in `ide-agents`; never
  `match` on a provider id elsewhere (see `docs/agent-providers.md`).
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
- Server/core errors reach the app as `ProtoError` (stable id + named args). A new id needs an
  `err-*` message in every locale and an arm in `ide-app/src/i18n.rs::proto_error` (unknown ids
  fall back to `err-unknown` with the id and args); for `ide-core` errors also add the variant to
  `ErrorMessage` and its args in `ide-server/src/service.rs::core_error`.
- Add a language: create `crates/ide-i18n/locales/<code>/ide.ftl` (copy `en`, translate every message,
  set `language-name` to the language's own name). `Lang`, the settings picker and OS-locale
  negotiation pick it up automatically.
