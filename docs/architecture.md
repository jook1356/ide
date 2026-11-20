# Architecture and MVP

## Why this shape
Decided by spikes (Slint / iced / egui) recorded outside this repo. iced won on the deciding test:
a terminal hosting Claude Code / Codex TUIs (lowest draw cost, CPU and memory; Korean IME via the
public `Shell::request_input_method` API; verified with the real Microsoft Korean IME). CEF is
composited as a GPU texture (cef-rs `accelerated_osr`, wgpu 30 = iced master's wgpu).

## Crates
| Crate | Role | Depends on UI? |
|---|---|---|
| `ide-core` | Profiles, sessions, git worktrees, agent launch specs, persistence. Pure logic, unit-tested. Used by the server only. | no |
| `ide-proto` / `ide-server` / `ide-client` | Client/server split (see `docs/server-architecture.md`): wire protocol, the per-user daemon owning PTYs/sessions/git, the client library. | no (client: optional iced adapter) |
| `ide-i18n` | UI translations: Fluent files embedded at build time, generated type-safe `tr::` API, current language. | no |
| `ide-term` | Client-side terminal model fed by server bytes (`TermModel`) and the iced terminal widget (`TerminalView`): rendering, selection/copy, scrollback, resize, Korean IME. No PTY. | iced |
| `ide-web` | CEF runtime init/subprocess handling and an iced widget showing one browser off-screen (GPU texture import, input forwarding, IME). | iced |
| `ide-views` | File tree, code view/editor, diff view, changes list widgets fed by plain view models. | iced |
| `ide-app` | The binary (client only): window layout, sidebar, connection status, `ide-client` calls, one `TermModel` per session, the per-session detail area (web page, files, diffs, tree, changes, search), `ui.json`. | iced |

## Core concepts
- **Profile**: a named set of agent config. Holds its own Claude config dir and Codex home so skills
  and MCP servers stay per profile. Launching an agent sets `CLAUDE_CONFIG_DIR` / `CODEX_HOME` to the
  profile's dirs. Stored under the app data dir: `profiles/<name>/{claude,codex}/`.
- **Session**: one agent run = (profile, repo, worktree, agent kind, terminal). Each session gets its
  own git worktree on its own branch (`ide/<session-id>`) under the app data dir
  (`worktrees/<repo-slug>/<session-id>`), so sessions never share a checkout.
- **Agent kind**: `Claude` (`claude`), `Codex` (`codex`), `Shell` (cmd / pwsh / $SHELL).
  The agent's own TUI is the chat UI (no structured stream parsing in the MVP).
- **Detail area**: per-session open items (the web page, files, diffs) shown one at a time under a
  header with an "open items" switcher, plus a side panel (Changes / Explore / Search). Items
  belong to a session, so switching sessions switches the detail area (see
  `docs/server-architecture.md`, "App integration (phase 2)").

## Languages (i18n)
- All UI text lives in `crates/ide-i18n/locales/<code>/ide.ftl` ([Project Fluent](https://projectfluent.org)).
  English (`en`) is the reference and fallback; Korean (`ko`) is complete.
- `ide-i18n/build.rs` reads the English file and generates one function per message
  (`new-session` -> `tr::new_session()`, `profile-sessions` -> `tr::profile_sessions(total: i64,
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
1. Window layout: left sidebar (profiles + sessions), centre terminal of the selected session,
   right detail area with tabs; resizable split.
2. New-session flow: pick repo folder, profile, agent kind -> create worktree -> spawn the agent in a PTY.
3. Terminal widget ported from the spike: colours, wide chars, box/block drawn as quads, selection +
   Ctrl+C / Ctrl+V (bracketed paste), wheel scrollback, resize, Korean IME preedit at the cursor.
4. Session switching keeps every PTY alive (background sessions keep running, in the server; they
   also survive closing the app and are replayed on reopen).
5. Detail area: one CEF tab per session showing a URL (preview placeholder).
6. Persistence: profiles and session list (not scrollback) survive restart; worktrees are reused.

Later: file viewer/editor (iced `text_editor` + highlighter), diff viewer, previews (bundled
tsx/vue/svelte), mods, structured agent output, settings UI, packaging (bundle CEF).
