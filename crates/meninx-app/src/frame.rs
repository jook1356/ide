//! Frameless windows (Hermes Desk's `frame: false` windows): the app draws each window's title
//! bar (drag, double-click to maximise, Windows 11-style caption buttons) and thin resize grips
//! along its edges. Used by the app window and the detail window.

use std::sync::LazyLock;

use iced::widget::{button, column, container, mouse_area, row, space, stack, svg, text};
use iced::{
    Alignment, Border, Element, Fill, Length, Shadow, Task, Theme, Widget as _, color, mouse,
    window,
};

use crate::app::{App, Message};
use crate::i18n::tr;
use crate::style;
use crate::view::tip;

type El<'a> = Element<'a, Message>;

/// Height of a title bar.
pub const BAR_H: f32 = 36.0;
/// Width of a caption button (minimise / maximise / close), as in Windows 11's title bar.
const CAPTION_W: f32 = 46.0;
/// Thickness of the resize grips along a window's edges.
const GRIP: f32 = 5.0;

/// Windows 11 caption glyphs (10 x 10, 1 px strokes); the colour comes from the style.
const GLYPHS: [&str; 4] = [
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><path d="M0 5.5h10" fill="none" stroke="#fff"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect x=".5" y=".5" width="9" height="9" fill="none" stroke="#fff"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect x=".5" y="2.5" width="7" height="7" fill="none" stroke="#fff"/><path d="M2.5 2.5v-2h7v7h-2" fill="none" stroke="#fff"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><path d="M.5.5l9 9m0-9l-9 9" fill="none" stroke="#fff"/></svg>"##,
];

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

static GLYPH_HANDLES: LazyLock<Vec<svg::Handle>> = LazyLock::new(|| {
    GLYPHS
        .iter()
        .map(|s| svg::Handle::from_memory(s.as_bytes()))
        .collect()
});

/// A title bar or resize grip action on one window.
#[derive(Debug, Clone, Copy)]
pub enum FrameMsg {
    /// Press on the title bar: move the window (the OS move loop, so Aero snap works).
    Drag,
    /// Press on a resize grip.
    Resize(window::Direction),
    Minimize,
    /// Maximise / restore button or a double click on the title bar.
    ToggleMaximize,
    /// The window's maximised state, asked after each resize.
    Maximized(bool),
    /// The app window's minimised state, asked after each of its resizes.
    Minimized(Option<bool>),
}

/// Windows restores owned windows un-maximised: the pinned detail window is maximised again
/// this long after the app window comes back (Desk waits the same).
const REMAXIMIZE_AFTER: std::time::Duration = std::time::Duration::from_millis(50);

/// macOS keeps its own window buttons (the traffic lights, over the app's title bar); elsewhere
/// the app draws minimise / maximise / close.
const NATIVE_BUTTONS: bool = cfg!(target_os = "macos");
/// Room left of the title for macOS' traffic lights.
const TRAFFIC_LIGHTS_W: f32 = 78.0;

/// `settings` without the OS title bar: Windows and Linux lose the frame (DWM's shadow and
/// rounded corners are kept on Windows so the window still reads as a window); macOS keeps its
/// frame and traffic lights with the title bar transparent and the content drawn under it.
pub fn frameless(settings: window::Settings) -> window::Settings {
    #[cfg(target_os = "macos")]
    return window::Settings {
        platform_specific: window::settings::PlatformSpecific {
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        },
        ..settings
    };
    #[cfg(not(target_os = "macos"))]
    window::Settings {
        decorations: false,
        #[cfg(windows)]
        platform_specific: window::settings::PlatformSpecific {
            undecorated_shadow: true,
            corner_preference: window::settings::platform::CornerPreference::Round,
            ..settings.platform_specific
        },
        ..settings
    }
}

impl App {
    pub(crate) fn on_frame(&mut self, id: window::Id, m: FrameMsg) -> Task<Message> {
        match m {
            FrameMsg::Drag => window::drag(id),
            FrameMsg::Resize(d) => window::drag_resize(id, d),
            FrameMsg::Minimize => window::minimize(id, true),
            FrameMsg::ToggleMaximize => window::toggle_maximize(id),
            FrameMsg::Maximized(m) => {
                if Some(id) == self.detail_window {
                    self.detail_window_maximized = m;
                } else if Some(id) == self.window {
                    self.window_maximized = m;
                }
                Task::none()
            }
            FrameMsg::Minimized(m) => {
                let now = m.unwrap_or(false);
                if Some(id) != self.window || now == self.window_minimized {
                    return Task::none();
                }
                self.window_minimized = now;
                if now {
                    // Remembered as the app goes away (the owned window's own state changes next).
                    self.remaximize_detail = self.ui.detail_pinned && self.detail_window_maximized;
                    return Task::none();
                }
                match (std::mem::take(&mut self.remaximize_detail), self.detail_window) {
                    (true, Some(detail)) => {
                        Task::perform(tokio::time::sleep(REMAXIMIZE_AFTER), |()| ())
                            .then(move |()| window::maximize(detail, true))
                    }
                    _ => Task::none(),
                }
            }
        }
    }

    /// Ask whether the app window is minimised (after its resizes).
    pub(crate) fn query_minimized(id: window::Id) -> Task<Message> {
        window::is_minimized(id).map(move |m| Message::Frame(id, FrameMsg::Minimized(m)))
    }
}

/// The HWND of a window (`None` off Windows).
pub fn hwnd_of(w: &dyn window::Window) -> Option<isize> {
    use window::raw_window_handle::RawWindowHandle;
    match w.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// Windows 11 snap layouts when the pointer rests on window `id`'s maximise button (see
/// `meninx_web::enable_snap_layout`; the button's click is then handled by that hook).
pub fn enable_snap_layout(id: window::Id) -> Task<Message> {
    window::run(id, |w| {
        if let Some(h) = hwnd_of(w) {
            meninx_web::enable_snap_layout(h, BAR_H, CAPTION_W);
        }
    })
    .discard()
}

/// Ask whether window `id` is maximised (after a resize): the title bar's maximise / restore
/// glyph and the resize grips follow it.
pub fn query_maximized(id: window::Id) -> Task<Message> {
    window::is_maximized(id).map(move |m| Message::Frame(id, FrameMsg::Maximized(m)))
}

/// The title bar of window `id`: `label` on the left, then `lead` (the app window's server menu
/// button), the window's own `actions`, then the
/// caption buttons; a 1 px border below. The bar drags the window (its buttons take their own
/// presses first). The close button sends the same close request as the OS one did.
pub fn title_bar<'a>(
    id: window::Id,
    label: String,
    lead: El<'a>,
    actions: El<'a>,
    maximized: bool,
) -> El<'a> {
    let title = text(label)
        .size(12)
        .color(style::DIM)
        .wrapping(text::Wrapping::None)
        .ellipsis(text::Ellipsis::End);
    let frame = move |m| Message::Frame(id, m);
    let captions = row![
        caption(
            Glyph::Minimize,
            false,
            frame(FrameMsg::Minimize),
            tr::window_minimize()
        ),
        if maximized {
            caption(
                Glyph::Restore,
                false,
                frame(FrameMsg::ToggleMaximize),
                tr::window_restore(),
            )
        } else {
            caption(
                Glyph::Maximize,
                false,
                frame(FrameMsg::ToggleMaximize),
                tr::window_maximize(),
            )
        },
        caption(
            Glyph::Close,
            true,
            Message::WindowCloseRequest(id),
            tr::close()
        ),
    ];
    let bar = mouse_area(
        container(
            row![
                space().width(if NATIVE_BUTTONS { TRAFFIC_LIGHTS_W } else { 0.0 }),
                container(title).padding([0, 12]),
                lead,
                space().width(Fill),
                actions,
                space().width(8),
                if NATIVE_BUTTONS { space().boxed() } else { captions.boxed() },
            ]
            .spacing(4)
            .height(Fill)
            .align_y(Alignment::Center),
        )
        .height(BAR_H)
        .width(Fill)
        .style(style::bg(style::RAIL)),
    )
    .on_press(frame(FrameMsg::Drag))
    .on_double_click(frame(FrameMsg::ToggleMaximize));
    column![
        bar,
        container(space())
            .height(1)
            .width(Fill)
            .style(style::bg(style::BORDER)),
    ]
    .boxed()
}

/// `content` with invisible resize grips along the edges of window `id` (none while maximised).
/// Always a stack, so maximising keeps the content's widget tree (terminal, web views).
pub fn with_grips<'a>(id: window::Id, content: El<'a>, maximized: bool) -> El<'a> {
    // macOS resizes from its own frame.
    if maximized || NATIVE_BUTTONS {
        return stack![content, space()].width(Fill).height(Fill).boxed();
    }
    use window::Direction as D;
    let g = |d, w: Length, h: Length| grip(id, d, w, h);
    let grips = column![
        row![
            g(D::NorthWest, GRIP.into(), GRIP.into()),
            g(D::North, Fill, GRIP.into()),
            g(D::NorthEast, GRIP.into(), GRIP.into()),
        ],
        row![
            g(D::West, GRIP.into(), Fill),
            space().width(Fill).height(Fill),
            g(D::East, GRIP.into(), Fill),
        ]
        .height(Fill),
        row![
            g(D::SouthWest, GRIP.into(), GRIP.into()),
            g(D::South, Fill, GRIP.into()),
            g(D::SouthEast, GRIP.into(), GRIP.into()),
        ],
    ];
    stack![content, grips].width(Fill).height(Fill).boxed()
}

/// A caption button (Windows 11 look: flat, red behind the close glyph).
fn caption<'a>(glyph: Glyph, close: bool, msg: Message, label: String) -> El<'a> {
    let b = button(
        container(
            svg(GLYPH_HANDLES[glyph as usize].clone())
                .width(10)
                .height(10)
                .style(|_, _| svg::Style {
                    color: Some(style::TEXT),
                }),
        )
        .center(Fill),
    )
    .width(CAPTION_W)
    .height(Fill)
    .padding(0)
    .style(move |_: &Theme, status| {
        let bg = match (status, close) {
            (button::Status::Hovered, true) => Some(color!(0xc42b1c)),
            (button::Status::Pressed, true) => Some(color!(0xa9271a)),
            (button::Status::Hovered, false) => Some(style::HOVER),
            (button::Status::Pressed, false) => Some(style::SELECTED),
            _ => None,
        };
        button::Style {
            background: bg.map(Into::into),
            text_color: style::TEXT,
            border: Border::default(),
            shadow: Shadow::default(),
            snap: true,
        }
    })
    .on_press(msg);
    tip(b.boxed(), label)
}

/// An invisible strip along an edge (or a corner) that resizes the window when pressed.
fn grip<'a>(id: window::Id, d: window::Direction, width: Length, height: Length) -> El<'a> {
    use window::Direction as D;
    let cursor = match d {
        D::North | D::South => mouse::Interaction::ResizingVertically,
        D::East | D::West => mouse::Interaction::ResizingHorizontally,
        D::NorthWest | D::SouthEast => mouse::Interaction::ResizingDiagonallyDown,
        D::NorthEast | D::SouthWest => mouse::Interaction::ResizingDiagonallyUp,
    };
    mouse_area(space().width(width).height(height))
        .interaction(cursor)
        .on_press(Message::Frame(id, FrameMsg::Resize(d)))
        .boxed()
}
