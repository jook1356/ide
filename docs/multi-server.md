# Several servers in one app

Status: in progress (2026-11-28). Code: `meninx-app/src/shell.rs` (one `App` per server),
`ui_state.rs` (`servers`, globals), the title bar's server menu (`server_menu.rs`), the settings
page's server list. Earlier the app had one server (Local or one SSH host) and switching
reloaded the whole app (`close_for_reopen`).

## What lives where

The rule: **data lives where the agents run**. The app stays a client and moves no data.

| Per server (its daemon, as before) | Per PC (the app) |
|---|---|
| sessions, worktrees, terminals, repos, accounts (CLI logins stay on the machine that runs the CLI), profiles (MCP servers are programs of that machine), providers, approvals, auto-answers, widgets (files and data: the app's CEF renders them, their files come through `widget.file`) | the server list, UI language, fonts, window and panel layout, the CEF browser profile, bookmarks |

The same name on two servers is two things (a "기본" profile on each). Copying a profile to
another server is a later addition.

## Model

- **Server list** (`ui.json` `servers`): Local (always, first, not stored) and SSH entries
  `{id, name, kind: "ssh", host, command, auto_connect}`. `current_server` = the shown one's id
  (`local` = Local). An old `server: {kind: "ssh", ...}` becomes one entry, shown.
  `MENINX_REMOTE=ssh:<host>` replaces the list with that server alone (tests, scripts).
- **One `App` per server** (`shell.rs` `Shell`): the existing `App` is already per server
  (switching used to rebuild it), so each server gets its own: connection, sessions, lists,
  terminals, browser tabs, notices, detail state. The shell holds them, shows the current one
  and keeps the others connected in the background (`auto_connect`, default on; a server with
  it off connects when it is picked). Switching is instant (terminals already attached) and a
  background server's events keep arriving.
- **Routing**: each `App` has a serial; its tasks and subscriptions are tagged with it
  (`Shell::App(serial, message)`), so a reply always reaches the `App` that asked, also after a
  switch, and a rebuilt `App` (new serial) ignores the old one's replies. CEF events and the
  `meninx-widget://` / `meninx-preview://` requests are subscribed to once by the shell and go
  to the `App` that owns the web view (`App::owns_view`), else the current one. Window and
  keyboard events go to the current `App`; window resizes to all.
- **UI state**: Local's `App` keeps `<data>/ui.json`; another server's keeps
  `<data>/servers/<id>/ui.json`. Per-server fields: selected session, active configuration,
  last repo folder, per-session detail items, folded groups / families / archives, default
  shell, recent terminal folders. All other fields are **globals** (`UiState::GLOBALS`): the
  shell copies a change from the current `App` to the others and Local's `ui.json` is their
  record.
- **Switching** (`Shell::switch`): the old `App` closes its detail window (the detached
  setting stays), the new one gets the window id, size and focus, reopens the detail window if
  detached, and is connected if it was not. Its selected session is the one last shown there.
- **Closing the window**: as before for Local's sessions (ask / keep / stop); when another
  server is shown and Local's sessions run, Local is shown first so the question is visible.
  Remote servers' sessions keep running anyway.

## UI

- **Title bar**: the server menu beside the brand: the current server's name and a dot for
  its connection; open: every server with its state (connected / connecting / not connected)
  and counts (working, waiting for the user), a retry for a disconnected one, and 서버 관리
 (the settings page). A dot on the button when another server has a session waiting for the
  user or a new answer.
- **Settings -> 서버**: the list (Local first, then SSH servers): auto-connect, edit, remove, and
  서버 추가. Adding and editing open a dialog: SSH host and name with hints, the server command
  under 고급, auto-connect, 연결 테스트 (a throwaway client: the server version or why not). A change rebuilds that server's `App`
  (its sessions keep running on the server).
- Every other page shows the current server's data, as before (repos, accounts, profiles,
  widgets, the session list).

## Later

- One session list with every server's sessions (grouped by server): the per-server `App`s
  stay; the list draws all of them.
- Background notices in the current server's bell (now: the menu's counts and dot).
- Copy a profile to another server; a widget cache for remote servers.
