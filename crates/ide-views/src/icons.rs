//! Small drawn icons: file-type badges (by extension), folders, git status letters.

use crate::theme;
use iced::Widget as _;
use iced::widget::{column, container, space, text};
use iced::{Border, Color, Element, Length, color};

/// Short label and colour of a file-type badge for `name` (extension / well-known names).
pub fn file_kind(name: &str) -> (&'static str, Color) {
    let lower = name.to_lowercase();
    match lower.as_str() {
        "cargo.toml" | "cargo.lock" => return ("RS", color!(0xdea584)),
        "package.json" => return ("{}", color!(0x8bc34a)),
        "dockerfile" => return ("DK", color!(0x0db7ed)),
        "makefile" => return ("MK", color!(0x6d8086)),
        ".gitignore" | ".gitattributes" => return ("GI", color!(0xf14e32)),
        "license" | "license.md" => return ("LI", color!(0xd0b344)),
        _ => {}
    }
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "rs" => ("RS", color!(0xdea584)),
        "ts" | "mts" | "cts" => ("TS", color!(0x3178c6)),
        "tsx" | "jsx" => ("RX", color!(0x61dafb)),
        "js" | "mjs" | "cjs" => ("JS", color!(0xf1e05a)),
        "json" | "jsonc" => ("{}", color!(0xcbcb41)),
        "md" | "mdx" => ("MD", color!(0x519aba)),
        "py" | "pyi" => ("PY", color!(0x3572a5)),
        "toml" => ("TM", color!(0x9c4221)),
        "yml" | "yaml" => ("YM", color!(0xcb171e)),
        "html" | "htm" => ("<>", color!(0xe34c26)),
        "css" | "scss" | "less" => ("#", color!(0x563d7c)),
        "go" => ("GO", color!(0x00add8)),
        "c" | "h" => ("C", color!(0x6e7f99)),
        "cpp" | "hpp" | "cc" | "cxx" => ("C+", color!(0xf34b7d)),
        "cs" => ("C#", color!(0x178600)),
        "java" | "kt" | "kts" => ("JV", color!(0xb07219)),
        "sh" | "bash" | "zsh" | "ps1" | "bat" | "cmd" => (">_", color!(0x89e051)),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "svg" => ("IM", color!(0xa074c4)),
        "lock" => ("LK", color!(0x8b8f98)),
        "txt" | "log" => ("TX", color!(0x8b8f98)),
        "vue" => ("VU", color!(0x41b883)),
        "svelte" => ("SV", color!(0xff3e00)),
        _ => ("", color!(0x8b8f98)),
    }
}

/// 14×14 file-type badge.
pub fn file_icon<'a, M: 'a>(name: &str) -> Element<'a, M> {
    let (label, c) = file_kind(name);
    if label.is_empty() {
        // Plain page outline for unknown types.
        return container(space())
            .width(10)
            .height(13)
            .style(move |_| container::Style {
                border: Border {
                    color: c,
                    width: 1.2,
                    radius: 2.0.into(),
                },
                ..Default::default()
            })
            .center_x(14)
            .boxed();
    }
    container(
        text(label)
            .size(7.5)
            .font(theme::BOLD)
            .color(color!(0x1b1c1f))
            .line_height(1.0),
    )
    .center(14)
    .style(move |_| container::Style {
        background: Some(c.into()),
        border: Border {
            radius: 3.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .boxed()
}

/// 14×14 outlined folder (tab + body), like Hermes Desk's tabler folder icon.
pub fn folder_icon<'a, M: 'a>(open: bool, tint: Color) -> Element<'a, M> {
    let tab = container(space())
        .width(6)
        .height(3)
        .style(move |_| container::Style {
            border: Border {
                color: tint,
                width: 1.2,
                radius: iced::border::top(1.5),
            },
            ..Default::default()
        });
    let body = container(space())
        .width(13)
        .height(9)
        .style(move |_| container::Style {
            background: open.then(|| Color { a: 0.25, ..tint }.into()),
            border: Border {
                color: tint,
                width: 1.2,
                radius: 2.0.into(),
            },
            ..Default::default()
        });
    container(column![tab, body])
        .width(Length::Fixed(14.0))
        .height(Length::Fixed(14.0))
        .padding(iced::Padding {
            top: 1.0,
            left: 0.5,
            ..Default::default()
        })
        .boxed()
}
