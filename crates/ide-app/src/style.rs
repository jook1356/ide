//! Colours and widget styles of the dark IDE look.

use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Shadow, Theme, color};

pub const BG: Color = color!(0x1e1f22);
pub const SIDEBAR: Color = color!(0x18191c);
pub const PANEL: Color = color!(0x202226);
pub const HEADER: Color = color!(0x26282d);
pub const BORDER: Color = color!(0x34363c);
/// Ground of text inputs.
pub const INPUT: Color = color!(0x16171a);
pub const HOVER: Color = color!(0x2c2f35);
/// Background of a selected / active item (session row, rail page, a toggle that is on).
pub const SELECTED: Color = color!(0x323a4a);
pub const TEXT: Color = color!(0xd4d6db);
pub const DIM: Color = color!(0x8b8f98);
pub const FAINT: Color = color!(0x5d616a);
pub const GREEN: Color = color!(0x3fb950);
pub const AMBER: Color = color!(0xd29922);
pub const RED: Color = color!(0xf85149);
/// Category colours (Mantine orange-6 / yellow-6 / teal-5): the detail rail's groups, footer
/// strips and notices.
pub const ORANGE: Color = color!(0xfd7e14);
pub const YELLOW: Color = color!(0xfab005);
pub const TEAL: Color = color!(0x20c997);
/// Branch badge colours of 커밋 보유 (Mantine cyan-5) and 머지됨 (GitHub's merged purple).
pub const CYAN: Color = color!(0x22b8cf);
pub const PURPLE: Color = color!(0xa371f7);

// Accent: the one highlight hue of the app (a theme swaps these).
/// Solid accent: primary buttons, focus rings, the rail's active bar, badges.
pub const ACCENT: Color = color!(0x4c8dff);
pub const ACCENT_HOVER: Color = color!(0x5d9bff);
pub const ACCENT_DISABLED: Color = color!(0x2b3b5c);
/// Accent for text and icons on dark grounds (active rail icon, links, accent buttons).
pub const ACCENT_TEXT: Color = color!(0x8ab4ff);
/// Text on a solid accent ground.
pub const ON_ACCENT: Color = Color::WHITE;

/// The accent at `alpha` (translucent fills such as Mantine's `variant="light"` buttons).
pub const fn accent_alpha(alpha: f32) -> Color {
    Color { a: alpha, ..ACCENT }
}

// Navigation rail: same palette as the app (sidebar ground, app border/hover/text).
pub const RAIL: Color = SIDEBAR;
pub const RAIL_BORDER: Color = BORDER;
pub const RAIL_HOVER: Color = HOVER;
pub const RAIL_ICON: Color = TEXT;
pub const RAIL_ACTIVE: Color = SELECTED;
pub const RAIL_ACTIVE_ICON: Color = ACCENT_TEXT;

pub use ide_views::theme::{bold, mono, ui};

pub fn theme() -> Theme {
    Theme::custom(
        "ide dark",
        iced::theme::palette::Seed {
            background: BG,
            text: TEXT,
            primary: ACCENT,
            success: GREEN,
            warning: AMBER,
            danger: RED,
        },
    )
}

pub fn bg(c: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(c.into()),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

pub fn modal_card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(PANEL.into()),
        text_color: Some(TEXT),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: iced::Vector::new(0.0, 8.0),
            blur_radius: 24.0,
        },
        ..Default::default()
    }
}

pub fn backdrop(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.55).into()),
        ..Default::default()
    }
}

pub fn error_box(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba8(0xf8, 0x51, 0x49, 0.12).into()),
        text_color: Some(color!(0xff9a94)),
        border: Border {
            color: Color::from_rgba8(0xf8, 0x51, 0x49, 0.5),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

/// A warning (Desk's yellow `Alert`, e.g. "원본 레포에서 바로 작업해요").
pub fn warn_box(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba8(0xd2, 0x99, 0x22, 0.12).into()),
        text_color: Some(color!(0xe3b341)),
        border: Border {
            color: Color::from_rgba8(0xd2, 0x99, 0x22, 0.5),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

fn flat(background: Option<Color>, text: Color) -> button::Style {
    button::Style {
        background: background.map(Background::Color),
        text_color: text,
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Transparent icon button, highlighted on hover.
pub fn icon(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Active => flat(None, DIM),
        button::Status::Hovered => flat(Some(HOVER), TEXT),
        button::Status::Pressed => flat(Some(SELECTED), TEXT),
        button::Status::Disabled => flat(None, FAINT),
    }
}

/// Icon button that shows a setting is on (selected ground, accent icon).
pub fn icon_toggle(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let mut s = if on {
            flat(Some(SELECTED), ACCENT_TEXT)
        } else {
            icon(theme, status)
        };
        s.border.radius = 4.0.into();
        s
    }
}

/// Session list group divider: no background, a faint tint on hover.
pub fn divider(_: &Theme, status: button::Status) -> button::Style {
    let mut s = match status {
        button::Status::Hovered | button::Status::Pressed => flat(Some(HOVER), TEXT),
        _ => flat(None, DIM),
    };
    s.border.radius = 4.0.into();
    s
}

/// A 44 x 44 navigation rail button: hover tint, the active page selected like a session row.
pub fn rail_link(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = match (active, status) {
            (true, _) => Some(RAIL_ACTIVE),
            (false, button::Status::Hovered | button::Status::Pressed) => Some(RAIL_HOVER),
            _ => None,
        };
        let mut s = flat(bg, RAIL_ICON);
        s.border.radius = 8.0.into();
        s
    }
}

/// Accent bar on the rail's left edge beside the active page (rounded on the right).
pub fn rail_accent(_: &Theme) -> container::Style {
    container::Style {
        background: Some(ACCENT.into()),
        border: Border {
            radius: iced::border::Radius {
                top_left: 0.0,
                top_right: 3.0,
                bottom_right: 3.0,
                bottom_left: 0.0,
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Tooltip of the navigation rail (dark card matching the app theme).
pub fn rail_tip(_: &Theme) -> container::Style {
    container::Style {
        background: Some(HEADER.into()),
        text_color: Some(TEXT),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

/// Small round status dot.
pub fn dot(c: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(c.into()),
        border: Border {
            radius: 100.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A section card of a page (bordered, rounded).
pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(PANEL.into()),
        text_color: Some(TEXT),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

/// Sidebar session row (selected rows keep a highlight).
pub fn row(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = match (selected, status) {
            (true, _) => Some(SELECTED),
            (false, button::Status::Hovered | button::Status::Pressed) => Some(HOVER),
            _ => None,
        };
        flat(bg, TEXT)
    }
}

/// Toggle-like choice button (agent kind, tabs).
pub fn choice(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let mut s = match (active, status) {
            (true, _) => flat(Some(SELECTED), TEXT),
            (false, button::Status::Hovered) => flat(Some(HOVER), TEXT),
            _ => flat(Some(HEADER), DIM),
        };
        if active {
            s.border = Border {
                color: ACCENT,
                width: 1.0,
                radius: 4.0.into(),
            };
        }
        s
    }
}

/// Side panel tab (the active one is underlined by the caller).
pub fn tab_flat(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let mut s = match status {
            button::Status::Hovered | button::Status::Pressed => flat(Some(HOVER), TEXT),
            _ => flat(None, if active { TEXT } else { DIM }),
        };
        s.border.radius = 0.0.into();
        s
    }
}

pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered => flat(Some(ACCENT_HOVER), ON_ACCENT),
        button::Status::Disabled => flat(Some(ACCENT_DISABLED), DIM),
        _ => flat(Some(ACCENT), ON_ACCENT),
    }
}

pub fn danger(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered => flat(Some(color!(0xff6a63)), Color::WHITE),
        button::Status::Disabled => flat(Some(color!(0x5c2b2b)), DIM),
        _ => flat(Some(color!(0xd83b34)), Color::WHITE),
    }
}

pub fn secondary(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered => flat(Some(color!(0x3a3d44)), TEXT),
        _ => flat(Some(HEADER), TEXT),
    }
}

pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let mut s = text_input::default(theme, status);
    s.background = Background::Color(INPUT);
    s.border.radius = 4.0.into();
    s.border.color = match status {
        text_input::Status::Focused { .. } => ACCENT,
        _ => BORDER,
    };
    s
}
