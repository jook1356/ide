# Platforms

Windows is the platform that is built, run and tested. macOS and Linux code paths exist; this
page lists what each piece does there and what is still open.

| Piece | Windows | macOS | Linux |
|---|---|---|---|
| Server ↔ app socket | named pipe | Unix socket | Unix socket |
| Daemon start (detached) | `CREATE_NO_WINDOW` | own process group | own process group |
| Shells (`uterm.shells`) | pwsh, powershell, cmd, Git Bash, WSL | `$SHELL`, `/etc/shells` | same |
| Ports (`uterm.ports`) | `listeners` + `sysinfo` | same | same |
| Browse roots | drive letters + home | `/` + home | same |
| Reveal / open URL | `explorer.exe` | `open -R` / `open` | `xdg-open` |
| iced backend (`ide_web::backend`) | DX12 | Metal | Vulkan |
| CEF frames (GPU) | D3D11 shared texture | IOSurface | dmabuf |
| CEF frames (fallback) | software `on_paint` | same | same |
| Title bar (`frame.rs`) | frameless, drawn caption buttons, snap layout | native traffic lights over a transparent, full-size title bar; the app leaves 78 px for them and draws no caption buttons or grips | frameless, drawn caption buttons, grips |
| Detail window pin | owned window (`set_window_owner`) | always on top | always on top |
| Web key codes (`native_key_code`) | scan code (`MapVirtualKeyW`) | `kVK_*` from the physical key | X11 keycode (evdev + 8) |
| Shortcuts in web views | Ctrl | Ctrl or Cmd | Ctrl |
| Windowing (iced features) | — | — | X11 + Wayland (`wayland-dlopen`) |

Checked on this machine: `cargo check --target aarch64-apple-darwin` and
`--target x86_64-unknown-linux-gnu` for `ide-proto`, `ide-core`, `ide-agents`, `ide-i18n`,
`ide-term`, `ide-views`, `ide-client`. `ide-server` (`ring`, a C build) and `ide-web` / `ide-app`
(the CEF wrapper, CMake + CEF binaries of the target) need that OS's toolchain.

## Linux: to do

- [ ] **CEF.** Should work with `multi_threaded_message_loop` and `libcef.so` next to the
  executable (`CEF_PATH` of the Linux build); never run. The dmabuf import needs the Vulkan device
  feature `VULKAN_EXTERNAL_MEMORY_DMA_BUF`; without it a view falls back to software frames.
- [ ] **Packaging.** The binaries with `libcef.so` and its resources in one folder.
- [ ] **Terminal font.** `ide-term` defaults to `Consolas` (`config.rs`); pick a Linux default
  (`DejaVu Sans Mono` / `monospace`).
- [ ] **Session menu "작업 폴더 열기".** `session_menu::open_folder` runs `xdg-open <folder>`.
  Check: right-click a session, open its folder in the desktop's file manager.
- [ ] **Widget scheme (`ide-widget://`, `ide-web/src/scheme.rs`).** Not OS-specific code, but
  custom schemes must be registered in every CEF process: `register_custom_schemes` runs before
  `maybe_run_subprocess`, which assumes the helper processes run the app's own executable (as on
  Windows). Check: `IDE_APP_SCRIPT_ONLY=widget` passes (`docs/widgets.md`).
- [ ] **Preview scheme (`ide-preview://`, `previews_ui.rs`).** Same as the widget scheme: one more
  name in the same `register_custom_schemes` list. Check: `IDE_APP_SCRIPT_ONLY=preview` passes
  (`docs/previews.md`).
- [ ] **Self-update (빌드 후 재시작).** Windows only (`self_update::available` is false elsewhere;
  `scripts/ide-update.ps1`). Assumption: a shell script doing the same steps works on Linux / macOS
  (a running binary can be replaced there directly). Check: port the script, show the footer button.
- [ ] **Taskbar attention for agent notices** (`notice_view.rs` `attention`):
  `window::request_user_attention(.., Informational)` while the window is not focused. Assumption:
  winit maps it to the urgency hint on X11 and to `xdg_activation` on Wayland (some compositors
  ignore it). Check: unfocus the app, let an agent finish, and see whether the taskbar entry is
  marked.

- [ ] **Restart an incompatible server.** `old_server::kill_tree` ends the daemon's process
  group with `kill -TERM -<pid>` (assumes `spawn_daemon` made it a group leader). Check: run an
  old daemon, start a newer-protocol app, press 서버 재시작 in the connection status.
- [ ] **Codex resume without the hook** (`ide-agents/src/codex.rs` `started_rollout`): a
  rollout's age is its creation time, else its last write (file systems without a birth
  time). Assumption: Codex's rollouts and `session_meta.cwd` look the same as on Windows.
  Check: start a Codex session, send one prompt, restart the server, open the session: it
  shows `codex resume <uuid>` with the conversation.

## macOS: to do

The maintainer has no Mac, so nothing here has been built or run. This list is the hand-off for
whoever gets one: keep it current. Any change that adds or touches macOS-specific code (or a
`cfg(windows)` branch whose other side is a guess) adds or updates an item here, with the file,
what is assumed and how to check it. Tick an item only after checking it on a Mac.

### 1. Blockers (the app does not start)

- [ ] **CEF framework loading.** `cef::library_loader::LibraryLoader::new(&current_exe, helper)`
  and `.load()` before any CEF call, in the main process (`helper = false`, path
  `../Frameworks/...`) and in every helper (`helper = true`, path `../../..`). Keep the loader
  alive until after `cef::shutdown()` (its `Drop` unloads the framework). Where:
  `ide-web/src/runtime.rs` (`maybe_run_subprocess`, `initialize`).
- [ ] **App bundle layout.** CEF needs `ide.app/Contents/{MacOS/ide, Frameworks/Chromium Embedded
  Framework.framework, Frameworks/ide Helper.app, ide Helper (GPU).app, (Renderer), (Plugin),
  (Alerts)}` plus `Info.plist`s. cef-rs's `bundle-cef-app` (and `build_util::mac`) builds that
  layout; helpers are separate executables, so the helper path of `maybe_run_subprocess` needs its
  own small `main` (or the same binary copied and renamed per helper). `ide-server` must sit next
  to `ide` (`Contents/MacOS/`): the app starts it from its own folder
  (`ide-client/src/transport.rs`).
- [ ] **`NSApplication` and `CefAppProtocol`.** CEF requires the shared `NSApplication` to answer
  `isHandlingSendEvent` / `setHandlingSendEvent:` (cef-rs `application_mac.rs` defines the
  protocol). winit creates its own `NSApplication` subclass in `EventLoop::new`, so the methods
  have to be added to winit's class at run time (objc2 `class_addMethod`) before CEF starts, or
  winit has to use a CEF-aware subclass. Check: no "NSApp does not conform" assert; menus and
  key events still reach iced.
- [ ] **Message loop.** `multi_threaded_message_loop` is not supported on macOS
  (`ide-web/src/runtime.rs` sets it unconditionally). Use `external_message_pump = true` and call
  `cef::do_message_loop_work()` on the main thread, scheduled from
  `BrowserProcessHandler::on_schedule_message_pump_work`. Calling it from iced's `update()` hung
  on Windows (spike 001b, comment at the top of `runtime.rs`): find a main-thread hook outside
  `update()` (a `CFRunLoopTimer`, or a winit user event).
- [ ] **Running without web views.** Optional but useful first: when `ide_web::initialize` fails,
  start the app without browser tabs instead of exiting (`ide-app/src/main.rs`), so everything
  else can be tested on a Mac before CEF works.

### 2. Needs checking on a Mac (code exists, untested)

- [ ] **Browser control (`ide-web/src/devtools.rs`, `docs/browser-control.md`).** Assumption:
  CEF's DevTools messages, `post_task(UI)` and `Input.dispatch*Event` behave as on Windows, and
  `Page.captureScreenshot` of a hidden OSR view works after `was_hidden(false)`. Check:
  `IDE_APP_SCRIPT_ONLY=browser` (all `browser call` lines pass; also on Linux).

- [ ] **Dock attention for agent notices** (`ide-app/src/notice_view.rs` `attention`):
  `window::request_user_attention(.., Informational)`. Assumption: winit makes it one Dock icon
  bounce (`NSInformationalRequest`). Check: switch to another app, let an agent finish, and watch
  the Dock icon.

- [ ] **Widget scheme (`ide-widget://`).** As on Linux; on macOS CEF's helper processes are
  separate app bundles (`bundle-cef-app`): they must call `ide_web::register_custom_schemes` with
  the same list too, or widget pages do not get their own origin. Check: `IDE_APP_SCRIPT_ONLY=widget`.
- [ ] **Preview scheme (`ide-preview://`).** As the widget scheme: the helper bundles need
  `previews_ui::SCHEME` in their list too. Check: `IDE_APP_SCRIPT_ONLY=preview`.
- [ ] **Login-shell `PATH`.** An app started from Finder / Dock gets
  `/usr/bin:/bin:/usr/sbin:/sbin`, so `claude`, `codex`, `gh`, Homebrew `git` and `node` are not
  found. The server should read the login shell's environment once (`$SHELL -lc env` or similar)
  and use it for agents, git and `gh` (`ide-core/src/launch.rs` `LaunchEnv`;
  `ide-server/src/pr.rs` `resolve_gh` already looks in `/opt/homebrew/bin`).
- [ ] **Unix socket path length.** `sun_path` is 104 bytes on macOS. The socket is
  `<data>/ide-server.sock` (`ide-server/src/user.rs` `socket_name`); the default data dir fits, a
  deep `IDE_DATA_DIR` may not. If it does not fit, fall back to `$TMPDIR/ide-<hash>.sock`.
- [ ] **Title bar** (`ide-app/src/frame.rs`): transparent, full-size title bar with the native
  traffic lights. Check that the 78 px left gap (`TRAFFIC_LIGHTS_W`) clears them; that they sit
  vertically centred in the 36 px bar (`BAR_H`; the native bar is about 28 px, so their position
  may need moving); that dragging the bar moves the window (`window::drag`); and what a
  double-click does (macOS has a setting: zoom or minimise; the app always toggles maximise).
  Resize grips are off on macOS (the native frame resizes).
- [ ] **Full screen.** The green button enters native full screen: check the layout and that the
  traffic-light gap does not look odd there.
- [ ] **Detail window pin** (`ide-app/src/detail_window.rs` `apply_pin`): `AlwaysOnTop` instead
  of an owned window, so it floats over every app, not just ide. A closer match is an `NSWindow`
  child window (`addChildWindow:ordered:`) through `raw_window_handle`.
- [ ] **GPU frames** (`ide-web/src/gpu.rs`, `lib.rs` `backend`): IOSurface import on Metal. Check
  that pages render, resizing is clean, and `IDE_WEB_FORCE_CPU=1` (software frames) works as the
  fallback.
- [ ] **Web view keys** (`ide-web/src/input.rs`): `native_key_code` uses the `kVK_*` table
  (`mac_key_code`; only its values are unit-tested). Check `KeyboardEvent.code` in a page, Cmd+C /
  V / A / Z in inputs (Cmd is treated like Ctrl: no CHAR event), and whether CEF wants
  `EF_COMMAND` shortcuts with `is_system_key = 0`.
- [ ] **Korean IME.** Composition in the iced text inputs, the code editor, the terminal and the
  web views (CEF's composition path is Windows-tested only).
- [ ] **Terminal keys** (`ide-term/src/keys.rs`): Option as Meta (`ESC` prefix) versus Option for
  typing characters; Cmd+C / Cmd+V copy and paste; Cmd+K.
- [ ] **App shortcuts.** iced's `Modifiers::command()` is Cmd on macOS (`app.rs`, `editor.rs`,
  `code.rs`); `ide-views/src/tree.rs` checks `control()` directly: check that this is meant.
- [ ] **Fonts.** The terminal defaults to `Consolas` (`ide-term/src/config.rs`): use `Menlo` /
  `SF Mono` on macOS. The symbols chosen for Segoe UI Symbol (`ide-app/src/view.rs`) need a check
  with Apple's fallback fonts; Korean falls back to Apple SD Gothic Neo.
- [ ] **Shells** (`ide-server/src/service.rs` `shells`): `$SHELL` (zsh by default) and
  `/etc/shells`, run with `-lc`. Check the names shown and that a login shell starts.
- [ ] **Ports** (`ide-server/src/ports.rs`): `listeners` uses libproc on macOS; check that a dev
  server started in a terminal shows up under 실행 중인 서버.
- [ ] **Reveal / open** (`ide-app/src/item_menu.rs`): `open -R`, `open <url>`. VS Code's `code` is
  on `PATH` only after "Install 'code' command in PATH": fall back to
  `open -a "Visual Studio Code" <path>`.
- [ ] **Daemon detach** (`ide-server/src/daemon.rs`): own process group. Check that the daemon
  survives quitting the app and that the 10 min idle exit works.
- [ ] **Retina.** Scale factor 2: pixel-exact web frames, 1 px borders, editor scrollbar.
- [ ] **Attachments in the input box** (`ide-app/src/composer.rs`): file drops (winit
  `FileDropped`), files copied in Finder (`clipboard::read_files`) and clipboard images
  (`read_image`, arboard) as chips; Cmd+V is the paste key there. Check that Finder copies
  arrive as paths, that a screenshot (Cmd+Ctrl+Shift+4) pastes as an image, and that Claude
  Code turns the pasted image path into `[Image #n]`.
- [ ] **App end-to-end script** (`IDE_APP_SCRIPT=1`, see `AGENTS.md`): run it on a Mac. The footer
  pass needs a shell-script twin of `crates/ide-app/tests/fake-gh/gh.cmd`.
- [ ] **Explore cut / copy / paste with the OS clipboard.** `explore_actions.rs` writes the
  entries as `clipboard::Content::Files` and pastes `read_files()` results (files copied in
  Finder). Assumption: iced's winit clipboard supports file lists on macOS like on Windows.
  Check: copy a file in Explore, paste in Finder; copy in Finder, paste in Explore (Cmd+C / Cmd+V).
- [ ] **Explore delete to the Trash.** `fsops::trash` uses the `trash` crate (Finder's Trash on
  macOS). Check: delete a file in Explore, find it in the Trash with "Put Back".
- [ ] **Explore "Finder에서 보기" label and reveal.** `tree.rs::reveal_os_label` and
  `item_menu::reveal_in_file_manager` (`open -R`). Check the label and that Finder selects the entry.
- [ ] **Session menu "작업 폴더 열기".** `session_menu::open_folder` runs `open <folder>`.
  Assumption: `open` on a directory opens it in Finder. Check: right-click a session, open its folder.

### 3. Shipping

- [ ] **Signing and notarisation.** Sign the app, the framework and every helper (hardened
  runtime; the renderer helper needs CEF's JIT entitlements), notarise, staple. Without it
  Gatekeeper blocks a downloaded build.
- [ ] **Architectures.** A universal binary or two builds (`aarch64-apple-darwin`,
  `x86_64-apple-darwin`); CEF ships per-architecture frameworks.
- [ ] **Data dir.** `directories::ProjectDirs::from("dev", "ide", "ide")` →
  `~/Library/Application Support/dev.ide.ide`: check that `ui.json`, the CEF profile and the
  daemon files land there.
- [ ] **CI.** A macOS runner doing `cargo check -p ide-server -p ide-app` would catch compile
  errors in `cfg(target_os = "macos")` code that the Windows machine cannot build.

## Log

Newest first; one line per change that touched another OS's code, so the Mac pass knows what to
look at.

- 2026-11-24: Codex resumes after a server restart (`codex.rs` rollout lookup by creation /
  write time and cwd); checked on Windows only.
- 2026-11-24: previews: CEF custom scheme `ide-preview://` next to `ide-widget://` (same list in
  `main`); checked on Windows only.
- 2026-11-24: browser control by agents (`ide-web` `devtools.rs`: CDP through `send_dev_tools_message`, `post_task` to CEF's UI thread; `browser_agent.rs`): no OS-specific code, checked on Windows only.
- 2026-11-23: `old_server.rs` ends an incompatible daemon by pid; `kill -TERM -<pid>` off Windows (unbuilt).
- 2026-11-23: widgets: CEF custom scheme `ide-widget://` (`scheme.rs`), registered in every
  process; checked on Windows only.
- 2026-11-23: notification centre flashes the taskbar / Dock while unfocused
  (`notice_view.rs` `attention`, iced `request_user_attention`); unchecked on macOS / Linux.
- 2026-11-23: footer self-update (`self_update.rs`) is Windows-only; off on Linux / macOS.
- 2026-11-23: session right-click menu opens the working folder with `open` (macOS) /
  `xdg-open` (Linux) (`session_menu.rs`); unbuilt on macOS / Linux.
- 2026-11-23: Explore cut / copy / paste via the OS file clipboard, delete to the Trash
  (`trash` crate), reveal label per OS (`explore_actions.rs`, `fsops.rs`, `tree.rs`); unbuilt on
  macOS / Linux.
- 2026-11-23: input-box attachments: Cmd+V pastes files / images as chips (`composer.rs`);
  unbuilt on macOS.
- 2026-11-22: `frame.rs` macOS title bar (traffic lights, no grips); `detail_window.rs` pin =
  always on top off Windows; `ide-web` `backend()` per OS, `native_key_code` per OS, Cmd as Ctrl
  in web views; iced `x11` / `wayland` features. All unbuilt on macOS / Linux.
