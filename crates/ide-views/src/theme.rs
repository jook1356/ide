//! Colours, sizes and widget styles shared by the views (same dark palette as `ide-app`'s
//! `style.rs`; diff / syntax colours follow Hermes Desk's GitHub-dark look).

use iced::widget::{button, container, text_input};
use iced::font::Family;
use iced::{Background, Border, Color, Font, Shadow, Theme, color};

pub const BG: Color = color!(0x1e1f22);
pub const PANEL: Color = color!(0x202226);
pub const HEADER: Color = color!(0x26282d);
pub const BORDER: Color = color!(0x34363c);
pub const HOVER: Color = color!(0x2c2f35);
pub const SELECTED: Color = color!(0x323a4a);
pub const TEXT: Color = color!(0xd4d6db);
pub const DIM: Color = color!(0x8b8f98);
pub const FAINT: Color = color!(0x5d616a);
pub const ACCENT: Color = color!(0x4c8dff);
pub const INPUT_BG: Color = color!(0x16171a);

/// Git status colours (Mantine yellow/teal/red/blue/orange 4, as Hermes Desk's tree rows).
pub const GIT_MODIFIED: Color = color!(0xffd43b);
pub const GIT_ADDED: Color = color!(0x38d9a9);
pub const GIT_DELETED: Color = color!(0xff8787);
pub const GIT_RENAMED: Color = color!(0x74c0fc);
pub const GIT_CONFLICT: Color = color!(0xff922b);

/// Code text (GitHub dark default).
pub const CODE_TEXT: Color = color!(0xe6edf3);
pub const LINE_NO: Color = color!(0x6e7681);
pub const LINE_NO_ACTIVE: Color = color!(0xe6edf3);
pub const ACTIVE_LINE: Color = over(color!(0x6e7681), 0.10, BG);
pub const FIND_MATCH: Color = over(color!(0xfab005), 0.22, BG);
pub const FIND_MATCH_BORDER: Color = over(color!(0xfab005), 0.45, BG);
pub const FIND_CURRENT: Color = over(color!(0xfab005), 0.45, BG);
pub const SELECTION: Color = Color::from_rgba(0.22, 0.55, 0.99, 0.30);
/// The code editor's scrollbar thumb: idle, hovered, dragged.
pub const SCROLL_THUMB: Color = Color { a: 0.30, ..DIM };
pub const SCROLL_THUMB_HOVER: Color = Color { a: 0.50, ..DIM };
pub const SCROLL_THUMB_DRAG: Color = Color { a: 0.65, ..DIM };

/// `fg` with opacity `a` painted over `bg`, blended in sRGB like the browser does. iced blends
/// translucent colours in linear space, which makes low-alpha tints look much stronger than in
/// Hermes Desk, so the views use these precomputed opaque colours instead.
pub const fn over(fg: Color, a: f32, bg: Color) -> Color {
    Color {
        r: fg.r * a + bg.r * (1.0 - a),
        g: fg.g * a + bg.g * (1.0 - a),
        b: fg.b * a + bg.b * (1.0 - a),
        a: 1.0,
    }
}

const DIFF_GREEN: Color = color!(0x2ea043);
const DIFF_RED: Color = color!(0xf85149);
const DIFF_BLUE: Color = color!(0x388bfd);

/// Diff rows (Hermes Desk `.diff-add` / `.diff-del` / `.diff-hunk`; number cells and word
/// highlights are layered over the row tint as in its table).
pub const ADD_BG: Color = over(DIFF_GREEN, 0.15, BG);
pub const ADD_NO_BG: Color = over(DIFF_GREEN, 0.22, ADD_BG);
pub const ADD_WORD_BG: Color = over(DIFF_GREEN, 0.40, ADD_BG);
pub const DEL_BG: Color = over(DIFF_RED, 0.15, BG);
pub const DEL_NO_BG: Color = over(DIFF_RED, 0.22, DEL_BG);
pub const DEL_WORD_BG: Color = over(DIFF_RED, 0.40, DEL_BG);
pub const HUNK_BG: Color = over(DIFF_BLUE, 0.10, BG);
pub const HUNK_HOVER_BG: Color = over(DIFF_BLUE, 0.22, BG);
pub const HUNK_TEXT: Color = color!(0x74c0fc);
pub const FILLER_BG: Color = over(color!(0x808080), 0.05, BG);
pub const STAT_ADD: Color = color!(0x38d9a9);
pub const STAT_DEL: Color = color!(0xff8787);

// ------------------------------------------------------------------ fonts
//
// The user picks the UI font and the code font in Settings (`ide-app`'s `ui.json`); empty = the
// defaults below. The UI font is read once at start (iced's renderer default font, used by every
// text without `.font(..)`, is fixed when the app starts); the code font applies at once.

/// Default code family. On Windows this names Consolas instead of the generic monospace family:
/// with the generic family, cosmic-text falls back for Hangul to the first *monospaced* face that
/// has it (GungsuhChe / BatangChe / GulimChe, serif-looking bitmap-era faces), while a named family
/// uses the script fallback (Malgun Gothic), the same face the UI text and the terminal use.
#[cfg(windows)]
const DEFAULT_CODE_FAMILY: Family = Family::Name("Consolas");
#[cfg(not(windows))]
const DEFAULT_CODE_FAMILY: Family = Family::Monospace;

static UI_FAMILY: std::sync::OnceLock<Family> = std::sync::OnceLock::new();
static CODE_FAMILY: std::sync::RwLock<Family> = std::sync::RwLock::new(DEFAULT_CODE_FAMILY);

/// A font family by name; blank = `default`.
fn family_or(name: &str, default: Family) -> Family {
    match name.trim() {
        "" => default,
        n => Family::name(n),
    }
}

/// Set the UI font family (blank = the system default). Takes effect only before the first call
/// of [`ui`] / [`bold`]: call it once at start.
pub fn init_ui_font(name: &str) {
    let _ = UI_FAMILY.set(family_or(name, Font::DEFAULT.family));
}

/// Set the code font family (blank = the default); later [`mono`] / [`mono_bold`] calls use it.
pub fn set_code_font(name: &str) {
    let f = family_or(name, DEFAULT_CODE_FAMILY);
    *CODE_FAMILY.write().unwrap_or_else(|e| e.into_inner()) = f;
}

fn ui_family() -> Family {
    *UI_FAMILY.get_or_init(|| Font::DEFAULT.family)
}

fn code_family() -> Family {
    *CODE_FAMILY.read().unwrap_or_else(|e| e.into_inner())
}

/// UI text font (the app's default font).
pub fn ui() -> Font {
    Font {
        family: ui_family(),
        ..Font::DEFAULT
    }
}

/// Bold UI text.
pub fn bold() -> Font {
    Font {
        weight: iced::font::Weight::Bold,
        ..ui()
    }
}

/// Code font (editor, diff, hashes, paths).
pub fn mono() -> Font {
    Font {
        family: code_family(),
        ..Font::MONOSPACE
    }
}

/// Semibold code font.
pub fn mono_bold() -> Font {
    Font {
        weight: iced::font::Weight::Semibold,
        ..mono()
    }
}

/// Code text size and line height (px) in the editor and the diff.
pub const CODE_SIZE: f32 = 12.0;
pub const CODE_LINE: f32 = 20.0;
/// UI text size of rows (tree, changes list).
pub const UI_SIZE: f32 = 13.0;

pub fn bg(c: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(c.into()),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

/// Floating card (find bar, context menu, tooltips).
pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(PANEL.into()),
        text_color: Some(TEXT),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
            offset: iced::Vector::new(0.0, 6.0),
            blur_radius: 18.0,
        },
        ..Default::default()
    }
}

/// Bottom border line under a header.
pub fn header_bar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(HEADER.into()),
        text_color: Some(TEXT),
        border: Border {
            color: BORDER,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// Small rounded badge (diff stats).
pub fn badge(c: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Color { a: 0.15, ..c }.into()),
        text_color: Some(c),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Boxed status letter (changes list), GitHub-Desktop style.
pub fn status_box(c: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: None,
        text_color: Some(c),
        border: Border {
            color: c,
            width: 1.5,
            radius: 3.0.into(),
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

/// Icon button that shows an on/off state.
pub fn toggle(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |t, status| {
        let mut s = icon(t, status);
        if active {
            s.background = Some(SELECTED.into());
            s.text_color = TEXT;
        }
        s
    }
}

/// List row (tree / changes / menu); selected rows keep a highlight.
pub fn row(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = match (selected, status) {
            (true, _) => Some(SELECTED),
            (false, button::Status::Hovered | button::Status::Pressed) => Some(HOVER),
            _ => None,
        };
        let mut s = flat(bg, TEXT);
        s.border.radius = 0.0.into();
        s
    }
}

/// Menu item in a context menu card.
pub fn menu_item(danger: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let text = if danger { GIT_DELETED } else { TEXT };
        match status {
            button::Status::Hovered | button::Status::Pressed => flat(Some(HOVER), text),
            _ => flat(None, text),
        }
    }
}

pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let mut s = text_input::default(theme, status);
    s.background = Background::Color(INPUT_BG);
    s.border.radius = 6.0.into();
    s.border.color = match status {
        text_input::Status::Focused { .. } => ACCENT,
        _ => BORDER,
    };
    s.value = CODE_TEXT;
    s.selection = SELECTION;
    s
}

/// Tooltip below `content`.
pub fn tip<'a, M: 'a>(content: impl Into<iced::Element<'a, M>>, label: String) -> iced::Element<'a, M> {
    use iced::Widget as _;
    iced::widget::tooltip(
        content.into(),
        iced::widget::container(iced::widget::text(label).size(12))
            .padding([3, 7])
            .style(card),
    )
    .position(iced::widget::tooltip::Position::Bottom)
    .boxed()
}

/// Small icon-only button with a tooltip (the app's `icon_button`).
pub fn icon_button<'a, M: Clone + 'a>(glyph: &'a str, msg: Option<M>, label: String) -> iced::Element<'a, M> {
    use iced::Widget as _;
    let b = iced::widget::button(
        iced::widget::text(glyph)
            .size(13)
            .width(iced::Length::Fixed(14.0))
            .align_x(iced::Alignment::Center),
    )
    .padding([3, 5])
    .style(icon)
    .on_press_maybe(msg);
    tip(b.boxed(), label)
}
