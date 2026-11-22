# Detail window (상세영역 새 창으로 분리)

The detail area can live in an OS window of its own, like Hermes Desk's region window for
`detail`. Code: `crates/ide-app/src/detail_window.rs` (state, messages, the window's view),
hook-ins in `app.rs` (daemon messages / subscriptions), `view.rs` / `detail_view.rs` (layout,
header buttons), `ui_state.rs` (persisted flags), `crates/ide-web/src/owner.rs` (pin, Win32),
script steps in `script_detail_window.rs`.

## Hermes Desk -> ide

| Hermes Desk (powered_ide) | ide |
| --- | --- |
| `DetailHeader` "새 창으로 분리" (`IconWindowMaximize`, right end of the header; absent inside the window) | `detail_view.rs::popout_button` at the right end of the detail header (after the hide `»`); not in the window's header. Also on the no-session header. |
| `openRegion('detail', '상세 · <label>')` | `WinMsg::Popout`: `ui.detail_detached = true`, shown; `sync_detail_window` opens the window (or `gain_focus` if open). |
| Region window = `window.open` drawn by the app renderer, always the selected session's region | Second iced window (`iced::daemon`, `window::open`); `App::view_window(id)` draws `pane(selected, in_window = true)` from the same `App` state, so it follows session switches. |
| Window title `상세 · <session> — Hermes Desk` (`상세 보기` when idle) | `상세 · <session> — ide` / `상세 보기 — ide` (`App::title(id)`, i18n `detail-window-title*`). |
| `RegionPortal` title bar: `PinButton` (`IconPin` / `IconPinFilled`, tooltips `앱 창 위에 항상 같이 띄우기` / `고정 해제 (앱 위에 같이 뜨지 않음)`, violet "light" when on) + `앱으로 합치기` button (`IconArrowBarToLeft` 14, tooltip `앱 창으로 합치기`) | A 36 px bar at the top of the window: pin (28 px, 15 px Tabler pin / pin-filled, `style::icon_toggle` violet when on) and `앱으로 합치기` (14 px arrow-bar-to-left + label), same tooltips. |
| `closeRegion` (앱으로 합치기): detached cleared, window closes, the pane shows again in the app | `WinMsg::Merge`: `detail_detached = false`, `detail_collapsed = false`, window closed by `sync_detail_window`. |
| `hideRegion` / window's own ×: closes the window, stays detached (`detailHidden`) | Detail toggle (`ToggleDetail`) and the window's close button (`WindowCloseRequest(detail)`) set `detail_collapsed = true`; the window closes, `detail_detached` stays. A window that disappears by itself (`WindowClosed`) counts as hidden. |
| `toggleDetail` with a detached region: hides / shows the window | Same `ToggleDetail`; while detached the app window keeps the thin toggle strip: `»` "분리된 상세 창 숨기기" while shown, `«` "상세 영역을 분리된 창으로 보기" while hidden (Desk's `DetailToggle` tooltips). |
| Opening an item while detached and hidden re-opens the window | Existing paths that set `detail_collapsed = false` (open file, quick open, find in files) re-open it. |
| Detached remembered across restarts (`localStorage region-detached:detail`) | `ui.json` `detail_detached` (kept while hidden); the window reopens on start once a session is selected. |
| Pin remembered (`region-pinned:detail`), applied on open | `ui.json` `detail_pinned`, applied after `window::open` completes and on toggle. |
| `region:pin` = `win.setParentWindow(pinned ? mainWindow : null)` (Electron on Windows: owner via `GWLP_HWNDPARENT`, then hide/show + `BringWindowToTop`) | `ide_web::set_window_owner(child, owner)`: the same Win32 calls on the HWNDs from `window::run` (raw-window-handle). Owned window: in front of the app window, minimises / restores with it, destroyed with it. |
| What the main window shows while detached: the conversation takes the room | The terminal takes the detail pane's width (`detail_in_app()` false -> `centre()` fills, the divider is a 1 px line). |
| Closing the app closes region windows | `exit_app` -> `iced::exit()` closes both; the main window's close button still runs the existing close dialog. `close_for_reopen` (script restart / server switch) closes the detail window. |
| Window size 960 x 640, min 420 x 240 | Same defaults; last position and size are kept in `ui.json` `detail_window` (logical px, from `Moved` / resize events and `window::position` after opening) and reused. |

The side panel (Changes / History / Explore with its footer) stays in the app window: in Desk the
Changes panel is not part of the detail region (it has its own toggle, `mod+b`). In ide the side
panel's visibility is still tied to the detail toggle (as before), so hiding the detached window
also hides the side panel; files opened from it show in the detail window.

## Spike findings (iced master 98bd48fe, CEF 156, DX12, this host: one 2x display)

- `iced::application` -> `iced::daemon`: `view(&self, window::Id)`, `title(&self, Id)`,
  `theme(&self, Id)`; the app window is opened in `boot` (`window::open`, `exit_on_close_request:
  false`); `.settings(Settings { backend: ide_web::backend(), .. })` replaces `.backend()`.
  `window::events()` must not be subscribed (it also carries `RedrawRequested`); use
  `resize_events`, `close_requests`, `close_events` and an `event::listen_with` for `Moved`.
- One wgpu device serves every window (iced's compositor), so the CEF OSR texture import works
  unchanged in the second window: the web page renders crisp (1:1) there; the script checks the
  page's colours in the window's `window::screenshot` (`ExpectWebDrawn`) and a read-only OS capture
  showed the page and the URL input value.
- The widget's per-window state re-registers CEF's waker when the view moves windows (state is new
  in the other window's tree), so frames keep arriving; no change in `ide-web` was needed.
- `ide-views` code editor and diff render in the second window (OS capture of the editor; in-app
  screenshot of the diff).
- Scale: each window's renderer has its own scale factor; CEF is sized from it. Only one monitor
  (2x) is attached here, so mixed-DPI moves were not tested.
- Input, IME and cursor are per window in iced_winit (each window has its own user interface and
  `InputMethod` requests); widget operations (`operation::focus`) run on every window, so focusing
  the terminal (session switch) unfocuses inputs in the detail window, as clicking the app would.
  Real Korean IME in the detail window's inputs / web page was not typed (no OS input from
  tests): manual check.
- The script's in-app event injector (`ide_web::testing::Injector`) wraps the app window only.

## Deviations from Desk

- Native OS title bar (with the title) plus a 36 px action bar, instead of Desk's custom title bar
  with the OS buttons drawn over it (the ide app window also uses the native title bar).
- Desk's detached window shows 열린 항목 as a tab list beside the content by default (`DetailRail`,
  with its own settings); ide keeps the header's 열린 항목 popover in the window too.
- Desk re-maximises pinned windows after the app window is restored (Windows restores owned
  windows un-maximised); not ported.
- Desk's `Ctrl+J` detail toggle does not exist in ide (not added: Ctrl+J is a newline in agent
  CLIs).
- Dialogs opened from the detail window's content (unsaved-file close, definition picker) open in
  the app window (ide's modals are drawn by the app window).
