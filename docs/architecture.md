# Architecture and MVP

## Why this shape
Decided by spikes (Slint / iced / egui) recorded outside this repo. iced won on the deciding test:
a terminal hosting Claude Code / Codex TUIs (lowest draw cost, CPU and memory; Korean IME via the
public `Shell::request_input_method` API; verified with the real Microsoft Korean IME). CEF is
composited as a GPU texture (cef-rs `accelerated_osr`, wgpu 30 = iced master's wgpu).

## Crates
| Crate | Role | Depends on UI? |
|---|---|---|
| `ide-core` | Accounts, configurations, sessions, git worktrees, launch helpers, persistence. Pure logic, unit-tested. Used by the server only. | no |
| `ide-proto` / `ide-server` / `ide-client` | Client/server split (see `docs/server-architecture.md`): wire protocol, the per-user daemon owning PTYs/sessions/git, the client library. | no (client: optional iced adapter) |
| `ide-i18n` | UI translations: Fluent files embedded at build time, generated type-safe `tr::` API, current language. | no |
| `ide-term` | Client-side terminal model fed by server bytes (`TermModel`) and the iced terminal widget (`TerminalView`): rendering, selection/copy, scrollback, resize, Korean IME, xterm mouse reporting (`mouse.rs`: clicks, drags, motion and the wheel go to the program when it asked for them, SGR / UTF-8 / legacy encoding; Shift bypasses it for selection; Claude Code's clickable UI uses it). No PTY. | iced |
| `ide-web` | CEF runtime init/subprocess handling and an iced widget showing one browser off-screen (GPU texture import, input forwarding, IME). | iced |
| `ide-views` | File tree, code view/editor, diff view, changes list widgets fed by plain view models. | iced |
| `ide-app` | The binary (client only): window layout, sidebar, connection status, `ide-client` calls, one `TermModel` per session, the per-session detail area (web page, files, diffs, tree, changes, search), the input box under agent terminals (`composer.rs`: Desk's Composer for CLI agents; Enter sends the text as one bracketed paste and then Enter, Shift+Enter breaks the line, ↑/↓ walk the per-session history, the draft survives session switches; the box grows to about 10 lines, then scrolls with a scrollbar (`ide_views::CodeEditor`'s plain look: grows with its text, then scrolls; chips drawn as pills), with Desk's round ↑ send button inside it; attachments as chips (in the text: U+2068, the chip id as variation selectors, blank braille cells as wide as the label at 11.5 px, an × slot, U+2069; the editor draws the pill, the small label and the ×; the cursor and selection never stop inside a chip; sent as its path, quoted when it has spaces): files dropped on the window or copied in the file manager, and clipboard images (Ctrl+V; PNG `screenshot-<time>.png` via `attach.put`); a local server gets a dropped file's own path, a remote one a copy. With the box hidden the same paths are typed into the agent's own input (a drop, or Ctrl+V in the terminal without text). Header toggle 입력창, `ui.composer_open`), `ui.json`. | iced |

## Core concepts
- **Provider / Account / Configuration**: see `docs/agent-providers.md`. A provider is an agent
  CLI (crate `ide-agents`), an account one login of it (its own config dir under
  `accounts/<provider>/<slug>/`), a configuration a provider-agnostic bundle of skills, MCP servers
  and instructions layered on top per session. (They replace the old profiles, which are migrated.)
- **Session**: one agent run = (provider, account, configuration, repo, worktree, terminal). Each session gets its
  own git worktree on its own branch (`ide/<session-id>`) under the app data dir
  (`worktrees/<repo-slug>/<session-id>`), so sessions never share a checkout.
- **Agent kind**: `Claude` (`claude`), `Codex` (`codex`), `Shell` (cmd / pwsh / $SHELL).
  The agent's own TUI is the chat UI (no structured stream parsing in the MVP).
- **Detail area**: per-session open items (the web page, files, diffs, commits) shown one at a
  time under a header with an "open items" switcher, plus a side panel (Changes / History /
  Explore, with the file and text search in Explore; see `docs/side-panel.md`). Items
  belong to a session, so switching sessions switches the detail area (see
  `docs/server-architecture.md`, "App integration (phase 2)"). The header + item part can be
  detached into a window of its own (`docs/detail-window.md`; the app is an `iced::daemon`).

## Languages (i18n)
- All UI text lives in `crates/ide-i18n/locales/<code>/ide.ftl` ([Project Fluent](https://projectfluent.org)).
  English (`en`) is the reference and fallback; Korean (`ko`) is complete.
- `ide-i18n/build.rs` reads the English file and generates one function per message
  (`new-session` -> `tr::new_session()`, `configuration-sessions` -> `tr::configuration_sessions(total: i64,
  running: i64)`), plus the `Lang` enum (one variant per locale directory, `Lang::native_name()` from
  the reserved `language-name` message). Variables used as a plural selector or in `NUMBER()` are
  `i64`, all others `&str`. A wrong key or missing argument is a compile error.
- The build fails on Fluent syntax errors and when a locale's message set or per-message variables
  differ from English. Unit tests format every message in every locale and fail on generated
  functions no crate calls.
- Runtime: a global current language (`set_language`, `current`); views call `tr::` on every
  `view`, so switching is instant. `LanguagePref { System, Fixed(Lang) }` is stored in `ui.json`
  (`"language": "system" | "en" | "ko"`, missing = system); `System` negotiates the OS UI locales
  (`sys-locale`) against the available languages, falling back to English.
- `ide-core` stays language-agnostic: `Error::i18n()` returns an `ErrorMessage` (stable id + args,
  untranslatable details such as git's stderr passed through); the server sends it as a
  `ProtoError` and `ide-app/src/i18n.rs::proto_error` maps every id to `tr::err_*`. `Display` stays English for logs. Errors are translated when they happen, so an error
  already on screen keeps its language after a switch.
- Korean UI text renders through iced's `Shaping::Auto` (advanced shaping for non-ASCII text) and
  cosmic-text's system fallback (Malgun Gothic on Windows); terminal output is never translated.

## MVP (milestone 1)
1. Window layout: left sidebar (configuration filter + sessions), centre terminal of the selected session,
   right detail area with tabs; resizable split.
2. New-session flow: pick repo folder, provider, account, configuration -> create worktree -> spawn the agent in a PTY.
3. Terminal widget ported from the spike: colours, wide chars, box/block drawn as quads, selection +
   Ctrl+C / Ctrl+V (bracketed paste), wheel scrollback, resize, Korean IME preedit at the cursor.
4. Session switching keeps every PTY alive (background sessions keep running, in the server; they
   also survive closing the app and are replayed on reopen).
5. Detail area: one CEF tab per session showing a URL (preview placeholder).
6. Persistence: accounts, configurations and session list survive restart; worktrees are reused. A
   session terminal's output tail (`sessions/<id>/output.log`) is replayed into the scrollback after
   a server restart (`docs/server-architecture.md`, "Output log across server restarts").

Later: file viewer/editor (iced `text_editor` + highlighter), diff viewer, previews (bundled
tsx/vue/svelte), mods, structured agent output, settings UI, packaging (bundle CEF).
