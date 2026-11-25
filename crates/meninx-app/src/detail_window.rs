//! The detail area in a window of its own (Hermes Desk's region window for `detail`:
//! `lib/regionWindows.ts`, `components/RegionPortal.tsx`; see `docs/detail-window.md`).
//!
//! - "새 창으로 분리" (detail header) detaches the area: it is drawn into a second iced window
//!   (same `App` state, same widgets; it always shows the selected session's detail) and the
//!   terminal takes its room in the app window. The side panel (Changes / History / Explore)
//!   stays in the app window, like Desk's Changes panel.
//! - Detached is remembered (`ui.json`), also while the window is hidden: the detail toggle and
//!   the window's own close button hide it, the toggle (or opening an item) shows it again, and
//!   it comes back after a restart. Only "앱으로 합치기" puts the area back into the app window.
//! - The pin keeps the window in front of the app window (owned window, like Electron's
//!   `setParentWindow`): it comes up and minimises with the app.
//! - Its place is kept in `ui.json`. On Windows that is the restored rectangle in physical
//!   pixels with the monitor's DPI and the maximised flag (`meninx_web::Placement`), read when the
//!   window loses focus, hides or the app quits, and applied (clamped onto a monitor) while the
//!   window is still hidden: logical positions do not survive monitors of different scale, and
//!   a minimised window reports (-32000, -32000).
//! - Frameless like Desk's region window (`frame.rs`): the app draws the title bar, with the pin
//!   and 앱으로 합치기 beside the caption buttons.

use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Element, Fill, Point, Size, Task, Widget as _, window};

use crate::app::{App, Message};
use crate::frame;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::style;
use crate::ui_state::WindowGeom;
use crate::view::tip;

type El<'a> = Element<'a, Message>;

/// Desk's region window: 960 x 640 at first, at least 420 x 240.
const DEFAULT_SIZE: Size = Size::new(960.0, 640.0);
const MIN_SIZE: Size = Size::new(420.0, 240.0);

#[derive(Debug, Clone)]
pub enum WinMsg {
    /// "새 창으로 분리" (detail header).
    Popout,
    /// "앱으로 합치기" (the window's bar).
    Merge,
    /// The window's pin button.
    TogglePin,
    /// The window exists (apply the pin).
    Opened(window::Id),
    /// Where the window was placed (by the OS or from `ui.json`).
    Placed(window::Id, Option<Point>),
    /// The detail window's owner after a pin change: (app window, owner) handles.
    Owner(Option<isize>, Option<isize>),
    /// The window's placement was read (Windows): remember it in `ui.json`.
    Saved(Option<meninx_web::Placement>),
    /// Same, then quit the app (`exit_app`).
    SavedThenExit(Option<meninx_web::Placement>),
}

/// Read the detail window's placement (Windows; nothing elsewhere, where `Moved` / resize
/// events keep it) and hand it to `then`.
fn read_placement(id: window::Id, then: fn(Option<meninx_web::Placement>) -> WinMsg) -> Task<Message> {
    if !cfg!(windows) {
        return Task::none();
    }
    window::run(id, |w| {
        frame::hwnd_of(w).and_then(meninx_web::window_placement)
    })
    .map(move |p| Message::DetailWindow(then(p)))
}

/// Remember the detail window's place (Windows), e.g. before it closes.
pub(crate) fn save_placement(id: window::Id) -> Task<Message> {
    read_placement(id, WinMsg::Saved)
}

/// A placement saved on Windows (physical pixels).
fn saved_placement(g: Option<WindowGeom>) -> Option<meninx_web::Placement> {
    let g = g?;
    Some(meninx_web::Placement {
        x: g.x.round() as i32,
        y: g.y.round() as i32,
        width: g.width.round() as i32,
        height: g.height.round() as i32,
        dpi: g.dpi?,
        maximized: g.maximized,
    })
}

impl App {
    /// The detail pane is drawn in the app window (shown and not detached).
    pub fn detail_in_app(&self) -> bool {
        !self.ui.detail_collapsed && !self.ui.detail_detached
    }

    /// Desk's `상세 · <session>` (`상세 보기` without a session), shown in the title bar.
    fn detail_window_label(&self) -> String {
        match self.selected_live() {
            Some(l) => tr::detail_window_title(&l.info.title),
            None => tr::detail_window_title_idle(),
        }
    }

    /// The window's OS title (taskbar, Alt+Tab): the label plus the app's name.
    pub fn detail_window_title(&self) -> String {
        format!("{} — Meninx", self.detail_window_label())
    }

    /// Open or close the detail window so it matches the state: open while detached and shown
    /// (once there is a session to show; it then stays open without one), closed otherwise.
    pub(crate) fn sync_detail_window(&mut self) -> Task<Message> {
        let want = self.ui.detail_detached
            && !self.ui.detail_collapsed
            && self.window.is_some()
            && (self.detail_window.is_some() || self.selected.is_some());
        match (want, self.detail_window) {
            (true, None) => self.open_detail_window(),
            (false, Some(id)) => {
                crate::script::log("detail window: close");
                self.detail_window = None;
                self.detail_window_ready = false;
                save_placement(id).chain(window::close(id))
            }
            _ => Task::none(),
        }
    }

    fn open_detail_window(&mut self) -> Task<Message> {
        // Windows: opened hidden, placed from the saved physical rectangle once it exists, then
        // shown (`show_detail_window`). A logical place saved by an older version is dropped
        // there (it may be off every monitor).
        let geom = self
            .ui
            .detail_window
            .filter(|g| !cfg!(windows) && g.dpi.is_none());
        let size = geom
            .map(|g| Size::new(g.width.max(MIN_SIZE.width), g.height.max(MIN_SIZE.height)))
            .unwrap_or(DEFAULT_SIZE);
        let position = match geom {
            Some(g) => window::Position::Specific(Point::new(g.x, g.y)),
            None => window::Position::Default,
        };
        let (id, open) = window::open(frame::frameless(window::Settings {
            size,
            position,
            visible: !cfg!(windows),
            min_size: Some(MIN_SIZE),
            // Its close button hides the area (it stays detached): handled by the app.
            exit_on_close_request: false,
            ..window::Settings::default()
        }));
        crate::script::log(&format!("detail window: open {size:?} at {position:?}"));
        self.detail_window = Some(id);
        self.detail_window_maximized = false;
        self.detail_window_size = size;
        self.detail_window_pos = None;
        open.map(|id| Message::DetailWindow(WinMsg::Opened(id)))
    }

    pub(crate) fn on_detail_window(&mut self, m: WinMsg) -> Task<Message> {
        match m {
            WinMsg::Popout => {
                crate::script::log("detail: open in a new window");
                self.ui.detail_detached = true;
                self.ui.detail_collapsed = false;
                self.open_items = false;
                self.save_ui();
                self.sync_web();
                let focus = match self.detail_window {
                    Some(id) => window::gain_focus(id),
                    None => Task::none(),
                };
                Task::batch([focus, self.sync_detail()])
            }
            WinMsg::Merge => {
                crate::script::log("detail: merge into the app window");
                self.ui.detail_detached = false;
                self.ui.detail_collapsed = false;
                self.open_items = false;
                self.save_ui();
                self.sync_web();
                self.sync_detail()
            }
            WinMsg::TogglePin => {
                self.ui.detail_pinned = !self.ui.detail_pinned;
                crate::script::log(&format!("detail window pinned: {}", self.ui.detail_pinned));
                self.save_ui();
                self.apply_pin()
            }
            WinMsg::Opened(id) => {
                if Some(id) != self.detail_window {
                    return Task::none();
                }
                crate::script::log("detail window: opened");
                self.detail_window_ready = true;
                let size = Task::batch([
                    window::size(id).map(move |s| Message::WindowResized(id, s)),
                    window::position(id).map(move |p| Message::DetailWindow(WinMsg::Placed(id, p))),
                    frame::enable_snap_layout(id),
                ]);
                let then = if self.ui.detail_pinned {
                    Task::batch([size, self.apply_pin()])
                } else {
                    size
                };
                self.show_detail_window(id).chain(then)
            }
            WinMsg::Saved(p) => {
                self.remember_placement(p);
                Task::none()
            }
            WinMsg::SavedThenExit(p) => {
                self.remember_placement(p);
                iced::exit()
            }
            WinMsg::Placed(id, p) => {
                if Some(id) == self.detail_window
                    && let Some(p) = p
                {
                    self.on_detail_window_moved(p);
                }
                Task::none()
            }
            WinMsg::Owner(app, owner) => {
                crate::script::log(&format!(
                    "detail window owner: {owner:?} (app window {app:?})"
                ));
                self.detail_owner = Some((app, owner));
                Task::none()
            }
        }
    }

    /// Windows: place the hidden window from `ui.json` (clamped onto a monitor), show it and
    /// maximise it if it was. Elsewhere it is already shown where iced put it.
    fn show_detail_window(&self, id: window::Id) -> Task<Message> {
        if !cfg!(windows) {
            return Task::none();
        }
        let saved = saved_placement(self.ui.detail_window);
        let place = match saved {
            Some(p) => window::run(id, move |w| {
                let used = frame::hwnd_of(w).and_then(|h| meninx_web::place_window(h, &p));
                crate::script::log(&format!("detail window: placed {used:?} (saved {p:?})"));
            })
            .discard(),
            None => Task::none(),
        };
        let maximize = if saved.is_some_and(|p| p.maximized) {
            window::maximize(id, true)
        } else {
            Task::none()
        };
        place
            .chain(window::set_mode(id, window::Mode::Windowed))
            .chain(maximize)
    }

    /// Keep a placement read from the window (Windows) in `ui.json`.
    fn remember_placement(&mut self, p: Option<meninx_web::Placement>) {
        let Some(p) = p else { return };
        let g = WindowGeom {
            x: p.x as f32,
            y: p.y as f32,
            width: p.width as f32,
            height: p.height as f32,
            dpi: Some(p.dpi),
            maximized: p.maximized,
        };
        if self.ui.detail_window != Some(g) {
            crate::script::log(&format!("detail window: saved place {p:?}"));
            self.ui.detail_window = Some(g);
            // Not `save_ui`: this can arrive after `close_for_reopen` reloaded the state, whose
            // `selected` is still unset until the session list loads.
            self.ui.save(&self.data_dir);
        }
    }

    /// Quit the app, remembering the detail window's place first (Windows).
    pub(crate) fn exit_after_saving_detail(&self) -> Task<Message> {
        match self.detail_window {
            Some(id) if cfg!(windows) => read_placement(id, WinMsg::SavedThenExit),
            _ => iced::exit(),
        }
    }

    /// The detail window was resized: remember its size, then ask whether it is maximised
    /// (the title bar's maximise / restore glyph and the resize grips follow it).
    pub(crate) fn on_detail_window_resized(&mut self, id: window::Id, size: Size) -> Task<Message> {
        self.detail_window_size = size;
        self.remember_detail_geometry();
        frame::query_maximized(id)
    }

    /// Make the detail window owned by the app window (pinned) or a top-level window again.
    fn apply_pin(&self) -> Task<Message> {
        let (Some(main), Some(detail)) = (self.window, self.detail_window) else {
            return Task::none();
        };
        let pinned = self.ui.detail_pinned;
        // Owned windows are a Windows thing: elsewhere the pin keeps the window on top.
        if !cfg!(windows) {
            let _ = main;
            let level = if pinned {
                window::Level::AlwaysOnTop
            } else {
                window::Level::Normal
            };
            return window::set_level(detail, level);
        }
        window::run(main, frame::hwnd_of).then(move |app| {
            window::run(detail, move |w| {
                let child = frame::hwnd_of(w)?;
                meninx_web::set_window_owner(child, if pinned { app } else { None });
                meninx_web::window_owner(child)
            })
            .map(move |owner| Message::DetailWindow(WinMsg::Owner(app, owner)))
        })
    }

    /// The window's own close button: the area hides and stays detached (Desk: the window is
    /// the area itself).
    pub(crate) fn on_detail_window_close(&mut self) -> Task<Message> {
        crate::script::log("detail window: close button (hide, stays detached)");
        self.ui.detail_collapsed = true;
        self.open_items = false;
        self.save_ui();
        self.sync_web();
        self.sync_detail()
    }

    /// A window is gone. The detail window closing by itself (not through the app) counts as
    /// hiding it.
    pub(crate) fn on_window_closed(&mut self, id: window::Id) -> Task<Message> {
        if Some(id) != self.detail_window {
            return Task::none();
        }
        self.detail_window = None;
        self.detail_window_ready = false;
        self.on_detail_window_close()
    }

    /// Remember the window's place for the next opening (saved with `ui.json`), once both its
    /// position and size are known; not while maximised or minimised. Windows reads the
    /// placement from the OS instead (`remember_placement`).
    fn remember_detail_geometry(&mut self) {
        if cfg!(windows) || self.detail_window_maximized {
            return;
        }
        if let Some(p) = self.detail_window_pos
            && p.x > -10_000.0
            && p.y > -10_000.0
        {
            let s = self.detail_window_size;
            self.ui.detail_window = Some(WindowGeom {
                x: p.x,
                y: p.y,
                width: s.width,
                height: s.height,
                dpi: None,
                maximized: false,
            });
        }
    }

    pub(crate) fn on_detail_window_moved(&mut self, p: Point) {
        self.detail_window_pos = Some(p);
        self.remember_detail_geometry();
    }

    /// The view of a window: the app window or the detail window.
    pub fn view_window(&self, window: window::Id) -> El<'_> {
        if Some(window) == self.detail_window {
            self.detail_window_view()
        } else {
            self.view()
        }
    }

    /// The detail window: a title bar drawn by the app (Desk's region title bar: title, pin,
    /// 앱으로 합치기, then the caption buttons) above the detail pane of the selected session,
    /// with resize grips along the edges while not maximised.
    fn detail_window_view(&self) -> El<'_> {
        let pinned = self.ui.detail_pinned;
        let pin = button(
            container(icons::icon(
                if pinned { Icon::PinFilled } else { Icon::Pin },
                15.0,
                if pinned {
                    style::ACCENT_TEXT
                } else {
                    style::DIM
                },
            ))
            .center(28),
        )
        .width(28)
        .height(28)
        .padding(0)
        .style(style::icon_toggle(pinned))
        .on_press(Message::DetailWindow(WinMsg::TogglePin));
        let pin_tip = if pinned {
            tr::detail_window_unpin()
        } else {
            tr::detail_window_pin()
        };
        let merge = button(
            row![
                icons::icon(Icon::ArrowBarToLeft, 14.0, style::DIM),
                text(tr::detail_window_merge()).size(12),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .style(style::icon)
        .on_press(Message::DetailWindow(WinMsg::Merge));
        let Some(id) = self.detail_window else {
            return text("").boxed();
        };
        let maximized = self.detail_window_maximized;
        let actions = row![
            tip(pin.boxed(), pin_tip),
            tip(merge.boxed(), tr::detail_window_merge_tip()),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        let body: El<'_> = match self.shown_live() {
            Some(live) => self.pane(live, true),
            None => container(text(tr::no_session()).size(13).color(style::DIM))
                .center(Fill)
                .boxed(),
        };
        let content = container(column![
            frame::title_bar(id, self.detail_window_label(), actions.boxed(), maximized),
            body,
        ])
        .width(Fill)
        .height(Fill)
        .style(style::bg(style::PANEL));
        // The item menu is placed in window coordinates: a layer over the whole window.
        let window = iced::widget::stack![
            content,
            self.toast_layer(true),
            self.item_menu_layer(true),
            self.modal_layer(true)
        ]
            .width(Fill)
            .height(Fill);
        frame::with_grips(id, window.boxed(), maximized)
    }
}
