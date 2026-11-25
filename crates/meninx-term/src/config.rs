//! Terminal appearance and behaviour settings.
use iced::Color;

/// Colours used by the terminal. `ansi` holds the 16 base colours (0-7 normal, 8-15 bright);
/// indices 16-255 follow the xterm 256-colour cube and grey ramp.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub foreground: Color,
    pub background: Color,
    pub cursor: Color,
    pub selection: Color,
    /// IME preedit box background and underline.
    pub preedit_background: Color,
    pub preedit_underline: Color,
    pub ansi: [Color; 16],
}

impl Default for Palette {
    /// A dark theme (VS Code "Dark+"-like colours on the app's header ground).
    fn default() -> Self {
        const BASE16: [(u8, u8, u8); 16] = [
            (0x00, 0x00, 0x00),
            (0xcd, 0x31, 0x31),
            (0x0d, 0xbc, 0x79),
            (0xe5, 0xe5, 0x10),
            (0x24, 0x72, 0xc8),
            (0xbc, 0x3f, 0xbc),
            (0x11, 0xa8, 0xcd),
            (0xe5, 0xe5, 0xe5),
            (0x66, 0x66, 0x66),
            (0xf1, 0x4c, 0x4c),
            (0x23, 0xd1, 0x8b),
            (0xf5, 0xf5, 0x43),
            (0x3b, 0x8e, 0xea),
            (0xd6, 0x70, 0xd6),
            (0x29, 0xb8, 0xdb),
            (0xff, 0xff, 0xff),
        ];
        Palette {
            foreground: Color::from_rgb8(0xd4, 0xd4, 0xd4),
            // The app's header / composer strip (`meninx-app` `style::HEADER`), so the terminal and
            // the input area under it read as one surface. Keep `meninx-server`'s `pty::palette`
            // (OSC 11 replies) in sync.
            background: Color::from_rgb8(0x26, 0x28, 0x2d),
            cursor: Color::from_rgba8(0xd0, 0xd0, 0xd0, 0.55),
            selection: Color::from_rgb8(0x26, 0x4f, 0x78),
            preedit_background: Color::from_rgb8(0x3a, 0x3d, 0x41),
            preedit_underline: Color::from_rgb8(0xff, 0xcc, 0x00),
            ansi: BASE16.map(|(r, g, b)| Color::from_rgb8(r, g, b)),
        }
    }
}

impl Palette {
    /// Colour for an xterm 256-colour index.
    pub fn indexed(&self, i: u8) -> Color {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let c = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Color::from_rgb8(c(i / 36), c((i / 6) % 6), c(i % 6))
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                Color::from_rgb8(v, v, v)
            }
        }
    }
}

/// Settings shared by [`TermSession`](crate::TermSession) and [`TerminalView`](crate::TerminalView).
#[derive(Debug, Clone, PartialEq)]
pub struct TermConfig {
    /// Monospace font family (system font name).
    pub font_family: String,
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Minimum row height in logical pixels (the measured glyph height is used if larger).
    pub line_height: f32,
    /// Inner padding of the widget in logical pixels.
    pub padding: f32,
    /// Scrollback history in lines (applied when a session is spawned).
    pub scrollback_lines: usize,
    /// Lines scrolled per mouse-wheel notch.
    pub wheel_lines: f32,
    pub palette: Palette,
}

impl Default for TermConfig {
    fn default() -> Self {
        TermConfig {
            font_family: "Consolas".into(),
            font_size: 14.0,
            line_height: 18.0,
            padding: 4.0,
            scrollback_lines: 10_000,
            wheel_lines: 3.0,
            palette: Palette::default(),
        }
    }
}
