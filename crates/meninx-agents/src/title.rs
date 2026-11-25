//! Session titles from the user's first prompt (`docs/session-titles.md`).
//!
//! [`AgentProvider::prompt_title`](crate::AgentProvider::prompt_title) makes a title at once
//! (default: [`from_prompt`], no model call); a CLI that names its conversations itself reports
//! that name later through
//! [`AgentProvider::conversation_title`](crate::AgentProvider::conversation_title).

/// Longest automatic title, in characters (an ellipsis is added when cut).
pub const TITLE_MAX: usize = 40;

/// A short title from a prompt: its first meaningful line (Markdown markers, `@file` mentions
/// and extra spaces removed), cut at the end of its first sentence or at a word boundary
/// within [`TITLE_MAX`] characters. `None` for an empty prompt or one that starts with one of
/// `commands` (a CLI's own commands, e.g. `/clear` or `!ls`: no request to name a session by).
pub fn from_prompt(prompt: &str, commands: &[&str]) -> Option<String> {
    let trimmed = prompt.trim_start();
    if commands.iter().any(|c| trimmed.starts_with(c)) {
        return None;
    }
    let line = trimmed.lines().map(clean_line).find(|l| !l.is_empty())?;
    Some(shorten(first_sentence(&line), TITLE_MAX))
}

/// A line without Markdown markers, `@file` mentions and repeated spaces.
fn clean_line(line: &str) -> String {
    let line = line
        .trim()
        .trim_start_matches(['#', '>', '-', '*', '+'])
        .trim();
    if line.starts_with("```") {
        return String::new();
    }
    line.split_whitespace()
        .filter(|w| !(w.starts_with('@') && w.len() > 1))
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(['*', '_', '`'])
        .trim()
        .to_owned()
}

/// The text up to the end of the first sentence (`.`, `?`, `!` followed by a space, or `。`),
/// without the final period.
fn first_sentence(s: &str) -> &str {
    let bytes = s.as_bytes();
    for (i, c) in s.char_indices() {
        let end = match c {
            '。' => true,
            '.' | '?' | '!' => bytes.get(i + 1).is_some_and(|b| *b == b' '),
            _ => false,
        };
        if end {
            return if c == '?' || c == '!' {
                &s[..i + c.len_utf8()]
            } else {
                &s[..i]
            };
        }
    }
    s.strip_suffix('.').unwrap_or(s)
}

/// At most `max` characters: cut at the last space within the limit (if there is one in its
/// second half), with `…`.
fn shorten(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max - 1).collect();
    let cut = match cut.rfind(' ') {
        Some(i) if cut[..i].chars().count() >= max / 2 => &cut[..i],
        _ => &cut[..],
    };
    format!("{}…", cut.trim_end_matches([' ', ',', '.', ':', ';']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_prompt_is_kept() {
        assert_eq!(from_prompt("Fix the login bug", &[]).as_deref(), Some("Fix the login bug"));
        assert_eq!(from_prompt("  \n\n안녕?\n", &[]).as_deref(), Some("안녕?"));
    }

    #[test]
    fn first_sentence_and_line() {
        assert_eq!(
            from_prompt("Fix the login bug. It fails on Windows.\nMore text", &[]).as_deref(),
            Some("Fix the login bug")
        );
        assert_eq!(
            from_prompt("## Task\nsomething", &[]).as_deref(),
            Some("Task"),
            "Markdown heading"
        );
        assert_eq!(from_prompt("```\ncode\n```", &[]).as_deref(), Some("code"));
        assert_eq!(
            from_prompt("@src/main.rs 의 패닉 고쳐줘", &[]).as_deref(),
            Some("의 패닉 고쳐줘"),
            "file mentions are dropped"
        );
        assert_eq!(from_prompt("세션 제목을 요약해줘。다음", &[]).as_deref(), Some("세션 제목을 요약해줘"));
        assert_eq!(from_prompt("v1.2 release notes", &[]).as_deref(), Some("v1.2 release notes"));
    }

    #[test]
    fn long_prompt_is_cut() {
        let t = from_prompt(
            "Please refactor the session list so that pinned sessions stay at the top of every group",
            &[],
        )
        .unwrap();
        assert!(t.chars().count() <= TITLE_MAX, "{t}");
        assert_eq!(t, "Please refactor the session list so…");
        let ko = from_prompt(&"가".repeat(100), &[]).unwrap();
        assert_eq!(ko.chars().count(), TITLE_MAX);
        assert!(ko.ends_with('…'));
    }

    #[test]
    fn empty_and_commands() {
        assert_eq!(from_prompt("", &[]), None);
        assert_eq!(from_prompt("   \n  ", &[]), None);
        assert_eq!(from_prompt("/clear", &["/", "!"]), None);
        assert_eq!(from_prompt("!git status", &["/", "!"]), None);
        assert_eq!(from_prompt("/clear", &[]).as_deref(), Some("/clear"));
    }
}
