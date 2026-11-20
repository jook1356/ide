//! Syntax highlighting shared by [`crate::CodeView`] and [`crate::DiffView`].
//!
//! Parsing is iced's `highlighter` (syntect with the two-face grammar set, parsed in 50-line
//! snapshots and only up to the last visible line inside the editor). Its scope classes
//! ([`Code`]) are coloured here with GitHub dark default colours, the theme Hermes Desk uses
//! (Shiki `github-dark-default` / CodeMirror `githubDark`).

use iced::Code;
use iced::advanced::text::highlighter;
use iced::font;
use iced::{Color, color};
use std::ops::Range;

/// GitHub dark default token colours.
pub fn code_color(code: Code) -> Option<Color> {
    Some(match code {
        Code::Comment => color!(0x8b949e),
        Code::String => color!(0xa5d6ff),
        Code::Keyword => color!(0xff7b72),
        Code::Constant => color!(0x79c0ff),
        Code::Function => color!(0xd2a8ff),
        Code::Type => color!(0xffa657),
        Code::Support => color!(0x79c0ff),
        Code::Invalid => color!(0xffa198),
        Code::Variable | Code::Punctuation | Code::Path | Code::Other => return None,
    })
}

/// [`highlighter::Style`] for a [`Code`] (comments in italics, like the GitHub theme).
pub fn code_style(code: Code) -> highlighter::Style {
    highlighter::Style {
        color: code_color(code),
        style: (code == Code::Comment).then_some(font::Style::Italic),
    }
}

/// The token the syntect parser understands for `path` (an extension or a grammar name), or
/// `None` for plain text / unknown files.
pub fn language_token(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_lowercase();
    let by_name = match name.as_str() {
        "dockerfile" => Some("Dockerfile"),
        "makefile" | "gnumakefile" => Some("Makefile"),
        "cmakelists.txt" => Some("cmake"),
        "cargo.lock" => Some("toml"),
        ".bashrc" | ".profile" | ".zshrc" => Some("sh"),
        _ => None,
    };
    if let Some(t) = by_name {
        return Some(t.to_owned());
    }
    let (_, ext) = name.rsplit_once('.')?;
    let token = match ext {
        "" | "txt" | "log" => return None,
        "mjs" | "cjs" => "js",
        "mts" | "cts" => "ts",
        "yml" => "yaml",
        "ps1" | "psm1" => "ps1",
        "bat" | "cmd" => "bat",
        "h" => "c",
        "hpp" | "cc" | "cxx" => "cpp",
        "pyi" => "py",
        "bash" | "zsh" => "sh",
        "kts" => "kt",
        "svelte" | "vue" => "html",
        "jsonc" | "json5" => "json",
        other => other,
    };
    Some(token.to_owned())
}

/// Highlighted ranges of each line of a continuous snippet (`lines` in order; multi-line strings
/// and comments carry over). Empty vectors for plain text.
pub fn highlight_lines<'a>(
    token: &str,
    lines: impl IntoIterator<Item = &'a str>,
) -> Vec<Vec<(Range<usize>, Code)>> {
    let mut stream = iced::highlighter::Stream::new(&iced::highlighter::Settings {
        token: token.to_owned(),
    });
    lines
        .into_iter()
        .map(|line| {
            let spans: Vec<_> = stream
                .parse_line(line)
                .filter(|(_, c)| code_color(*c).is_some())
                .collect();
            stream.commit();
            spans
        })
        .collect()
}
