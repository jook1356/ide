# Detail window (상세영역 새 창으로 분리)

The detail area can live in an OS window of its own, like Hermes Desk's region window for
`detail`. Code: `crates/meninx-app/src/detail_window.rs` (state, messages, the window's view),
hook-ins in `app.rs` (daemon messages / subscriptions), `view.rs` / `detail_view.rs` (layout,
header buttons), `ui_state.rs` (persisted flags), `crates/meninx-web/src/owner.rs` (pin, Win32),
script steps in `script_detail_window.rs`.

## Hermes Desk -> Meninx

| Hermes Desk (powered_ide) | Meninx |
| --- | --- |
| `DetailHeader` "새 창으로 분리" (`IconWindowMaximize`, right end of the header; absent inside the window) | `detail_view.rs::popout_button` at the right end of the detail header (after the hide `»`); not in the window's header. Also on the no-session header. |
| `openRegion('detail', '상세 · <label>')` | `WinMsg::Popout`: `ui.detail_detached = true`, shown; `sync_detail_window` opens the window (or `gain_focus` if open). |
| Region window = `window.open` drawn by the app renderer, always the selected session's region | Second iced window (`iced::daemon`, `window::open`); `App::view_window(id)` draws `pane(selected, in_window = true)` from the same `App` state, so it follows session switches. |
| Window title `상세 · <session> — Hermes Desk` (`상세 보기` when idle) | `상세 · <session> — Meninx` / `상세 보기 — Meninx` (`App::title(id)`, i18n `detail-window-title*`). |
| `RegionPortal` title bar: `PinButton` (`IconPin` / `IconPinFilled`, tooltips `앱 창 위에 항상 같이 띄우기` / `고정 해제 (앱 위에 같이 뜨지 않음)`, violet "light" when on) + `앱으로 합치기` button (`IconArrowBarToLeft` 14, tooltip `앱 창으로 합치기`) | A 36 px bar at the top of the window: pin (28 px, 15 px Tabler pin / pin-filled, `style::icon_toggle` (selected ground, accent icon) when on) and `앱으로 합치기` (14 px arrow-bar-to-left + label), same tooltips. |
| `DetailRail` (`lib/detailRail.ts` settings `{ mode: 'tabs', side: 'left', pinned: false }` in localStorage): in the detached window 열린 항목 is a vertical tab list under the pane's header (the header's 열린 항목 button hides). Pinned: 240 px list beside the content; unpinned: 44 px icon rail (group lines in the group colour, unsaved dot) that widens to 240 px over the content after 120 ms of hover and narrows when the pointer leaves. Top: 열린 항목 설정 menu (세로 탭 / 팝오버, 탭 위치 왼쪽 / 오른쪽, 작업 중 아닌 항목 닫기) and the list's pin. The popover in the window has "세로 탭으로 보기". The app window's pane always uses the popover. | `detail_rail.rs`, settings `ui.json` `detail_rail` (`RailPrefs`, same defaults). Same layout, widths, delay, groups (Browser orange, Changes yellow, History teal, Explore accent: `style` tokens), menu and pin; rows are the popover's (icon, name, unsaved dot, ✕). Popover footer button "세로 탭으로 보기" when in the window. |
| Frameless region window (`frame: false`, title bar drawn by the page, OS caption buttons on the right) | `frame.rs`, shared with the app window (bar label `meninx`, no actions; its × runs the close dialog; the bar stays above modals). `decorations: false` (DWM shadow and rounded corners kept via `undecorated_shadow` / `CornerPreference::Round`). The bar shows `상세 · <session>` on the left and drags the window (`window::drag`, Aero snap included; double click toggles maximise); Windows 11-style caption buttons (46 px, drawn 10 px glyphs, red close hover): minimise, maximise / restore (from `window::is_maximized` after each resize), close (= the old native ×: hides, stays detached). 5 px invisible grips along the edges resize it (`window::drag_resize`), absent while maximised. |
| `closeRegion` (앱으로 합치기): detached cleared, window closes, the pane shows again in the app | `WinMsg::Merge`: `detail_detached = false`, `detail_collapsed = false`, window closed by `sync_detail_window`. |
| `hideRegion` / window's own ×: closes the window, stays detached (`detailHidden`) | Detail toggle (`ToggleDetail`) and the window's close button (`WindowCloseRequest(detail)`) set `detail_collapsed = true`; the window closes, `detail_detached` stays. A window that disappears by itself (`WindowClosed`) counts as hidden. |
| `toggleDetail` with a detached region: hides / shows the window | Same `ToggleDetail`; the toggle is Desk's `DetailToggle` at the right end of the session header ("상세 영역" + open item count, tooltips "분리된 상세 창 숨기기" / "상세 영역을 분리된 창으로 보기" while detached); the app window keeps no strip for the hidden area. |
| Opening an item while detached and hidden re-opens the window | Existing paths that set `detail_collapsed = false` (open file, quick open, find in files) re-open it. |
| Detached remembered across restarts (`localStorage region-detached:detail`) | `ui.json` `detail_detached` (kept while hidden); the window reopens on start once a session is selected. |
| Pin remembered (`region-pinned:detail`), applied on open | `ui.json` `detail_pinned`, applied after `window::open` completes and on toggle. |
| `region:pin` = `win.setParentWindow(pinned ? mainWindow : null)` (Electron on Windows: owner via `GWLP_HWNDPARENT`, then hide/show + `BringWindowToTop`) | `meninx_web::set_window_owner(child, owner)`: the same Win32 calls on the HWNDs from `window::run` (raw-window-handle). Owned window: in front of the app window, minimises / restores with it, destroyed with it. |
| What the main window shows while detached: the conversation takes the room | The terminal takes the detail pane's width (`detail_in_app()` false -> `centre()` fills, the divider is a 1 px line). |
| Closing the app closes region windows | `exit_app` -> `iced::exit()` closes both; the main window's close button still runs the existing close dialog. `close_for_reopen` (script restart / server switch) closes the detail window. |
| Window size 960 x 640, min 420 x 240 | Same defaults; the last place is kept in `ui.json` `detail_window` and reused. Windows: the restored rectangle in physical px with the monitor's DPI and the maximised flag (`meninx_web::window_placement`, read when the window loses focus, hides, closes for a reopen or the app quits; no `Moved` subscription, so a drag does not rebuild the views at every step). The window opens hidden, is placed by `meninx_web::place_window` (clamped into the nearest monitor's work area, size scaled if that monitor's DPI changed; moved first, then sized, so a DPI change on the way does not leave winit's rescaled size), then shown and maximised if it was. A logical place saved by an older version is ignored. Elsewhere: logical px from `Moved` / resize events, not while maximised. |

The side panel (Changes / History / Explore with its footer) stays in the app window: in Desk the
Changes panel is not part of the detail region (it has its own toggle, `mod+b`). In Meninx the side
panel's visibility is still tied to the detail toggle (as before), so hiding the detached window
also hides the side panel; files opened from it show in the detail window.

## Spike findings (iced master 98bd48fe, CEF 156, DX12, this host: one 2x display)

- `iced::application` -> `iced::daemon`: `view(&self, window::Id)`, `title(&self, Id)`,
  `theme(&self, Id)`; the app window is opened in `boot` (`window::open`, `exit_on_close_request:
  false`); `.settings(Settings { backend: meninx_web::backend(), .. })` replaces `.backend()`.
  `window::events()` must not be subscribed (it also carries `RedrawRequested`); use
  `resize_events`, `close_requests`, `close_events` and an `event::listen_with` for `Moved`.
- One wgpu device serves every window (iced's compositor), so the CEF OSR texture import works
  unchanged in the second window: browser tabs render crisp (1:1) there; the script checks the
  page's colours in the window's `window::screenshot` (`ExpectWebDrawn`) and a read-only OS capture
  showed the page and the URL input value.
- The widget's per-window state re-registers CEF's waker when the view moves windows (state is new
  in the other window's tree), so frames keep arriving; no change in `meninx-web` was needed.
- `meninx-views` code editor and diff render in the second window (OS capture of the editor; in-app
  screenshot of the diff).
- Scale: each window's renderer has its own scale factor; CEF is sized from it. Only one monitor
  (2x) is attached here, so mixed-DPI moves were not tested.
- Input, IME and cursor are per window in iced_winit (each window has its own user interface and
  `InputMethod` requests); widget operations (`operation::focus`) run on every window, so focusing
  the terminal (session switch) unfocuses inputs in the detail window, as clicking the app would.
  Real Korean IME in the detail window's inputs / browser tabs was not typed (no OS input from
  tests): manual check.
- The script's in-app event injector (`meninx_web::testing::Injector`) wraps the app window only.

## Deviations from Desk

- The caption buttons are drawn by the app (Electron overlays the native ones). The snap layout
  flyout comes from a window subclass (`meninx_web::enable_snap_layout`) that answers `HTMAXBUTTON`
  over the maximise button; Windows then handles that button's click, so it shows no hover tint.
- 열린 항목 (tab list and popover): right-click menu at the pointer (`item_menu.rs`: 입력창에 첨부 (file path / commit
  reference / page address, not terminals; while the session runs), copy paths,
  Explore / file manager / VS Code (local server only), commit SHA / subject, web reload / copy /
  external browser, terminal command / stop, 닫기 / 다른 항목 닫기 / 작업 중 아닌 항목 닫기 /
  탭 모두 닫기), drag within a group (`meninx_views::Reorder`; the order is the session's item order,
  kept in `ui.json`), one 저장하지 않은 변경 question for all unsaved files, user terminals and
  "+ 새로 열기" (`uterm.rs`: 이 세션 | 공용 (Alt+click / Alt+Enter: 공용), the server's shells with
  the default ☆ (`ui.json` `default_shell`), the start folder (a repo / the session folder; 공용:
  home, recent folders, 폴더 선택… with the server folder picker), 실행 중인 서버 (ports listening in
  the scope's terminal process trees, asked every 3 s while the menu is open), a terminal per repo / the session folder / the home
  folder, 즐겨찾기 with rename / delete, the address field), browser tabs (`browser.rs`: Desk's
  `addressToUrl`, a page's new window opens a tab, ☆ in the header), 공용 items (no session; server
  terminals `g:t<n>` in the home folder) and 공통 tabs (pinned from the item menu, grouped by
  their session at the top; clicking one of another session switches to it). Not ported:
  agent processes,
  persisting browser tabs and 공통 tabs across restarts, the drag's dimming
  of other groups.
- Opening on hover uses iced's enter / exit events (Desk polls the OS cursor because its web pages
  are native views; Meninx's are drawn by iced).
- Desk's `Ctrl+J` detail toggle does not exist in Meninx (not added: Ctrl+J is a newline in agent
  CLIs).
- The pinned detail window is maximised again after the app window comes back from minimised
  (Desk's 50 ms re-maximise), detected from the app window's resize events.
- The unsaved-file close and definition picker dialogs draw in the detail window while it is
  detached (other dialogs belong to the app window's content and stay there).
