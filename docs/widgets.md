# Widgets

Status: implemented (protocol 16). Hermes Desk's mods, renamed: "mode" would be confused with
the agent CLIs' own modes (plan mode, permission modes) and "mod" with Claude Code's own mods.
A widget is a small web app (HTML / CSS / JS, any framework or none, no build step) that a
session's agent writes into the session folder; ide shows it as a tab of the detail area and
gives the page `window.ide`: the bridge's `ide_*` tools (approved by the user) and stored data.

Code: `ide-core/src/widget.rs` (folder, manifest, files, data, approvals), server
`ide-server/src/service/widgets.rs` (`widget.*`, the `ide_widget` tool), CEF scheme
`ide-web/src/scheme.rs`, app `ide-app/src/widgets_ui.rs`. Tests: `ide-core` `widget::tests`,
`ide-server/tests/widgets_e2e.rs`, app script `IDE_APP_SCRIPT_ONLY=widget`
(`script_widgets.rs`, real CEF: `window.ide`, tools, data, refused tool, files outside).

## Files

- `<session folder>/.ide/widgets/<id>/` (`id`: 1-40 of `a-z0-9-`, not starting or ending with
  `-`: it is a URL host label). Shared children see their parent's widgets (same folder). An
  original-repo session's folder is the repository, like `.ide/pr-*.md`.
- `widget.json`: `name` (tab title), `description`, `entry` (default `index.html`), `tools`
  (`ide_*` names the bridge has; others make the widget unusable), `scope` (`session` default /
  `family` / `global`). Unknown keys are refused (typos show up in `list`).
- Every other file of the folder is served (up to 8 MiB); paths leaving the folder (`..`,
  absolute, links out) are refused.

## Serving (`ide-widget://`)

- `ide_web::register_custom_schemes(&["ide-widget"])` first in `main` (CEF helper processes need
  the list): standard, secure, CORS- and fetch-enabled, so a widget page has its own origin
  `ide-widget://<id>.<session>` (relative URLs, `localStorage`, `fetch` to itself).
- CEF asks `ide_web::set_scheme_handler`'s handler on its IO thread; the app answers through a
  `Responder` (async: `Callback::Continue` when the answer is there). The app serves a request
  only for the tab it opened for that widget (the request's browser = that tab's view, host =
  that widget): files through `widget.file` (the server reads the session folder, so SSH
  servers work too); HTML gets `<script src="/__ide/ide.js">` at the start of `<head>`.
- `/__ide/call` and `/__ide/data` (POST JSON) answer only the widget's own frame (frame origin
  = widget origin), not pages it embeds. Answers are `{ok, text | value | error}` (HTTP 200).
- `window.ide`: `call(tool, args)` (JSON result parsed, else text; rejects on error),
  `data.get / set / delete / keys`, `widget = {id, session}`.

## Approvals and data

- A widget with `tools` opens only after the user allowed exactly that list (dialog 위젯 허용;
  `widget.grant`, stored per folder owner in `<data>/sessions/<owner>/widget-grants.json`). A
  changed list asks again; `widget.call` refuses undeclared tools and unapproved lists
  (`widget-not-allowed`) and runs the tool as the session's agent would (audit
  `widget.call <session> <widget> <tool>`).
- Data (JSON values, at most 1 MiB per widget and scope): `session` ->
  `<data>/sessions/<folder owner>/widgets/<id>.json`, `family` -> the family root's
  `widgets-family/<id>.json`, `global` -> `<data>/widgets/<id>.json`.

## Agent tool `ide_widget`

| Action | Answered by | What |
|---|---|---|
| `guide` | daemon | how to build one (the folder, `widget.json`, `window.ide`) |
| `list` | daemon | widgets with name, tools, scope, `approved_by_user`, `problem` |
| `open {widget}` | daemon checks, app opens | opens the tab, or reloads it (entry page) when open; asks the user first when the tools are not approved (the agent is told) |
| `close {widget}` | app | closes its tab |

The base instructions mention widgets in one sentence; the guide is fetched on demand.

## App

- 열린 항목 -> "+ 새로 열기" lists the selected session's widgets (위젯); a broken one is
  disabled with its problem as the tooltip.
- A widget tab is a browser tab (`Browser.widget`): name instead of the address, reload, and
  developer tools in its header; not remembered across restarts (like browser tabs).

## Protocol (16)

| Request | Payload |
|---|---|
| `widget.list` | `{session}` -> `Widgets([WidgetInfo {id, name, description, tools, scope, granted, error}])` |
| `widget.file` | `{session, widget, path}` -> `WidgetFile {mime, data}` |
| `widget.grant` | `{session, widget, tools}` -> `Unit` (`widget-changed` when the manifest lists others) |
| `widget.call` | `{session, widget, tool, args}` -> `AgentResult` |
| `widget.data` | `{session, widget, op: Get / Set / Delete / Keys}` -> `WidgetData(Option<json text>)` |

Errors: `widget {detail}`, `widget-changed {widget}`, `widget-not-allowed {tool}`.

## Later

- React / TSX without a CDN (a bundled runtime + transpiler page, Desk's approach).
- Remember open widget tabs; a widget list in the session's side panel.
- `macOS` / Linux: the scheme code is not OS-specific, but the app script ran on Windows only.
