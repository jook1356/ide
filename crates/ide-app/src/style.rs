//! Colours and widget styles of the dark IDE look.

use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Font, Shadow, Theme, color};

pub const BG: Color = color!(0x1e1f22);
pub const SIDEBAR: Color = color!(0x18191c);
pub const PANEL: Color = color!(0x202226);
pub const HEADER: Color = color!(0x26282d);
pub const BORDER: Color = color!(0x34363c);
pub const HOVER: Color = color!(0x2c2f35);
pub const SELECTED: Color = color!(0x323a4a);
pub const TEXT: Color = color!(0xd4d6db);
pub const DIM: Color = color!(0x8b8f98);
pub const FAINT: Color = color!(0x5d616a);
pub const ACCENT: Color = color!(0x4c8dff);
pub const GREEN: Color = color!(0x3fb950);
pub const AMBER: Color = color!(0xd29922);
pub const RED: Color = color!(0xf85149);

// Navigation rail, from Hermes Desk's dark theme (its retuned Mantine `dark` scale and the solid
// "violet light" accent): ground dark-9, border dark-6, hover dark-5, icons dark-0.
pub const RAIL: Color = color!(0x0c0d10);
pub const RAIL_BORDER: Color = color!(0x272930);
pub const RAIL_HOVER: Color = color!(0x30323a);
pub const RAIL_ICON: Color = color!(0xe6e7ec);
/// `--mantine-color-violet-light` (oklch of violet-8 at L 0.37, 60 % chroma).
pub const RAIL_ACTIVE: Color = color!(0x3f2e7f);
/// `--mantine-color-violet-light-color` (violet-2).
pub const RAIL_ACTIVE_ICON: Color = color!(0xd0bfff);

pub const BOLD: Font = Font {
    weight: iced::font::Weight::Bold,
    ..Font::DEFAULT
};
pub const MONO: Font = Font::MONOSPACE;

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

/// A 44 x 44 navigation rail button: hover tint, the active page in violet.
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

/// Accent bar on the rail's left edge beside the active page (Hermes Desk's `--accent-gradient`:
/// 135deg, violet-5 to #6d8bff; rounded on the right).
pub fn rail_accent(_: &Theme) -> container::Style {
    let gradient = iced::gradient::Linear::new(iced::Radians(135f32.to_radians()))
        .add_stop(0.0, color!(0x9775fa))
        .add_stop(1.0, color!(0x6d8bff));
    container::Style {
        background: Some(Background::Gradient(gradient.into())),
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

/// Tooltip of the navigation rail (Mantine's dark-scheme tooltip: light grey, dark text).
pub fn rail_tip(_: &Theme) -> container::Style {
    container::Style {
        background: Some(color!(0xe9ecef).into()),
        text_color: Some(Color::BLACK),
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
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
        button::Status::Hovered => flat(Some(color!(0x5d9bff)), Color::WHITE),
        button::Status::Disabled => flat(Some(color!(0x2b3b5c)), DIM),
        _ => flat(Some(ACCENT), Color::WHITE),
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
    s.background = Background::Color(color!(0x16171a));
    s.border.radius = 4.0.into();
    s.border.color = match status {
        text_input::Status::Focused { .. } => ACCENT,
        _ => BORDER,
    };
    s
}
