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

use iced::widget::{button, column, container, row, space, text};
use iced::window::raw_window_handle::RawWindowHandle;
use iced::{Alignment, Element, Fill, Point, Size, Task, Widget as _, window};

use crate::app::{App, Message};
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::style;
use crate::ui_state::WindowGeom;
use crate::view::tip;

type El<'a> = Element<'a, Message>;

/// Desk's region window: 960 x 640 at first, at least 420 x 240.
const DEFAULT_SIZE: Size = Size::new(960.0, 640.0);
const MIN_SIZE: Size = Size::new(420.0, 240.0);
/// Height of the window's bar (pin, 앱으로 합치기).
const BAR_H: f32 = 36.0;

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
}

/// The HWND of a window (`None` off Windows).
fn hwnd_of(w: &dyn window::Window) -> Option<isize> {
    match w.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

impl App {
    /// The detail pane is drawn in the app window (shown and not detached).
    pub fn detail_in_app(&self) -> bool {
        !self.ui.detail_collapsed && !self.ui.detail_detached
    }

    /// The window's title: Desk's `상세 · <session>` (`상세 보기` without a session) plus the
    /// app's name.
    pub fn detail_window_title(&self) -> String {
        let t = match self.selected_live() {
            Some(l) => tr::detail_window_title(&l.info.title),
            None => tr::detail_window_title_idle(),
        };
        format!("{t} — ide")
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
                window::close(id)
            }
            _ => Task::none(),
        }
    }

    fn open_detail_window(&mut self) -> Task<Message> {
        let geom = self.ui.detail_window;
        let size = geom
            .map(|g| Size::new(g.width.max(MIN_SIZE.width), g.height.max(MIN_SIZE.height)))
            .unwrap_or(DEFAULT_SIZE);
        let position = match geom {
            Some(g) => window::Position::Specific(Point::new(g.x, g.y)),
            None => window::Position::Default,
        };
        let (id, open) = window::open(window::Settings {
            size,
            position,
            min_size: Some(MIN_SIZE),
            // Its close button hides the area (it stays detached): handled by the app.
            exit_on_close_request: false,
            ..window::Settings::default()
        });
        crate::script::log(&format!("detail window: open {size:?} at {position:?}"));
        self.detail_window = Some(id);
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
                ]);
                if self.ui.detail_pinned {
                    Task::batch([size, self.apply_pin()])
                } else {
                    size
                }
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

    /// Make the detail window owned by the app window (pinned) or a top-level window again.
    fn apply_pin(&self) -> Task<Message> {
        let (Some(main), Some(detail)) = (self.window, self.detail_window) else {
            return Task::none();
        };
        let pinned = self.ui.detail_pinned;
        window::run(main, hwnd_of).then(move |app| {
            window::run(detail, move |w| {
                let child = hwnd_of(w)?;
                ide_web::set_window_owner(child, if pinned { app } else { None });
                ide_web::window_owner(child)
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
    /// position and size are known.
    fn remember_detail_geometry(&mut self) {
        if let Some(p) = self.detail_window_pos {
            let s = self.detail_window_size;
            self.ui.detail_window = Some(WindowGeom {
                x: p.x,
                y: p.y,
                width: s.width,
                height: s.height,
            });
        }
    }

    pub(crate) fn on_detail_window_resized(&mut self, size: Size) {
        self.detail_window_size = size;
        self.remember_detail_geometry();
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

    /// The detail window: Desk's region title bar actions (pin, 앱으로 합치기) above the detail
    /// pane of the selected session.
    fn detail_window_view(&self) -> El<'_> {
        let pinned = self.ui.detail_pinned;
        let pin = button(
            container(icons::icon(
                if pinned { Icon::PinFilled } else { Icon::Pin },
                15.0,
                if pinned {
                    style::RAIL_ACTIVE_ICON
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
        let bar = container(
            row![
                space().width(Fill),
                tip(pin.boxed(), pin_tip),
                tip(merge.boxed(), tr::detail_window_merge_tip()),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .height(BAR_H)
        .padding([0, 8])
        .width(Fill)
        .align_y(Alignment::Center)
        .style(style::bg(style::RAIL));
        let body: El<'_> = match self.selected_live() {
            Some(live) => self.pane(live, true),
            None => container(text(tr::no_session()).size(13).color(style::DIM))
                .center(Fill)
                .boxed(),
        };
        container(column![
            bar,
            container(space())
                .height(1)
                .width(Fill)
                .style(style::bg(style::BORDER)),
            body,
        ])
        .width(Fill)
        .height(Fill)
        .style(style::bg(style::PANEL))
        .boxed()
    }
}
