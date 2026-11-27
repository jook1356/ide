# AGENTS.md

Rules for humans and coding agents working in this repo. Read docs/architecture.md first.

## Build environment (Windows)
- Rust stable (edition 2024), MSVC build tools (vcvars64), Ninja and **CMake 4.x**. The VS-bundled
  CMake 3.31 crashes (0xc0000409) on non-ASCII paths when building the CEF wrapper.
- CEF binaries: set `CEF_PATH` to an exported CEF 156 directory
  (`cargo run -p export-cef-dir -- --force <dir>` from tauri-apps/cef-rs) and put it on `PATH`
  so `libcef.dll` loads at run time.
- Keep `CARGO_TARGET_DIR` short (e.g. `D:/t/meninx`); deep target paths overflow the Windows path limit
  in the CEF wrapper build (`cl.exe C1083`).
- Force DX12 for CEF texture sharing: `WGPU_BACKEND=dx12`.
- Build: **always build the app and the server together** (`meninx.exe` starts `meninx-server.exe` from
  its own folder): `cargo build --release -p meninx-app -p meninx-server` (alias `cargo app`; debug:
  `cargo app-debug`). `MENINX_SERVER_BIN` points the app at another server binary.
- Check: `cargo clippy --workspace --all-targets`, `cargo test --workspace` with
  `MENINX_TEST_TMP=<scratch dir outside any git repository>` (e.g. `D:/t/test-tmp`; server e2e tests
  spawn real `meninx-server` daemons there). Not inside this repo: tests that need a plain, non-git
  folder (`register_unregister`, `files_e2e`, `repos_e2e`) fail when the folder is in a git work tree.
- Run: `WGPU_BACKEND=dx12 <target>/release/meninx.exe`. `MENINX_DATA_DIR` overrides the data dir of the app
  *and* the server (each data dir gets its own daemon, pipe `\\.\pipe\meninx-server-<user>-<hash>`).
  The app attaches to a running daemon or starts one (detached, no console window); closing the
  window keeps sessions running unless the user chooses otherwise (close dialog / Settings). The
  daemon exits after 10 min without clients and running sessions (`MENINX_SERVER_IDLE_EXIT_SECS`, 0 =
  never). Linux servers for SSH installs: `scripts/build-remote-servers.ps1 -AppDir <target>/release` (zig + cargo-zigbuild; `docs/remote-install.md`).  never). The settings page (rail -> 설정 / Settings) lists Local and SSH servers, the title bar picks the shown one (`docs/multi-server.md`); `MENINX_REMOTE=ssh:<host>` replaces the list.
- Test daemons: kill only your own, by PID (`<data>/meninx-server.pid`), or end a script run with the
  app's stop-server action; never `taskkill /IM meninx-server.exe` (it hits other data dirs' daemons).
- App end-to-end test (in-app, no OS input): `MENINX_APP_SCRIPT=1 MENINX_DATA_DIR=<scratch>
  MENINX_APP_TEST_REPO=<scratch repo> MENINX_APP_TEST_REPO2=<second scratch repo> MENINX_APP_EVIDENCE=<dir>` (+ `MENINX_APP_SCRIPT_CLAUDE=1` to also open a
  Claude first-run screen, `MENINX_APP_SCRIPT_PAUSE=1` to hold key screens for an external capture, or a comma-separated list of
  pause-name prefixes such as `ko_rail` to hold only those);
  results in `<evidence>/script.log` (`MENINX_APP_SCRIPT_PAUSE_MS` lengthens the pauses;
  `MENINX_APP_SCRIPT_ONLY=p2` runs only startup + the phase 2 file/changes/search checks + server
  stop; `MENINX_APP_SCRIPT_ONLY=pop` runs only startup + the detail window (pop-out) checks + server
  stop; `MENINX_APP_SCRIPT_ONLY=servers` runs only startup + the server list checks + server stop; with `MENINX_APP_TEST_SSH=<host>` (and `MENINX_SSH_CONFIG=<ssh config>` for its key; e.g. sshd in WSL on port 2222) it also installs the server there from `<app>/remote` and updates it (`docs/remote-install.md`); `MENINX_APP_SCRIPT_ONLY=newrepos` registers twelve scratch repos and checks the new-session
  dialog with many repos (search, chips, recent order, a short window) + server
  stop). The phase 2 part creates its fixtures in the test repo on first use (TS import, Korean
  file name, binary file) and registers the repo on the repos page; a multi-repo pass (in Korean)
  registers the second repo, makes both a configuration's default repos, creates a two-worktree
  session, switches Changes/Explore between repos, detaches/attaches, and checks delete cleanup. A base-branch pass (Korean) gives the test repo a local bare `origin` (`<data>/script-origin.git`), saves a fixed base and a `release/*` rule on the repos page, creates a session with 기준 브랜치 최신화 from the rule's pick, changes the session's base, and checks that an unreachable origin fails that repo with 최신화 and works without it; registering the plain folder `<data>/not-a-repo` shows Desk's git init question (English: cancelled, Korean: initialised). Other script sessions turn 최신화 off (their fixture commits are local only). The footer pass (commit / sync / PR footer) needs a fake `gh` and no real credentials: `MENINX_GH=crates/meninx-app/tests/fake-gh/gh.cmd` (Node; PRs kept in `state.json` + `calls.log` under `MENINX_FAKE_GH_STATE`, head / base commits read from and merges written to the bare origin `MENINX_FAKE_GH_ORIGIN=<data>/script-origin.git`), `MENINX_GITHUB_API=http://127.0.0.1:9`, `GIT_CONFIG_GLOBAL=<scratch config with user.name/email and credential.helper answering quit=1>`, `GIT_CONFIG_NOSYSTEM=1`, empty `GH_TOKEN`/`GITHUB_TOKEN`. Unset an inherited `MENINX_APP_SCRIPT_CLAUDE` unless a Claude first-run check is wanted. Inside a Meninx session the inherited `MENINX_SERVER_SOCKET` is the agent bridge's (the user's real daemon): `socket_name` ignores it while `MENINX_SESSION_ID` is set, so apps, daemons and server tests started there use their own `MENINX_DATA_DIR`'s daemon (before 2026-11-25 they reached the real one: test sessions and scratch repos landed in the user's lists, and a script's server stop ended the user's sessions). Never set `MENINX_SERVER_SOCKET` yourself in a test inside a session. The script
  runs against a real local daemon (started from the data dir, stopped at the end through the close
  dialog), in English with one Korean pass (`ko_*` screenshots). iced's `window::screenshot` (this
  iced rev) leaves `text_input` values blank; capture the real window to check those. Do not block
  the UI thread in script steps (slow `tasklist` calls once ended in a wgpu `Surface::configure`
  panic): run external probes on a helper thread.

## Conventions
- `meninx-core` stays UI-free and unit-tested; put logic there, not in widgets. Only `meninx-server` uses
  it: the app (`meninx-app`) is a client and never spawns agents, runs git or touches project files;
  everything goes through `meninx_client::Client`. One exception: the footer's 빌드 후 재시작
  (`self_update.rs`, `docs/self-update.md`) starts the update script that pulls and rebuilds Meninx itself.
- Never enable the `cef` crate's default `sandbox` feature.
- No `unsafe` outside `meninx-web`'s texture import and FFI glue; comment every `unsafe` block.
- Tests and test scripts must inject input inside the app (iced events, writing to the PTY). Never
  drive the real mouse/keyboard (SetCursorPos, SendInput, SendKeys, ...): it hits whatever window the
  user is using.
- Never send prompts to `claude`/`codex` from tests: they run on paid subscriptions. Opening the TUI,
  screenshots and exiting are fine. Opt-in real-CLI checks of the generated provider flags (scratch
  account dirs, nothing typed, no login): `MENINX_REAL_CLI=1 cargo test -p meninx-agents real_cli --
  --ignored` and `MENINX_REAL_CLI=1 cargo test -p meninx-server --test real_cli -- --ignored`.
- OS-specific code: keep `docs/platforms.md` current. The maintainer has no Mac, so every change
  that adds or touches macOS / Linux code (or guesses what another OS needs) adds or updates an
  item in its to-do lists (file, assumption, how to check) and a line in its log.
- Provider-specific code (flags, env vars, files of an agent CLI) lives only in `meninx-agents`; never
  `match` on a provider id elsewhere (see `docs/agent-providers.md`).
- Commit messages: imperative, one topic per commit.

## UI text and languages
- No hard-coded user-visible strings in UI code: labels, buttons, tooltips, placeholders, dialog
  text, status words and error messages go through `meninx_i18n::tr::*` (or `crate::i18n` helpers in
  `meninx-app`). Not translated: the window title / brand `meninx`, icon glyphs, user data (session and
  profile names, paths), terminal and web content, log lines and `Display` of errors.
- Add a string: add the message to `crates/meninx-i18n/locales/en/meninx.ftl` **and every other locale**
  with the same variables, then call the generated `tr::<snake_case_id>(...)`. The build lists missing
  or extra keys and variable mismatches; `cargo test -p meninx-i18n` flags messages no code uses (remove
  them from every locale).
- Variables: plural counts use a selector (`{ $count -> [one] ... *[other] ... }`) or `NUMBER($n)` in
  English and become `i64`; everything else is `&str`. No message references (`{ other-message }`).
- Korean: short nouns for buttons/labels (`새 세션`, `취소`, `저장`), polite 해요체 for messages
  (`세션이 아직 없어요.`). Never put a particle right after a variable (`{ $name }을(를)`): rephrase
  (`“{ $name }” 프로필을 ...`) so the particle follows a fixed word.
- Server/core errors reach the app as `ProtoError` (stable id + named args). A new id needs an
  `err-*` message in every locale and an arm in `meninx-app/src/i18n.rs::proto_error` (unknown ids
  fall back to `err-unknown` with the id and args); for `meninx-core` errors also add the variant to
  `ErrorMessage` and its args in `meninx-server/src/service.rs::core_error`.
- Add a language: create `crates/meninx-i18n/locales/<code>/meninx.ftl` (copy `en`, translate every message,
  set `language-name` to the language's own name). `Lang`, the settings picker and OS-locale
  negotiation pick it up automatically.
