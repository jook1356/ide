# Previews

Status: implemented (roadmap feature 4; protocol: the parent bumps the version when merging).
Hermes Desk draws ` ```tsx preview ` blocks of its chat answers in a renderer sandbox
(`previewGuide.ts`); an ide session is a terminal TUI with no answer blocks to read, so its
agent shows things with a tool instead: `ide_preview` opens (or reloads) a tab in the session's
detail area. No approval: a preview only shows something in the user's own window.

Code: `ide-core/src/preview.rs` (records, pages, files inside the session folder, Markdown),
server `ide-server/src/service/previews.rs` (the `ide_preview` tool, `preview.file`), app
`ide-app/src/previews_ui.rs` (tabs, the `ide-preview://` scheme). Tests: `ide-core`
`preview::tests`, `ide-server/tests/previews_e2e.rs`, app script `IDE_APP_SCRIPT_ONLY=preview`
(`script_previews.rs`, real CEF).

## Agent tool `ide_preview`

`{action?, title, kind, content?, path?, url?, id?}`; `action` is `show` (default), `guide`,
`list` or `close`.

| Kind | With | Shows |
|---|---|---|
| `html` | `content` | the page (a whole document or a fragment) |
| `markdown` | `content` | it rendered (pulldown-cmark: tables, task lists, strikethrough, footnotes) in ide's dark style |
| `file` | `path` (relative to the session folder, or absolute inside it) | `.html` / `.htm`, `.md` / `.markdown` (rendered), `.txt`, images (`png jpg gif webp svg ico bmp`) |
| `url` | `url` (or `path` / `content`), `http(s)://` only | a normal browser tab |

- `id`: 1-40 of `a-z0-9-` (a URL host label, like widget ids); default: made from the title
  (`"Login page v2"` -> `login-page-v2`, nothing usable -> `preview`). The same id replaces
  the preview and reloads its tab.
- The daemon checks and stores (`<data>/sessions/<session>/previews/<id>.json`, and `<id>.html`
  for `html` / `markdown` text), audits `preview.show <session> <id> <kind>`, then asks an app
  (`Notify::AgentRequest`, `{action: show, preview, title, kind, url}`); the answer is
  `{id, kind, file?, tab: opened | reloaded}`. No app: an error (the preview stays stored).
- `close {id}`: the daemon forgets the preview, the app closes its tab (`tab: closed` /
  `it was not open`). `list`: this session's previews. `guide`: the long text (base
  instructions mention the tool in one sentence).
- Limits: 8 MiB of content or per file, titles up to 120 characters.
- A session's previews go with the session (its data folder is removed).

## Serving (`ide-preview://`)

- Registered in `main` next to `ide-widget` (`ide_web::register_custom_schemes`): standard,
  secure, CORS- and fetch-enabled; each preview has its own origin
  `ide-preview://<id>.<session>`.
- The app answers (`ide_web::set_scheme_handler`) only requests of the tab showing that preview
  (the request's view = that tab, host = that preview), `GET` / `HEAD` only (else 405); others
  get 403. There is no ide API (unlike widgets' `/__ide/`): a preview page cannot call tools or
  reach other previews' files.
- Files come from the server (`preview.file {session, preview, path}` -> `WidgetFile {mime,
  data}`): `path` empty = the page (Markdown rendered); else a file relative to the shown file's
  folder (`file`) or to the session folder (`html` / `markdown`), never outside the session
  folder (canonical paths; `..` refused). Errors are `preview {detail}`.

## App

- A preview tab is a browser tab with `Browser.preview` (`PreviewTab {session, id, title,
  kind}`), in the session's detail area (the folder owner's, like other browser tabs). Pages
  show the title instead of the address, a reload and developer tools in the header (▣ with a
  tooltip); `url` previews keep the normal browser header (the user may browse on).
- Opening shows the tab (work page, detail area expanded); the same id navigates the open tab
  again. Preview tabs are not remembered across restarts (unlike widget tabs).

## Protocol

| Request | Payload |
|---|---|
| `preview.file` | `{session, preview, path}` -> `WidgetFile {mime, data}` |

Agent tool name: `ide_proto::AGENT_TOOL_PREVIEW`. Error id `preview {detail}`.

## Not like Desk (yet)

- No React / TSX runtime (Desk bundles one and transpiles the block): write HTML (CDN modules
  work).
- No restore of open preview tabs after a restart; no preview list in the UI.
- `macOS` / Linux: the scheme code is not OS-specific; the app script ran on Windows only.
