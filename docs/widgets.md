# Widgets

Status: implemented (protocol 17). Hermes Desk's mods, renamed: "mode" would be confused with
the agent CLIs' own modes (plan mode, permission modes) and "mod" with Claude Code's own mods.
A widget is a small web app (HTML / CSS / JS, any framework or none, no build step) that a
session's agent drafts in its folder and registers once; like Desk's mod classes, every session
can then open it as a tab of the detail area. The page gets `window.meninx`: the bridge's `meninx_*`
tools (allowed by the user at registration) and stored data.

Code: `meninx-core/src/widget.rs` (drafts, registry, files, data), server
`meninx-server/src/service/widgets.rs` (`widget.*`, the `meninx_widget` tool), CEF scheme
`meninx-web/src/scheme.rs`, app `meninx-app/src/widgets_ui.rs` (tabs, scheme, registration dialog)
and `widgets_page.rs` (위젯 page, status-bar launcher). Tests: `meninx-core` `widget::tests`,
`meninx-server/tests/widgets_e2e.rs`, app script `MENINX_APP_SCRIPT_ONLY=widget` (`script_widgets.rs`,
real CEF: registration dialog, page, launcher, menu, `window.meninx`, remembered tab, agent
open / close, delete).

## Drafts and registration

- Draft: `<session folder>/.meninx/widgets/<id>/` (`id`: 1-40 of `a-z0-9-`, not starting or ending
  with `-`: it is a URL host label). `widget.json`: `name`, `description`, `entry` (default
  `index.html`), `tools` (`meninx_*` names the bridge has, the host features `exec` / `fs` /
  `terminal` / `sessions`, or `*` for everything), `scope` (`session` default / `family` /
  `global`). Unknown keys are refused (typos show up in the agent's `list`).
- `meninx_widget register` copies the draft (files and folders, no links; 8 MiB per file, 64 MiB in
  all) to `<data>/widgets/sources/<id>/` and records it in `<data>/widgets/registry.json`
  (name, description, entry, tools, scope, version, registering sessions, times).
- Registering asks the user first (dialog 위젯 등록, Desk's "모드 등록 (모든 세션에서 사용)") when the
  widget is new, was registered by another session, or its tool list changed; the agent's call
  waits for the answer (5 min). 허용 sends `widget.register` with the tools shown (refused with
  `widget-changed` when the draft lists others now); closing / 거절 answers the agent "refused".
  The same session updating its widget with the same tools registers without asking.
- The tab serves the registered copy, never the draft: change, register again, open again.
- Delete: the 위젯 page (`widget.delete`) or the registering session's agent (`unregister`):
  the copy, its open tabs and its global data go. Session / family data stays in those
  sessions' data folders until they are deleted.

## Scopes

`scope` decides where `meninx.data` lives, and so how the widget is shown:

| Scope | Data | Tab |
|---|---|---|
| `session` | `<data>/sessions/<folder owner>/widgets/<id>.json` (shared children: the parent's) | one per session |
| `family` | `<data>/sessions/<family root>/widgets-family/<id>.json` | one per session, same data in the family |
| `global` | `<data>/widgets/data/<id>.json` | one 공용 tab, shown in every session |

## Serving (`meninx-widget://`)

- `meninx_web::register_custom_schemes(&["meninx-widget"])` first in `main` (CEF helper processes need
  the list): standard, secure, CORS- and fetch-enabled, so a widget page has its own origin
  `meninx-widget://<id>.<session>` (`session`: the session showing the tab; for a 공용 tab the one
  it was opened from).
- CEF asks `meninx_web::set_scheme_handler`'s handler on its IO thread; the app answers through a
  `Responder`. The app serves a request only for the tab it opened for that widget (the
  request's browser = that tab's view, host = that widget): files through `widget.file` (the
  registered copy); HTML gets `<script src="/__meninx/meninx.js">` at the start of `<head>`.
- `/__meninx/call` and `/__meninx/data` (POST JSON) answer only the widget's own frame (frame origin
  = widget origin), not pages it embeds. Answers are `{ok, text | value | error}` (HTTP 200).
- `window.meninx`: `call(tool, args)` (JSON result parsed, else text; rejects on error),
  `data.get / set / delete / keys`, `widget = {id, session}`.
- `widget.call` refuses tools the registration did not allow (`widget-not-allowed`) and runs the
  tool as the session showing the tab would (audit `widget.call <session> <widget> <tool>`).

## Host API (protocol 22)

Desk's mods are sandboxed (`deskAccess` asks per call); Meninx's widgets are the user's own tools,
so a page reaches Meninx directly, as the user, through `widget.host` (`meninx-server/src/service/
widget_host.rs`, `/__meninx/host` in the app). Which features a widget may use is its `tools`
list (allowed at registration) minus what the user blocked (below).

| Feature | Methods (`meninx.host(method, args)`; helpers on `window.meninx`) |
|---|---|
| `exec` | `exec {program, args, cwd, stdin, env, timeout_ms}` or `{command}` (`cmd /d /s /c` / `sh -c`) -> `{code, stdout, stderr, timed_out, truncated, ms}`; no console window, default 120 s, at most 3600 s, 16 MiB kept per stream |
| `fs` | `fs.read {path, encoding?: base64}` (at most 32 MiB), `fs.write {path, text \| base64, append?, mkdirs? = true}` (atomic replace), `fs.list`, `fs.stat`, `fs.delete {recursive?}`, `fs.mkdir` |
| `terminal` | `term.open {command, cwd, title, shell, session}` (a user terminal, `uterm.open` with any folder), `term.list {session \| "*", ports?}` (with listening ports), `term.read {id, lines}`, `term.write {id, text, keys}` (no approval: the widget is the user's), `term.stop`, `term.rerun`, `term.remove` |
| `sessions` | `sessions.list`, `sessions.get {id?}`: id, title, folder, parent, shared (works in the parent's folder), archived, repositories |

Relative paths are under the showing session's folder. A failing call answers an error result
(the page's promise rejects). Calls that change something are audited (`widget.host <session>
<widget> <method>`).

App-side helpers (answered by the app, `/__meninx/context` and `/__meninx/browser`):
`meninx.context()` -> `{selected, session}` (the session selected now, for a 공용 tab that follows
it, like Desk's global mods polling `desk.context()`); `meninx.openBrowser(url, {session}?)` opens
an http(s) page in a browser tab of that session (default: the selected one) without a site
approval, if the widget declares `meninx_browser_open` and the user did not block it.

### Block policy

`<data>/widgets/policy.json` (`meninx_core::widget::Policy`): `all` (blocked for every widget,
hand-edited) and `widgets.<id>` (the 위젯 page's 차단 / 허용 per declared tool or feature,
`widget.set_blocked`). A blocked name (or `*`) fails `widget.call` / `widget.host` with
`widget-blocked`, whatever the widget declares: meant for widgets shared by others. Deleting a
widget forgets its entries. `WidgetInfo.blocked` lists a widget's own.

## Agent tool `meninx_widget`

| Action | Answered by | What |
|---|---|---|
| `guide` | daemon | how to build and register one |
| `list` | daemon | registered widgets (`registered_by_this_session`, `open_here`) and this session's drafts with their problems |
| `register {widget}` | daemon, or app (question) | register / update the draft |
| `unregister {widget}` | daemon | only the registering session's agent |
| `open {widget}` | daemon checks, app opens | opens the tab here, or reloads it (entry page) when open |
| `close {widget}` | app | closes its tab here |

The base instructions mention widgets in one sentence; the guide is fetched on demand.

## App

- Rail -> 위젯 (Desk's 모드 page): the list beside the rail (hidden with the rail button again)
  grouped 세션 가족별 / 세션별 / 공용 데이터, each row with a dot when open in the selected
  session and its version; the picked widget's 설명, 데이터, Meninx 기능, 등록한 세션 (a link), 버전,
  수정, 열린 곳, "이 세션에서 열기" / "여기서 닫기" and 위젯 삭제 (confirmed).
- Status bar: the puzzle button (Desk's `ModLauncher`) lists 공용 and 이 세션 widgets with
  열림 / 닫힘; a click opens or closes it in the selected session; 관리 goes to the page.
- 열린 항목 -> "+ 새로 열기" lists the registered widgets too.
- A widget tab is a browser tab (`Browser.widget`): name instead of the address, reload, and
  developer tools in its header. Open tabs are remembered by the server
  (`registry.json` `open`, `widget.set_open`) and restored quietly at start (once per tab and
  run); deleting a session forgets its tabs; tabs of unregistered widgets close.

## Protocol (17)

| Request | Payload |
|---|---|
| `widget.list` | -> `Widgets([WidgetInfo {id, name, description, tools, scope, version, created_by, created_by_title, updated, open}])` |
| `widget.file` | `{session, widget, path}` -> `WidgetFile {mime, data}` |
| `widget.register` | `{session, widget, tools}` -> `Unit` (`widget-changed` when the draft lists others) |
| `widget.delete` | `{widget}` -> `Unit` |
| `widget.set_open` | `{session, widget, open}` -> `Unit` |
| `widget.call` | `{session, widget, tool, args}` -> `AgentResult` |
| `widget.data` | `{session, widget, op: Get / Set / Delete / Keys}` -> `WidgetData(Option<json text>)` |
| `widget.host` (22) | `{session, widget, method, args}` -> `AgentResult` (JSON text) |
| `widget.set_blocked` (22) | `{widget, name, blocked}` -> `Unit` |

`Notify::WidgetsChanged` after registrations, deletions and open / close. Errors:
`widget {detail}`, `widget-changed {widget}`, `widget-not-allowed {tool}`, `widget-blocked {tool}`.

## Not like Desk (yet)

- No separate OS windows (Desk's default `display: window`, 앱 위에 고정): widgets are tabs.
- One tab per session (Desk's `single: true`); no multi-instance widgets.
- No env file (secrets / allowed origins), no code view on the page. Instead of `deskAccess`
  the host API above (no per-call questions; the block policy instead).
- No events yet (Desk's `onTermData` / `onTermList`): pages poll `term.list` / `term.read`.
- React / TSX without a CDN (a bundled runtime + transpiler page, Desk's approach).
- `macOS` / Linux: the scheme code is not OS-specific, but the app script ran on Windows only.
